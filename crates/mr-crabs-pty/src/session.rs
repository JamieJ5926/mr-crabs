//! PTY session lifecycle: spawn, resize, bounded input/output, and process
//! termination/reaping.
//!
//! This module owns the `PtySession` handle, `PtyConfig`, and `ExitStatus`.
//! All low-level fork/openpty work is delegated to [`crate::platform`]; all
//! bounded queue primitives come from [`crate::queue`]. Every channel used
//! here is a bounded `std::sync::mpsc::sync_channel` — there are no unbounded
//! channels in the production PTY path.
//!
//! Reaping discipline: exactly one child status is produced. The exit waiter
//! blocks in the kernel via [`crate::platform::waitpid_block`] (no 5 ms
//! polling), `try_wait`/`shutdown_and_reap` poll via nonblocking
//! [`crate::platform::waitpid_nonblock`], and all sides go through a shared
//! [`OnceLock<ExitStatus>`] so any interleaving is safe: the kernel lets
//! exactly one `waitpid` reap the child and any loser observes `ECHILD`
//! (mapped to `None` by the platform layer), then reads the status the
//! winner cached in the shared `OnceLock`. The `OnceLock` makes the status
//! available to every caller and guarantees the child is reported exactly
//! once.
//! Optional [`OutputWake`] callbacks are cloned into the reader and exit
//! threads. They fire after each successful output enqueue, once when the
//! reader terminates (EOF/EIO/error/disconnect), and after exit-status
//! publication. Callbacks carry no bytes or status and must not touch
//! GPUI, model, or frame state; the bounded channels remain authoritative.

use std::io;
use std::os::fd::{AsRawFd, OwnedFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvError, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, OnceLock};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use libc::{EBADF, F_GETFL, F_SETFL, O_NONBLOCK, SIGHUP, SIGKILL, SIGTERM};

use crate::queue;
use crate::{CommandBuilder, PtyError, PtySize, PtySizeError, WriteError};

/// Thread-safe notification invoked after PTY output enters the bounded
/// reader queue, once when the reader thread terminates, and after the
/// child's exit status is published. The callback must return quickly and
/// must not touch GPUI, model, or frame state; it carries no bytes or
/// status. Delivery and backpressure remain owned by the bounded channels.
pub type OutputWake = Arc<dyn Fn() + Send + Sync + 'static>;

/// Retry interval for a full consumer queue. Kernel PTY readiness uses
/// `poll(2)` below, so normal I/O wakes immediately rather than sleeping.
const BACKPRESSURE_SLEEP: Duration = Duration::from_millis(1);
const IO_POLL_TIMEOUT_MS: libc::c_int = 100;
/// Exactly one of [`ExitStatus::code`] and [`ExitStatus::signal`] is `Some`:
/// `code` is set when the process terminated normally (the `WEXITSTATUS`
/// value), `signal` when it was killed by a signal (the `WTERMSIG` value).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExitStatus {
    /// Exit code when the process terminated normally; `None` when killed by
    /// a signal.
    pub code: Option<i32>,
    /// Signal number when the process was killed by a signal; `None` when it
    /// terminated normally.
    pub signal: Option<i32>,
}

impl ExitStatus {
    /// A normally-terminated process with the given exit code.
    pub fn exited(code: i32) -> Self {
        Self {
            code: Some(code),
            signal: None,
        }
    }

    /// A process killed by the given signal.
    pub fn signaled(signal: i32) -> Self {
        Self {
            code: None,
            signal: Some(signal),
        }
    }

    /// Exit code, if the process terminated normally.
    pub fn code(&self) -> Option<i32> {
        self.code
    }

    /// Signal number, if the process was killed by a signal.
    pub fn signal(&self) -> Option<i32> {
        self.signal
    }
}

impl From<i32> for ExitStatus {
    /// Maps a raw `waitpid` status word into an [`ExitStatus`].
    ///
    /// The status word layout is POSIX `waitpid`: the low 7 bits are the
    /// signal (0 when exited normally), bit 8 is the core-dump flag, and the
    /// exit code occupies bits 8..16 for normal exits. We never call
    /// `waitpid` with `WUNTRACED`, so stopped/continued states never appear.
    fn from(status: i32) -> Self {
        if status & 0o177 == 0 {
            Self::exited((status >> 8) & 0xff)
        } else {
            Self::signaled(status & 0o177)
        }
    }
}

/// Configuration for [`PtySession::spawn`].
///
/// The queue capacities default to the crate-wide
/// [`WRITER_QUEUE_CAPACITY`](queue::WRITER_QUEUE_CAPACITY) and
/// [`READER_QUEUE_CAPACITY`](queue::READER_QUEUE_CAPACITY) constants and can
/// be overridden per spawn.
pub struct PtyConfig {
    /// Initial PTY dimensions applied before the child is exec'd.
    pub size: PtySize,
    /// Command to run in the PTY.
    pub command: CommandBuilder,
    /// Capacity of the bounded writer queue (bytes chunks enqueued by
    /// `try_write`/`write_timeout` before the writer thread applies
    /// backpressure).
    pub writer_capacity: usize,
    /// Capacity of the bounded reader output queue delivered to the caller.
    pub reader_capacity: usize,
    /// Optional edge notification cloned into the reader and exit threads.
    /// Consumers should coalesce callbacks before scheduling main-thread work.
    pub output_wake: Option<OutputWake>,
}

impl PtyConfig {
    /// A spawn configuration with default bounded queue capacities.
    pub fn new(command: CommandBuilder, size: PtySize) -> Self {
        Self {
            size,
            command,
            writer_capacity: queue::WRITER_QUEUE_CAPACITY,
            reader_capacity: queue::READER_QUEUE_CAPACITY,
            output_wake: None,
        }
    }

    /// Override the bounded writer queue capacity.
    pub fn with_writer_capacity(mut self, capacity: usize) -> Self {
        self.writer_capacity = capacity;
        self
    }

    /// Override the bounded reader output queue capacity.
    pub fn with_reader_capacity(mut self, capacity: usize) -> Self {
        self.reader_capacity = capacity;
        self
    }

    /// Notify the consumer after each chunk successfully enters the bounded
    /// output queue, once when the reader terminates, and after exit-status
    /// publication. The queues remain authoritative; this is only a wakeup.
    pub fn with_output_wake(mut self, wake: OutputWake) -> Self {
        self.output_wake = Some(wake);
        self
    }
}

/// A live PTY session: the master side of a pseudo-terminal plus the child
/// process running against it.
///
/// Data flow:
/// - `try_write` / `write_timeout` / `write` enqueue input chunks into a
///   bounded writer queue; the writer thread drains it into the master.
/// - The reader thread copies master output into a bounded queue delivered
///   to the caller as an ordered stream of `Vec<u8>` chunks.
/// - The exit waiter thread reaps the child and reports its status both
///   through the exit notification channel and the shared status cache.
///   A configured [`OutputWake`] is cloned into the reader and exit
///   threads so those events can schedule consumer work without polling.
///
/// The session is single-owner: `shutdown_and_reap` must be called (or the
/// session dropped) to terminate and reap the child; every successful
/// [`PtySession::spawn`] is reaped exactly once.
pub struct PtySession {
    /// The session's own handle to the PTY master, used for resize. The
    /// reader and writer threads operate on their own clones, so closing
    /// this handle does not disturb them.
    master: Option<OwnedFd>,
    /// PID of the child (the session/process-group leader).
    child_pid: i32,
    /// Sender side of the bounded writer queue; `None` once shutdown has
    /// closed the queue.
    writer_tx: Option<SyncSender<Vec<u8>>>,
    /// Sender side of the bounded resize ledger channel (capacity 16),
    /// used to coalesce rapid resize requests; `None` once shutdown.
    resize_tx: Option<SyncSender<PtySize>>,
    /// Receiver side of the resize ledger channel; drained by `resize` so
    /// only the newest size is ever pending.
    resize_rx: Receiver<PtySize>,
    /// Last size applied to the master; identical requests are ignored.
    last_size: Mutex<PtySize>,
    /// Set when `shutdown_and_reap` (or `Drop`) begins; makes shutdown
    /// idempotent and tells the reader/writer threads to stop.
    shutdown: Arc<AtomicBool>,
    /// Shared, exactly-once child status cache. The exit waiter thread and
    /// every reaping caller (`try_wait`, `shutdown_and_reap`) publish and
    /// read through this guard, so the child can never be reaped twice.
    wait_guard: Arc<OnceLock<ExitStatus>>,
    reader_handle: Option<JoinHandle<()>>,
    writer_handle: Option<JoinHandle<()>>,
    exit_handle: Option<JoinHandle<()>>,
}

/// Handles returned by [`PtySession::spawn`]: the owned session, bounded
/// output receiver, and single-result exit receiver.
pub type SpawnedPty = (PtySession, Receiver<Vec<u8>>, Receiver<ExitStatus>);

impl PtySession {
    /// Spawns `config.command` in a new PTY of `config.size` and returns the
    /// session together with:
    /// - `output`: an ordered, bounded stream of chunks read from the master;
    /// - `exit`: a single-slot notification carrying the child's
    ///   [`ExitStatus`] once it has been reaped.
    ///
    /// A configured [`PtyConfig::output_wake`] is cloned into the reader and
    /// exit threads. It is invoked after each successful output enqueue, once
    /// when the reader terminates, and after exit-status publication. The
    /// callback carries no payload; the returned channels remain authoritative.
    ///
    /// The child is made a session/process-group leader with the PTY slave as
    /// its controlling terminal by the platform layer; the parent retains
    /// only the master.
    ///
    /// If any setup step fails after the child was created, the child is
    /// killed and reaped before the error is returned, so a failed spawn
    /// never leaks a live process.
    pub fn spawn(config: PtyConfig) -> Result<SpawnedPty, PtyError> {
        // Defensive re-validation: `PtySize::new` enforces nonzero rows/cols,
        // but the fields are public so a struct literal could bypass it.
        if config.size.cols == 0 {
            return Err(PtyError::InvalidSize(PtySizeError::ZeroColumns));
        }
        if config.size.rows == 0 {
            return Err(PtyError::InvalidSize(PtySizeError::ZeroRows));
        }

        let built = config.command.to_spawn_command();
        let (master, child_pid) = crate::platform::spawn_pty_child(config.size, &built)?;

        // Clone the master for the reader and writer threads; the session
        // keeps the original for resize ioctls.
        let reader_master = match master.try_clone() {
            Ok(clone) => clone,
            Err(err) => {
                abort_child(child_pid, None);
                return Err(PtyError::Io(err));
            }
        };
        let writer_master = match master.try_clone() {
            Ok(clone) => clone,
            Err(err) => {
                abort_child(child_pid, None);
                return Err(PtyError::Io(err));
            }
        };

        // All bounded channels; no unbounded channel anywhere in this path.
        let (writer_tx, writer_rx) = std::sync::mpsc::sync_channel(config.writer_capacity);
        let (output_tx, output_rx) = std::sync::mpsc::sync_channel(config.reader_capacity);
        let (exit_tx, exit_rx) = std::sync::mpsc::sync_channel::<ExitStatus>(1);
        let (resize_tx, resize_rx) = std::sync::mpsc::sync_channel::<PtySize>(16);

        let shutdown = Arc::new(AtomicBool::new(false));
        let wait_guard: Arc<OnceLock<ExitStatus>> = Arc::new(OnceLock::new());
        let exit_wait_guard = Arc::clone(&wait_guard);
        let reader_shutdown = Arc::clone(&shutdown);
        let writer_shutdown = Arc::clone(&shutdown);
        let output_wake = config.output_wake;
        let reader_wake = output_wake.clone();
        let exit_wake = output_wake;

        let exit_handle = match thread::Builder::new()
            .name("mr-crabs-pty-exit".to_owned())
            .spawn(move || exit_waiter(child_pid, exit_tx, exit_wait_guard, exit_wake))
        {
            Ok(handle) => handle,
            Err(err) => {
                abort_child(child_pid, None);
                return Err(PtyError::Io(err));
            }
        };
        let reader_handle = match thread::Builder::new()
            .name("mr-crabs-pty-reader".to_owned())
            .spawn(move || reader_loop(reader_master, output_tx, reader_shutdown, reader_wake))
        {
            Ok(handle) => handle,
            Err(err) => {
                abort_child(child_pid, Some(exit_handle));
                return Err(PtyError::Io(err));
            }
        };
        let writer_handle = match thread::Builder::new()
            .name("mr-crabs-pty-writer".to_owned())
            .spawn(move || writer_loop(writer_master, writer_rx, writer_shutdown))
        {
            Ok(handle) => handle,
            Err(err) => {
                abort_child(child_pid, Some(exit_handle));
                return Err(PtyError::Io(err));
            }
        };

        let session = Self {
            master: Some(master),
            child_pid,
            writer_tx: Some(writer_tx),
            resize_tx: Some(resize_tx),
            resize_rx,
            last_size: Mutex::new(config.size),
            shutdown,
            wait_guard,
            reader_handle: Some(reader_handle),
            writer_handle: Some(writer_handle),
            exit_handle: Some(exit_handle),
        };
        Ok((session, output_rx, exit_rx))
    }

    /// Applies a new size to the PTY.
    ///
    /// Requests identical to the last applied size are ignored. Rapid resize
    /// calls are coalesced through a bounded (capacity 16) ledger channel:
    /// stale pending sizes are drained and only the newest request survives,
    /// so the final dimensions always win and the ledger never overflows.
    /// The `TIOCSWINSZ` ioctl is applied synchronously on the master; if the
    /// master has been closed by shutdown, an `EBADF` io error is returned.
    pub fn resize(&self, size: PtySize) -> Result<(), PtyError> {
        let mut last = match self.last_size.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        if *last == size {
            return Ok(());
        }
        *last = size;

        // Non-blocking coalescing: drop any pending sizes (all older than
        // `size`), then record the newest one.
        if let Some(tx) = self.resize_tx.as_ref() {
            while self.resize_rx.try_recv().is_ok() {}
            match tx.try_send(size) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) => {
                    // Ledger full of stale entries: drain and retry once.
                    while self.resize_rx.try_recv().is_ok() {}
                    match tx.try_send(size) {
                        Ok(()) => {}
                        Err(TrySendError::Full(_)) => return Err(PtyError::QueueFull),
                        Err(TrySendError::Disconnected(_)) => return Err(PtyError::QueueClosed),
                    }
                }
                Err(TrySendError::Disconnected(_)) => return Err(PtyError::QueueClosed),
            }
        }

        // Apply the final size to the master.
        match self.master.as_ref() {
            Some(master) => crate::platform::set_winsize(master, size).map_err(PtyError::Io),
            None => Err(PtyError::Io(io::Error::from_raw_os_error(EBADF))),
        }
    }

    /// Non-blocking bounded enqueue of input for the child.
    ///
    /// Returns [`WriteError::Full`] when the writer queue is at capacity
    /// (the caller must apply backpressure), [`WriteError::Closed`] when the
    /// session is shut down, and `Ok` when the chunk was accepted.
    pub fn try_write(&self, data: &[u8]) -> Result<(), WriteError> {
        let tx = self.writer_tx.as_ref().ok_or(WriteError::Closed)?;
        queue::try_send(tx, data.to_vec()).map_err(WriteError::from)
    }

    /// Bounded enqueue of input with an overall deadline.
    ///
    /// Blocks while the writer queue is full, polling until `timeout`
    /// elapses ([`WriteError::Timeout`]) or the queue closes
    /// ([`WriteError::Closed`]).
    pub fn write_timeout(&self, data: &[u8], timeout: Duration) -> Result<(), WriteError> {
        let tx = self.writer_tx.as_ref().ok_or(WriteError::Closed)?;
        queue::send_timeout(tx, data.to_vec(), timeout).map_err(WriteError::from)
    }

    /// Blocking bounded enqueue of input.
    ///
    /// Returns when the chunk is accepted by the queue (the writer thread
    /// applies the actual PTY backpressure), or [`WriteError::Closed`] once
    /// the queue is closed by shutdown.
    pub fn write(&self, data: Vec<u8>) -> Result<(), WriteError> {
        let tx = self.writer_tx.as_ref().ok_or(WriteError::Closed)?;
        tx.send(data).map_err(|_| WriteError::Closed)
    }

    /// PID of the child process (session/process-group leader).
    pub fn child_pid(&self) -> i32 {
        self.child_pid
    }

    /// Non-blocking check for child exit.
    ///
    /// Returns `Ok(Some(status))` once the child has been reaped, `Ok(None)`
    /// while it is still running. Reaping is guarded by the shared
    /// [`OnceLock`] and only ever happens once, even when racing the exit
    /// waiter thread (both sides use nonblocking `waitpid`, so a loser
    /// observes `ECHILD` and reads the cached status instead).
    pub fn try_wait(&mut self) -> Result<Option<ExitStatus>, PtyError> {
        if let Some(status) = self.wait_guard.get().copied() {
            return Ok(Some(status));
        }
        if let Some(status) = crate::platform::waitpid_nonblock(self.child_pid) {
            let _ = self.wait_guard.set(status);
            Ok(Some(status))
        } else {
            Ok(None)
        }
    }

    /// Terminates the child and reaps it, exactly once, idempotently.
    ///
    /// Sequence:
    /// 1. Close the writer queue (the writer thread drains and exits).
    /// 2. Close the resize ledger and the session's master handle, so the
    ///    child's controlling terminal goes away.
    /// 3. `SIGHUP` then `SIGTERM` to the child's process group.
    /// 4. Wait up to `grace` for the child to exit (polled every 10 ms).
    /// 5. Escalate to `SIGKILL` if it is still alive.
    /// 6. Join the exit waiter (it reaps promptly once the child dies) and
    ///    return the cached status.
    ///
    /// A second call (e.g. from `Drop` after an explicit shutdown) observes
    /// the shutdown flag and returns the cached status without re-signaling
    /// or re-reaping. Kill errors for an already-dead process group are
    /// ignored; the reap is the authoritative liveness check.
    pub fn shutdown_and_reap(&mut self, grace: Duration) -> Result<ExitStatus, PtyError> {
        // Idempotency gate: only the first caller runs the terminate
        // sequence; later callers return the cached status.
        if self.shutdown.swap(true, Ordering::AcqRel) {
            return match self.wait_guard.get().copied() {
                Some(status) => Ok(status),
                // Unreachable with `&mut self` ownership: a completed
                // shutdown always cached a status before returning.
                None => Err(PtyError::Io(io::Error::new(
                    io::ErrorKind::WouldBlock,
                    "shutdown already in progress",
                ))),
            };
        }

        // 1. Close the writer queue; the writer thread finishes any in-flight
        //    chunk, discards the remainder, and exits.
        self.writer_tx = None;
        if let Some(handle) = self.writer_handle.take() {
            let _ = handle.join();
        }

        // 2. Close the resize ledger and the master. The reader/writer
        //    threads hold their own clones and exit on the shutdown flag.
        self.resize_tx = None;
        self.master = None;

        // 3. Signal the child's process group; ESRCH (already dead) and any
        //    other failure are best-effort — the reap below is authoritative.
        let _ = crate::platform::kill_pgid(self.child_pid, SIGHUP);
        let _ = crate::platform::kill_pgid(self.child_pid, SIGTERM);

        // 4. Grace window: poll for exit every 10 ms.
        let deadline = Instant::now() + grace;
        let mut reaped = self.try_wait()?.is_some();
        while !reaped && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
            reaped = self.try_wait()?.is_some();
        }

        // 5. Escalate.
        if !reaped {
            let _ = crate::platform::kill_pgid(self.child_pid, SIGKILL);
        }

        // 6. The exit waiter reaps within one poll interval of the child's
        //    death; joining it guarantees the reap completed exactly once.
        if let Some(handle) = self.exit_handle.take() {
            let _ = handle.join();
        }

        // 7. Stop the reader thread (it exits within one poll interval of
        //    the shutdown flag or the master closing).
        if let Some(handle) = self.reader_handle.take() {
            let _ = handle.join();
        }

        // 8. Return the cached status; a backstop nonblocking reap covers
        //    the (unreachable) case where the exit waiter could not reap.
        if let Some(status) = self.wait_guard.get().copied() {
            return Ok(status);
        }
        if let Some(status) = crate::platform::waitpid_nonblock(self.child_pid) {
            let _ = self.wait_guard.set(status);
            return Ok(status);
        }
        Err(PtyError::Io(io::Error::new(
            io::ErrorKind::NotFound,
            "child process status unavailable after shutdown",
        )))
    }
}

impl Drop for PtySession {
    /// Bounded best-effort terminate/reap without panic.
    ///
    /// Runs the same sequence as [`PtySession::shutdown_and_reap`] with a
    /// short grace period if the session was not explicitly shut down. Every
    /// operation in that path is panic-free (no `unwrap`/`expect`, poisoned
    /// locks tolerated, join errors ignored), so dropping a session can
    /// never panic.
    fn drop(&mut self) {
        if !self.shutdown.load(Ordering::Acquire) {
            let _ = self.shutdown_and_reap(Duration::from_millis(200));
        }
    }
}

/// Waits for the child to exit, caching and reporting its status exactly
/// once.
///
/// Blocks in the kernel via [`crate::platform::waitpid_block`] (no 5 ms
/// polling). If the caller has already set the shared guard (fake pre-set
/// guard used by tests) the waiter returns that status without issuing
/// `waitpid`. If the blocking wait observes `ECHILD` (already reaped by a
/// racing `try_wait`), it reads the status the winner cached in
/// `wait_guard` before publishing. The status is cached in `wait_guard`
/// before the exit channel send and the wake, so `try_wait`/`shutdown` and
/// wake observers always see a coherent ordering.
fn exit_waiter(
    child_pid: i32,
    exit_tx: SyncSender<ExitStatus>,
    wait_guard: Arc<OnceLock<ExitStatus>>,
    output_wake: Option<OutputWake>,
) {
    // Fake pre-set guard: tests pre-populate `wait_guard` and expect the
    // waiter to deliver that value without waiting for a real child. This
    // branch is taken only when the guard is already set at entry.
    if let Some(status) = wait_guard.get().copied() {
        let _ = exit_tx.send(status);
        invoke_output_wake(output_wake.as_ref());
        return;
    }
    let status = match crate::platform::waitpid_block(child_pid) {
        Some(status) => {
            // `OnceLock::set` may fail if `try_wait` raced and already set
            // the status; both raced values are the same child's exit.
            let _ = wait_guard.set(status);
            // `set` above may have lost the race; return the canonical
            // cached value so every observer sees one status.
            wait_guard.get().copied().unwrap_or(status)
        }
        None => {
            // `ECHILD`: `try_wait`/shutdown already reaped and cached the
            // status. The shared guard now holds it; if not yet visible
            // (narrow pre-publish window), yield until it appears rather
            // than polling at 5 ms — the winner always sets the guard
            // immediately after its successful `waitpid`.
            loop {
                if let Some(status) = wait_guard.get().copied() {
                    break status;
                }
                // No busy-wait: `try_wait` holds the same pid and reaps
                // under the same lock discipline; a single yield covers the
                // publish; a short backoff bounds the wait even under
                // pathological interleaving.
                thread::sleep(Duration::from_millis(1));
            }
        }
    };
    let _ = exit_tx.send(status);
    invoke_output_wake(output_wake.as_ref());
}

/// Copies master output into the bounded output queue, preserving byte
/// order, until EOF/EIO (clean closure) or shutdown.
///
/// The master clone is switched to nonblocking mode so the loop can observe
/// the shutdown flag while the PTY is idle; `EINTR` is retried, `EAGAIN`
/// sleeps one poll interval, and `EIO` (macOS "PTY closed"), `Ok(0)` (EOF),
/// or any other error ends the loop cleanly. Delivery uses `try_send` with a
/// bounded retry loop, so a stalled consumer applies backpressure without
/// unbounded buffering, and shutdown is never blocked behind the consumer.
/// If the caller drops the output receiver, output is drained and discarded
/// so the child never blocks on a full PTY buffer.
///
/// A configured wake fires after each successful enqueue and once more when
/// this thread terminates, after the output sender is dropped so the
/// consumer can observe disconnect without polling.
fn reader_loop(
    reader_master: OwnedFd,
    output_tx: SyncSender<Vec<u8>>,
    shutdown: Arc<AtomicBool>,
    output_wake: Option<OutputWake>,
) {
    // Without nonblocking reads the thread could not observe shutdown while
    // idle; fail closed rather than risk an unjoinable thread.
    if set_nonblocking(&reader_master).is_err() {
        drop(output_tx);
        invoke_output_wake(output_wake.as_ref());
        return;
    }
    let mut chunk = vec![0u8; 8192];
    'outer: loop {
        if shutdown.load(Ordering::Acquire) {
            break;
        }
        let read = match rustix::io::read(&reader_master, &mut chunk[..]) {
            Ok(n) => n,
            Err(err) => match err.kind() {
                io::ErrorKind::Interrupted => continue,
                io::ErrorKind::WouldBlock => {
                    #[cfg(feature = "phase-timing")]
                    let _phase_guard = crate::phase::Guard::new("pty_poll_wait");
                    if !wait_until_ready(&reader_master, libc::POLLIN) {
                        break;
                    }
                    continue;
                }
                // EIO is macOS's "slave side closed" and any other error
                // means the master is gone: clean closure either way.
                _ => break,
            },
        };
        if read == 0 {
            // EOF: the slave side has closed.
            break;
        }
        let mut data = Vec::with_capacity(read);
        data.extend_from_slice(&chunk[..read]);
        loop {
            match output_tx.try_send(data) {
                Ok(()) => {
                    invoke_output_wake(output_wake.as_ref());
                    break;
                }
                Err(TrySendError::Full(pending)) => {
                    if shutdown.load(Ordering::Acquire) {
                        break 'outer;
                    }
                    data = pending;
                    #[cfg(feature = "phase-timing")]
                    {
                        let _g = crate::phase::Guard::new("pty_queue_full_wait");
                        thread::sleep(BACKPRESSURE_SLEEP);
                    }
                    #[cfg(not(feature = "phase-timing"))]
                    thread::sleep(BACKPRESSURE_SLEEP);
                }
                Err(TrySendError::Disconnected(_)) => {
                    // Consumer gone: keep draining and discard so the child
                    // never blocks on a full PTY buffer.
                    break;
                }
            }
        }
    }
    drop(output_tx);
    invoke_output_wake(output_wake.as_ref());
}

/// Drains the bounded writer queue into the master, handling short writes,
/// `EINTR`, and `EAGAIN`.
///
/// Each chunk is written with an advancing-offset loop (`write` may accept
/// fewer bytes than offered); `EINTR` retries immediately and `EAGAIN`
/// (nonblocking master with a full kernel buffer) sleeps one poll interval.
/// The shutdown flag is checked before every chunk and every write attempt,
/// so a wedged child (never reading) cannot hold up shutdown. Once the queue
/// sender is dropped the thread finishes the remaining buffered chunks and
/// exits on the disconnect.
fn writer_loop(writer_master: OwnedFd, writer_rx: Receiver<Vec<u8>>, shutdown: Arc<AtomicBool>) {
    // Without nonblocking writes a wedged child could block the thread
    // forever; fail closed rather than risk an unjoinable thread.
    if set_nonblocking(&writer_master).is_err() {
        return;
    }

    loop {
        if shutdown.load(Ordering::Acquire) {
            break;
        }
        let data = match writer_rx.recv() {
            Ok(data) => data,
            Err(RecvError) => break, // sender dropped: queue closed.
        };
        let mut offset = 0;
        while offset < data.len() {
            if shutdown.load(Ordering::Acquire) {
                // Terminate semantics: remaining queued input is abandoned.
                break;
            }
            match rustix::io::write(&writer_master, &data[offset..]) {
                Ok(n) => offset += n,
                Err(err) => match err.kind() {
                    io::ErrorKind::Interrupted => continue,
                    io::ErrorKind::WouldBlock => {
                        if !wait_until_ready(&writer_master, libc::POLLOUT) {
                            return;
                        }
                        continue;
                    }
                    // Master closed (e.g. shutdown already tore it down):
                    // nothing more to write.
                    _ => return,
                },
            }
        }
    }
}

/// Wait until a nonblocking PTY descriptor may make progress.
///
/// A timeout is treated as a successful wake so the owning loop can observe
/// shutdown. Permanent poll errors stop the worker; `EINTR` retries.
fn wait_until_ready(fd: &OwnedFd, events: libc::c_short) -> bool {
    let mut poll_fd = libc::pollfd {
        fd: fd.as_raw_fd(),
        events,
        revents: 0,
    };
    loop {
        // SAFETY: `poll_fd` is one initialized stack value, its pointer and
        // count agree, and `fd` remains owned for the duration of the call.
        let result = unsafe { libc::poll(&mut poll_fd, 1, IO_POLL_TIMEOUT_MS) };
        if result >= 0 {
            return true;
        }
        if io::Error::last_os_error().kind() != io::ErrorKind::Interrupted {
            return false;
        }
    }
}

/// Puts `fd` into nonblocking mode (`O_NONBLOCK`), preserving other flags.
///
/// # Safety invariants
///
/// Both `fcntl` calls are `unsafe`:
/// - `fd` is a valid open file descriptor for the lifetime of the call: it is
///   a clone of the PTY master owned exclusively by this thread and not
///   closed anywhere else (the session only closes its own handle).
/// - `F_GETFL` takes no extra argument and `F_SETFL` takes exactly one
///   `c_int` (`flags | O_NONBLOCK`), which is the only flag bit we set; no
///   pointers are involved, so no other memory is touched.
/// - Concurrent `read`/`write` on this descriptor from other threads is
///   impossible by construction (this clone is moved into exactly one
///   thread), so toggling the flag cannot race an I/O operation on the same
///   descriptor.
fn set_nonblocking(fd: &OwnedFd) -> io::Result<()> {
    // SAFETY: see the invariants above; `fd` is a live owned descriptor and
    // `F_GETFL` takes no argument.
    let flags = unsafe { libc::fcntl(fd.as_raw_fd(), F_GETFL) };
    if flags < 0 {
        return Err(io::Error::last_os_error());
    }
    if flags & O_NONBLOCK != 0 {
        return Ok(());
    }
    // SAFETY: see the invariants above; `F_SETFL` takes one `c_int`
    // argument, which is the only thing we pass.
    let result = unsafe { libc::fcntl(fd.as_raw_fd(), F_SETFL, flags | O_NONBLOCK) };
    if result < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

/// Best-effort child teardown for the `spawn` failure path: `SIGKILL` the
/// process group, then wait for the reaping owner to finish.
///
/// If the exit waiter thread exists it is joined (it reaps within one poll
/// interval of the child's death). Otherwise the child is reaped directly
/// with a bounded nonblocking poll.
fn abort_child(child_pid: i32, exit_handle: Option<JoinHandle<()>>) {
    let _ = crate::platform::kill_pgid(child_pid, SIGKILL);
    if let Some(handle) = exit_handle {
        let _ = handle.join();
    } else {
        for _ in 0..100 {
            if crate::platform::waitpid_nonblock(child_pid).is_some() {
                return;
            }
            thread::sleep(Duration::from_millis(5));
        }
    }
}

/// Invoke a configured wake without carrying payload or blocking the caller.
fn invoke_output_wake(output_wake: Option<&OutputWake>) {
    if let Some(wake) = output_wake {
        wake();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use parking_lot::Mutex;
    use std::os::unix::net::UnixStream;
    use std::sync::atomic::AtomicUsize;
    use std::sync::mpsc::sync_channel;

    fn counting_wake() -> (OutputWake, Arc<AtomicUsize>) {
        let count = Arc::new(AtomicUsize::new(0));
        let wake_count = Arc::clone(&count);
        let wake: OutputWake = Arc::new(move || {
            wake_count.fetch_add(1, Ordering::SeqCst);
        });
        (wake, count)
    }

    fn pipe_pair() -> (OwnedFd, OwnedFd) {
        let (reader, writer) = UnixStream::pair().expect("socketpair");
        (OwnedFd::from(reader), OwnedFd::from(writer))
    }

    #[test]
    fn enqueue_wake_fires_after_chunk_is_queued() {
        let (reader, writer) = pipe_pair();
        let (output_tx, output_rx) = sync_channel(4);
        let output_rx = Arc::new(Mutex::new(output_rx));
        let queued = Arc::new(AtomicBool::new(false));
        let saw_queued = Arc::clone(&queued);
        let wake_output_rx = Arc::clone(&output_rx);
        let wake: OutputWake = Arc::new(move || {
            if saw_queued.load(Ordering::SeqCst) {
                return;
            }
            assert!(
                wake_output_rx.lock().try_recv().is_ok(),
                "enqueue wake must observe the queued chunk"
            );
            saw_queued.store(true, Ordering::SeqCst);
        });

        rustix::io::write(&writer, b"abc").expect("write pipe");
        drop(writer);
        reader_loop(
            reader,
            output_tx,
            Arc::new(AtomicBool::new(false)),
            Some(wake),
        );
        assert!(queued.load(Ordering::SeqCst));
    }

    #[test]
    fn reader_termination_wakes_once_on_eof() {
        let (reader, writer) = pipe_pair();
        let (output_tx, output_rx) = sync_channel(1);
        let (wake, count) = counting_wake();
        drop(writer);

        reader_loop(
            reader,
            output_tx,
            Arc::new(AtomicBool::new(false)),
            Some(wake),
        );

        assert_eq!(count.load(Ordering::SeqCst), 1);
        assert!(
            matches!(
                output_rx.try_recv(),
                Err(std::sync::mpsc::TryRecvError::Disconnected)
            ),
            "termination wake must drop the output sender first"
        );
    }

    #[test]
    fn reader_termination_wakes_after_consumer_disconnect() {
        let (reader, writer) = pipe_pair();
        let (output_tx, output_rx) = sync_channel(1);
        let (wake, count) = counting_wake();
        drop(output_rx);
        rustix::io::write(&writer, b"x").expect("write pipe");
        drop(writer);

        reader_loop(
            reader,
            output_tx,
            Arc::new(AtomicBool::new(false)),
            Some(wake),
        );

        assert_eq!(count.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn exit_publication_wakes_after_status_is_queued() {
        let wait_guard = Arc::new(OnceLock::new());
        wait_guard.set(ExitStatus::exited(7)).unwrap();
        let (exit_tx, exit_rx) = sync_channel(1);
        let exit_rx = Arc::new(Mutex::new(exit_rx));
        let published = Arc::new(AtomicBool::new(false));
        let saw_status = Arc::clone(&published);
        let wake_exit_rx = Arc::clone(&exit_rx);
        let wake: OutputWake = Arc::new(move || {
            assert_eq!(
                wake_exit_rx.lock().try_recv().ok(),
                Some(ExitStatus::exited(7)),
                "exit wake must observe the published status"
            );
            saw_status.store(true, Ordering::SeqCst);
        });

        exit_waiter(-1, exit_tx, wait_guard, Some(wake));
        assert!(published.load(Ordering::SeqCst));
    }

    #[test]
    fn exit_waiter_sets_guard_before_wake_and_channel() {
        let wait_guard = Arc::new(OnceLock::new());
        wait_guard.set(ExitStatus::exited(42)).unwrap();
        let (exit_tx, exit_rx) = sync_channel(1);
        let exit_rx = Arc::new(Mutex::new(exit_rx));
        let guard_seen = Arc::new(AtomicBool::new(false));
        let saw_guard = Arc::clone(&guard_seen);
        let channel_seen = Arc::new(AtomicBool::new(false));
        let saw_channel = Arc::clone(&channel_seen);
        let guard_for_wake = Arc::clone(&wait_guard);
        let exit_rx_for_wake = Arc::clone(&exit_rx);
        let wake: OutputWake = Arc::new(move || {
            // OnceLock must be set before the channel is readable and before
            // the wake fires — otherwise try_wait/shutdown would see None.
            assert_eq!(
                guard_for_wake.get().copied(),
                Some(ExitStatus::exited(42)),
                "wake must observe OnceLock already set"
            );
            saw_guard.store(true, Ordering::SeqCst);
            assert_eq!(
                exit_rx_for_wake.lock().try_recv().ok(),
                Some(ExitStatus::exited(42)),
                "wake must observe channel already queued"
            );
            saw_channel.store(true, Ordering::SeqCst);
        });
        exit_waiter(-1, exit_tx, wait_guard, Some(wake));
        assert!(guard_seen.load(Ordering::SeqCst));
        assert!(channel_seen.load(Ordering::SeqCst));
    }

    #[test]
    fn exit_waiter_fake_preset_guard_does_not_block() {
        let wait_guard = Arc::new(OnceLock::new());
        wait_guard.set(ExitStatus::exited(9)).unwrap();
        let (exit_tx, exit_rx) = sync_channel(1);
        let started = std::time::Instant::now();
        exit_waiter(-1, exit_tx, Arc::clone(&wait_guard), None);
        assert!(
            started.elapsed() < Duration::from_millis(50),
            "fake pre-set guard must not block in waitpid"
        );
        assert_eq!(exit_rx.try_recv().ok(), Some(ExitStatus::exited(9)));
        assert_eq!(wait_guard.get().copied(), Some(ExitStatus::exited(9)));
    }
}

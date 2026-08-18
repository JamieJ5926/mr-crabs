//! macOS PTY and child-process setup.
//!
//! This module owns the low-level platform work of the crate:
//!
//! - creating the PTY pair with [`rustix::pty`] (`openpt`/`grantpt`/
//!   `unlockpt`/`ptsname`),
//! - applying the initial window size with [`rustix::termios::tcsetwinsize`],
//! - `fork`/`exec` of the child as a session and process-group leader whose
//!   controlling terminal is the PTY slave (via `setsid` + `TIOCSCTTY`),
//! - nonblocking reaping ([`waitpid_nonblock`]) and process-group signalling
//!   ([`kill_pgid`]) helpers used by the session lifecycle.
//!
//! # Fork-safety
//!
//! All `argv`/`envp`/`cwd` `CString` construction happens in the parent
//! before `fork`. The child path ([`child_exec`]) runs only async-signal-safe
//! libc calls and either `execve`s successfully or terminates the child image
//! with `_exit(127)`; it never allocates, never runs Rust destructors, and
//! cannot return into the parent's control flow.

use std::collections::BTreeMap;
use std::ffi::CString;
use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd};
use std::os::unix::ffi::OsStrExt;

use libc::{c_char, c_int, pid_t};
use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};

use crate::PtySize;
use crate::command::SpawnCommand;
use crate::error::PtyError;
use crate::session::ExitStatus;

/// Exit code used by the post-fork child when any setup step fails before
/// `execve`. Mirrors the shell convention for "command not found/exec failed".
const CHILD_SETUP_FAILURE: c_int = 127;

/// Open a PTY pair, apply `size` to the master, and spawn `command` as a new
/// session/process-group leader with the slave as its controlling terminal.
///
/// Returns the master side (parent-owned; the slave is closed in the parent)
/// and the child's pid. The child inherits the master fd but it has
/// `O_CLOEXEC` and is explicitly closed before `execve`, so it never leaks
/// into the spawned program.
pub(crate) fn spawn_pty_child(
    size: PtySize,
    command: &SpawnCommand,
) -> Result<(OwnedFd, pid_t), PtyError> {
    // Open the master side of a new PTY. O_NOCTTY keeps the master from ever
    // becoming a controlling terminal; O_CLOEXEC prevents it leaking across
    // the child's execve even if the explicit close below were skipped.
    let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY)
        .map_err(|err| PtyError::Spawn(err.into()))?;
    // rustix-openpty 0.2 does not expose an openpt CLOEXEC flag.
    // SAFETY: `master` owns a live descriptor; F_SETFD changes only its
    // descriptor flags and does not transfer or duplicate ownership.
    let cloexec_result =
        unsafe { libc::fcntl(master.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) };
    if cloexec_result == -1 {
        return Err(PtyError::Spawn(io::Error::last_os_error()));
    }

    // Make the slave accessible and unlock the pair for opening. Note:
    // rustix documents grantpt as having unspecified behavior when a SIGCHLD
    // handler is installed; this crate never installs one.
    grantpt(&master).map_err(|err| PtyError::Spawn(err.into()))?;
    unlockpt(&master).map_err(|err| PtyError::Spawn(err.into()))?;
    let slave_path = ptsname(&master, Vec::new()).map_err(|err| PtyError::Spawn(err.into()))?;

    // Open the slave side. O_NOCTTY is required here as well: the slave must
    // not become a controlling terminal until the child explicitly claims it
    // with TIOCSCTTY after setsid.
    //
    // SAFETY: `slave_path` is a NUL-terminated C string produced by ptsname
    // and remains alive for the duration of the call; the returned fd (when
    // >= 0) is a fresh descriptor owned by this process.
    let slave_raw = unsafe {
        libc::open(
            slave_path.as_ptr(),
            libc::O_RDWR | libc::O_NOCTTY | libc::O_CLOEXEC,
        )
    };
    if slave_raw < 0 {
        return Err(PtyError::Spawn(io::Error::last_os_error()));
    }
    // SAFETY: `slave_raw` is a valid open fd (checked >= 0 above); OwnedFd
    // takes exclusive ownership and closes it exactly once when dropped, so
    // the descriptor can never be double-closed or leaked.
    let slave = unsafe { OwnedFd::from_raw_fd(slave_raw) };

    // Apply the initial terminal size on the master (the authoritative side
    // of the pty pair; the slave inherits the geometry).
    set_winsize(&master, size).map_err(PtyError::Resize)?;

    // Build argv[0..] from the executable and its arguments. All NUL bytes
    // are rejected up front (CString::new -> NulError -> io::Error), so the
    // pointer arrays below are guaranteed NUL-terminated.
    let program =
        cstring_from_bytes(command.exe.as_os_str().as_bytes()).map_err(PtyError::Spawn)?;
    let mut argv: Vec<CString> = Vec::with_capacity(command.args.len() + 1);
    // argv[0] conventionally names the program as invoked.
    argv.push(program.clone());
    for arg in &command.args {
        argv.push(cstring_from_bytes(arg.as_bytes()).map_err(PtyError::Spawn)?);
    }
    let envp: Vec<CString> = build_envp(&command.envs).map_err(PtyError::Spawn)?;
    let cwd: Option<CString> = command
        .cwd
        .as_ref()
        .map(|dir| cstring_from_bytes(dir.as_os_str().as_bytes()).map_err(PtyError::Spawn))
        .transpose()?;

    // Materialize the pointer arrays now, before fork, so the child needs no
    // allocation or iteration to hand them to execve. Each array gets a
    // trailing null pointer: execve requires NULL-terminated argv/envp
    // arrays, not merely NUL-terminated strings. The CStrings and these
    // vectors live in the parent frame; after fork the child's copy of that
    // memory (copy-on-write) stays valid until execve.
    let mut argv_ptrs: Vec<*const c_char> =
        argv.iter().map(|value| value.as_c_str().as_ptr()).collect();
    argv_ptrs.push(std::ptr::null());
    let mut envp_ptrs: Vec<*const c_char> =
        envp.iter().map(|value| value.as_c_str().as_ptr()).collect();
    envp_ptrs.push(std::ptr::null());
    let program_ptr = program.as_ptr();
    let cwd_ptr = cwd.as_ref().map(|value| value.as_c_str().as_ptr());
    let master_raw = master.as_raw_fd();
    let slave_raw = slave.as_raw_fd();

    // A close-on-exec pipe lets the child report the errno from chdir/execve
    // (or earlier setup) before spawn returns. EOF means execve succeeded.
    let (exec_error_read, exec_error_write) = exec_error_pipe().map_err(PtyError::Spawn)?;
    let exec_error_read_raw = exec_error_read.as_raw_fd();
    let exec_error_write_raw = exec_error_write.as_raw_fd();

    // Fork the child. The child branch never returns: it either execs
    // successfully or _exit(127)s.
    //
    // SAFETY: fork duplicates this process image. All pre-fork state is
    // plain data; the child path (child_exec) runs only async-signal-safe
    // libc calls and never touches Rust allocator state, so no locks held by
    // other threads can be left in an inconsistent state in the child.
    let pid = unsafe { libc::fork() };
    if pid < 0 {
        return Err(PtyError::Spawn(io::Error::last_os_error()));
    }

    if pid == 0 {
        // SAFETY: the raw fds and pointers passed below are the ones
        // constructed and validated above; see child_exec for the full
        // invariant list. This call diverges (execve or _exit).
        unsafe {
            libc::close(exec_error_read_raw);
            child_exec(
                master_raw,
                slave_raw,
                program_ptr,
                &argv_ptrs,
                &envp_ptrs,
                cwd_ptr,
                exec_error_write_raw,
            )
        }
    }

    // Parent: close its copy of the write end, then wait for either an errno
    // payload or close-on-exec EOF. This makes chdir/execve failures
    // synchronous while allowing successful commands to continue normally.
    drop(exec_error_write);
    drop(slave);
    match read_exec_error(&exec_error_read) {
        Ok(None) => Ok((master, pid)),
        Ok(Some(errno)) => {
            reap_child_blocking(pid);
            Err(PtyError::Spawn(io::Error::from_raw_os_error(errno)))
        }
        Err(err) => {
            // The handshake itself failed. Terminate the child directly (it
            // may not have reached setsid yet), then reap before returning.
            unsafe { libc::kill(pid, libc::SIGKILL) };
            reap_child_blocking(pid);
            Err(PtyError::Spawn(err))
        }
    }
}

/// Create a pipe whose write end closes automatically on successful exec.
fn exec_error_pipe() -> io::Result<(OwnedFd, OwnedFd)> {
    let mut fds = [-1; 2];
    // SAFETY: `fds` provides space for exactly two descriptors.
    if unsafe { libc::pipe(fds.as_mut_ptr()) } == -1 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: pipe returned two fresh descriptors, each transferred once.
    let read = unsafe { OwnedFd::from_raw_fd(fds[0]) };
    let write = unsafe { OwnedFd::from_raw_fd(fds[1]) };
    for fd in [&read, &write] {
        // SAFETY: fd is live and F_SETFD changes only descriptor flags.
        if unsafe { libc::fcntl(fd.as_raw_fd(), libc::F_SETFD, libc::FD_CLOEXEC) } == -1 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok((read, write))
}

/// Read the child's errno report. EOF before any bytes means exec succeeded.
fn read_exec_error(fd: &OwnedFd) -> io::Result<Option<c_int>> {
    let mut bytes = [0_u8; std::mem::size_of::<c_int>()];
    let mut offset = 0;
    loop {
        // SAFETY: the remaining slice is valid writable memory for read.
        let read = unsafe {
            libc::read(
                fd.as_raw_fd(),
                bytes[offset..].as_mut_ptr().cast(),
                bytes.len() - offset,
            )
        };
        if read == 0 {
            return if offset == 0 {
                Ok(None)
            } else {
                Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "partial PTY child exec error report",
                ))
            };
        }
        if read < 0 {
            let err = io::Error::last_os_error();
            if err.kind() == io::ErrorKind::Interrupted {
                continue;
            }
            return Err(err);
        }
        offset += read as usize;
        if offset == bytes.len() {
            return Ok(Some(c_int::from_ne_bytes(bytes)));
        }
    }
}

fn reap_child_blocking(pid: pid_t) {
    loop {
        // SAFETY: pid names the child created by this function; a null status
        // pointer is permitted when only reaping is required.
        let result = unsafe { libc::waitpid(pid, std::ptr::null_mut(), 0) };
        if result == pid {
            return;
        }
        if result == -1 && io::Error::last_os_error().kind() == io::ErrorKind::Interrupted {
            continue;
        }
        return;
    }
}

/// Report the current errno to the parent and terminate without destructors.
unsafe fn report_exec_error_and_exit(error_fd: c_int) -> ! {
    // SAFETY: macOS exposes thread-local errno through __error. No other libc
    // call occurs between the failing operation and this load.
    let errno = unsafe { *libc::__error() };
    let bytes = errno.to_ne_bytes();
    let mut offset = 0;
    while offset < bytes.len() {
        // SAFETY: error_fd is the live pipe write end and the byte slice is
        // valid. write and _exit are async-signal-safe after fork.
        let written = unsafe {
            libc::write(
                error_fd,
                bytes[offset..].as_ptr().cast(),
                bytes.len() - offset,
            )
        };
        if written > 0 {
            offset += written as usize;
        } else if written == -1 && unsafe { *libc::__error() } == libc::EINTR {
            continue;
        } else {
            break;
        }
    }
    unsafe { libc::_exit(CHILD_SETUP_FAILURE) }
}

/// Post-fork child routine: make the child a session/process-group leader,
/// claim the PTY slave as its controlling terminal, wire stdio to the slave,
/// apply the working directory, and exec.
///
/// Runs only async-signal-safe libc calls and terminates the child image on
/// any failure; never returns.
///
/// # Safety
///
/// - `master_fd` and `slave_fd` are the raw descriptor numbers opened in
///   [`spawn_pty_child`]; they are valid in this (the child's) process image
///   and are not closed by any other code path before these calls.
/// - `program_ptr` points to a NUL-terminated C string that remains valid in
///   the child's copy of the parent's memory.
/// - `cwd_ptr`, when `Some`, points to a NUL-terminated C string with the
///   same lifetime guarantee.
/// - `argv_ptrs`/`envp_ptrs` are NULL-terminated arrays of pointers to
///   NUL-terminated C strings (a trailing null pointer was pushed by the
///   caller), and both the arrays and the strings remain valid in the
///   child's memory image.
/// - `exec_error_fd` is the live close-on-exec pipe write end inherited from
///   the parent and is used only to report a setup errno before `_exit`.
unsafe fn child_exec(
    master_fd: c_int,
    slave_fd: c_int,
    program_ptr: *const c_char,
    argv_ptrs: &[*const c_char],
    envp_ptrs: &[*const c_char],
    cwd_ptr: Option<*const c_char>,
    exec_error_fd: c_int,
) -> ! {
    // Become a new session leader and process-group leader. The forked child
    // is not a process-group leader (its pid differs from its pgid, which is
    // the parent's pid), so setsid cannot fail with EPERM here.
    //
    // SAFETY: setsid is async-signal-safe and requires no arguments; its
    // failure is detected below and handled by terminating the child image.
    if unsafe { libc::setsid() } == -1 {
        // SAFETY: `_exit` terminates this (the child) process image at once
        // and never returns, skipping atexit handlers and Rust destructors
        // as required after fork.
        unsafe { report_exec_error_and_exit(exec_error_fd) }
    }

    // Claim the slave as the controlling terminal. TIOCSCTTY with a zero
    // argument refuses to steal a terminal already owned by another session;
    // since we just created a fresh session via setsid, the slave is unowned
    // and the ioctl succeeds.
    //
    // SAFETY: `slave_fd` is a valid open descriptor in this image and
    // TIOCSCTTY is the standard tty-control request; the variadic argument 0
    // (c_int) is the conventional "do not steal" flag on macOS.
    if unsafe { libc::ioctl(slave_fd, libc::TIOCSCTTY as libc::c_ulong, 0) } == -1 {
        // SAFETY: `_exit` terminates this process image without running
        // destructors; required in the post-fork child (see above).
        unsafe { report_exec_error_and_exit(exec_error_fd) }
    }

    // Wire the slave to stdin/stdout/stderr. dup2 atomically replaces each
    // standard descriptor and clears CLOEXEC on it, so the stdio fds survive
    // the execve below.
    //
    // SAFETY: all three dup2 calls reference the valid `slave_fd` and the
    // constant standard descriptors 0/1/2; dup2 is async-signal-safe, and
    // each target descriptor is exactly the one execve expects for stdio.
    if unsafe {
        libc::dup2(slave_fd, libc::STDIN_FILENO) == -1
            || libc::dup2(slave_fd, libc::STDOUT_FILENO) == -1
            || libc::dup2(slave_fd, libc::STDERR_FILENO) == -1
    } {
        // SAFETY: `_exit` terminates this process image without running
        // destructors; required in the post-fork child (see above).
        unsafe { report_exec_error_and_exit(exec_error_fd) }
    }

    // Drop the original slave descriptor and the inherited master. The
    // master's O_CLOEXEC is a second line of defense, but closing it here
    // also prevents the child from holding the PTY open across the setup
    // window (which would otherwise keep the master readable after exec).
    //
    // SAFETY: `slave_fd` is a descriptor opened by this module's parent path
    // and duplicated onto 0/1/2 above; it is >= 3 (PTY fds are allocated
    // after stdio), so closing it cannot touch stdio. `master_fd` is valid
    // and inherited; both fds are closed exactly once per process image.
    unsafe {
        libc::close(slave_fd);
        libc::close(master_fd);
    }

    // Apply the requested working directory, if any.
    if let Some(cwd) = cwd_ptr {
        // SAFETY: `cwd` points to a NUL-terminated path string that remains
        // valid in this process image (built before fork, see call site).
        if unsafe { libc::chdir(cwd) } == -1 {
            // SAFETY: `_exit` terminates this process image without running
            // destructors; required in the post-fork child (see above).
            unsafe { report_exec_error_and_exit(exec_error_fd) }
        }
    }

    // Replace the child image with the command. execve returns only on
    // failure, and only with the error in errno; any failure means the child
    // cannot run, so terminate with the conventional 127.
    //
    // SAFETY: `program_ptr` is a NUL-terminated executable path; `argv_ptrs`
    // and `envp_ptrs` are NUL-terminated pointer arrays to NUL-terminated
    // C strings (all constructed before fork and still valid here); execve
    // is async-signal-safe and does not return on success.
    unsafe {
        libc::execve(program_ptr, argv_ptrs.as_ptr(), envp_ptrs.as_ptr());
    }
    // SAFETY: execve returned, so it failed. Report its errno synchronously
    // and terminate without running destructors.
    unsafe { report_exec_error_and_exit(exec_error_fd) }
}

/// Apply `size` to the PTY master.
pub(crate) fn set_winsize(fd: &OwnedFd, size: PtySize) -> io::Result<()> {
    rustix::termios::tcsetwinsize(fd, size.to_winsize()).map_err(io::Error::from)
}

/// Nonblocking reaping helper: returns `Some(ExitStatus)` once the child has
/// been reaped, or `None` while it is still running (or when no status is
/// available, e.g. `ECHILD` after an earlier reap).
///
/// The session lifecycle pairs this with a reaped-flag guard so `waitpid` is
/// issued exactly once per child; `ECHILD` from a second call is therefore
/// treated as "no status" rather than an error.
pub(crate) fn waitpid_nonblock(pid: pid_t) -> Option<ExitStatus> {
    let mut raw_status: c_int = 0;
    loop {
        // SAFETY: `pid` is a live child of this process (the session guard
        // prevents a second reap), `raw_status` points to valid writable
        // memory of the correct size, and WNOHANG makes the wait nonblocking.
        let ret = unsafe { libc::waitpid(pid, &mut raw_status, libc::WNOHANG) };
        if ret == pid {
            return Some(exit_status_from_raw(raw_status));
        }
        if ret == 0 {
            // Child still running.
            return None;
        }
        let err = io::Error::last_os_error();
        if err.kind() == io::ErrorKind::Interrupted {
            // EINTR: the wait was interrupted by a signal; retry.
            continue;
        }
        // ECHILD (already reaped / no such child) or any other error: no
        // status is available right now.
        return None;
    }
}

/// Decode a raw `waitpid` status into the public exit representation,
/// preferring the exit code when the child exited normally and the signal
/// number when it was killed by a signal.
fn exit_status_from_raw(raw_status: c_int) -> ExitStatus {
    if libc::WIFEXITED(raw_status) {
        ExitStatus {
            code: Some(libc::WEXITSTATUS(raw_status)),
            signal: None,
        }
    } else if libc::WIFSIGNALED(raw_status) {
        ExitStatus {
            code: None,
            signal: Some(libc::WTERMSIG(raw_status)),
        }
    } else {
        ExitStatus {
            code: None,
            signal: None,
        }
    }
}

/// Send `sig` to the process group whose id is `pid` (the child's pgid, since
/// the child is its own process-group leader).
///
/// Refuses non-positive pids: `kill(-0, …)` would target the caller's own
/// process group, which must never happen from a supervisor path.
pub(crate) fn kill_pgid(pid: pid_t, sig: c_int) -> io::Result<()> {
    if pid <= 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "kill_pgid requires a positive child pid; negative would signal the caller's own process group",
        ));
    }
    // SAFETY: `-pid` with `pid > 0` (checked above) addresses exactly the
    // child's process group; `sig` is a valid signal number chosen by the
    // caller (SIGHUP/SIGTERM/SIGKILL). A child that already exited yields
    // ESRCH, surfaced as an io::Error.
    if unsafe { libc::kill(-pid, sig) } == -1 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// Convert a byte slice into a NUL-terminated C string, mapping an embedded
/// NUL byte to an `InvalidInput` I/O error.
fn cstring_from_bytes(bytes: &[u8]) -> Result<CString, io::Error> {
    CString::new(bytes).map_err(Into::into)
}

/// Serialize the deterministic environment overlay into `KEY=VALUE`
/// C strings. `BTreeMap` iteration order is sorted, so the resulting `envp`
/// is byte-for-byte deterministic across runs. Environment variable names
/// containing `=` are rejected (they would be ambiguous to parse); embedded
/// NUL bytes are rejected by `CString`.
fn build_envp(envs: &BTreeMap<String, String>) -> Result<Vec<CString>, io::Error> {
    let mut envp = Vec::with_capacity(envs.len());
    for (key, value) in envs {
        if key.contains('=') {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "environment variable names must not contain '='",
            ));
        }
        let mut entry = Vec::with_capacity(key.len() + value.len() + 1);
        entry.extend_from_slice(key.as_bytes());
        entry.push(b'=');
        entry.extend_from_slice(value.as_bytes());
        envp.push(cstring_from_bytes(&entry)?);
    }
    Ok(envp)
}

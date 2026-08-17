//! Bounded FIFO byte-delivery primitives for the PTY session.
//!
//! The session ships bytes over two bounded [`std::sync::mpsc::sync_channel`]
//! queues:
//!
//! * **Writer queue** — application → writer thread → master fd. Bound is
//!   [`WRITER_QUEUE_CAPACITY`].
//! * **Reader queue** — reader thread → application. Bound is
//!   [`READER_QUEUE_CAPACITY`].
//!
//! # Bounded invariant and backpressure
//!
//! Both channels are *always* constructed via [`sync_channel`] with an
//! explicit capacity; no unbounded or async channel type is used anywhere in
//! this module (or the PTY session). A bounded queue makes a slow consumer
//! apply backpressure on the producer instead of growing memory without
//! bound:
//!
//! * `try_send` reports [`QueueError::Full`] immediately when the queue is at
//!   capacity, so the caller can drop data, retry, or block.
//! * `send_timeout` blocks while full, polling at a fixed interval, and
//!   reports [`QueueError::Timeout`] once the caller's deadline elapses.
//!
//! Byte order is preserved: each write is enqueued as a single
//! `Vec<u8>` item and `sync_channel` is strictly FIFO, so chunks leave the
//! queue in exactly the order they were enqueued.

use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::sync::mpsc::{Receiver, SendError, SyncSender, TrySendError};
use std::thread;
use std::time::{Duration, Instant};

use crate::WriteError;

/// Maximum number of writes buffered between the session API and the master
/// fd writer thread. Explicit bound so a blocked fd applies backpressure as
/// [`QueueError::Full`] / [`QueueError::Timeout`] instead of unbounded memory
/// growth.
pub const WRITER_QUEUE_CAPACITY: usize = 64;

/// Maximum number of read chunks buffered between the reader thread and the
/// session consumer. Bounded so a slow consumer backpressures the reader
/// thread instead of growing memory without bound.
pub const READER_QUEUE_CAPACITY: usize = 32;

/// Poll interval used by [`send_timeout`] while the queue is full.
const SEND_TIMEOUT_POLL: Duration = Duration::from_millis(1);

/// Error returned by the bounded-send helpers in this module.
///
/// Deliberately distinct from [`WriteError`]: [`QueueError`] describes the
/// queue transport itself, while [`WriteError`] is the session-facing public
/// error that also carries I/O failures. [`From<QueueError> for WriteError`]
/// and [`map_write_error`] bridge the two.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum QueueError {
    /// The bounded queue is at capacity; the caller must apply backpressure
    /// (retry, block, or drop).
    Full,
    /// The receiving end has been dropped; no further items can be delivered.
    Closed,
    /// The caller's deadline elapsed while the queue stayed full.
    Timeout,
}

impl Display for QueueError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Full => f.write_str("PTY queue is full"),
            Self::Closed => f.write_str("PTY queue is closed"),
            Self::Timeout => f.write_str("PTY queue timed out waiting for capacity"),
        }
    }
}

impl Error for QueueError {}

/// Create the bounded writer channel: session API → writer thread → master fd.
///
/// The returned sender is the session-side write handle; the receiver is
/// consumed by the writer thread.
pub fn writer_channel() -> (SyncSender<Vec<u8>>, Receiver<Vec<u8>>) {
    std::sync::mpsc::sync_channel(WRITER_QUEUE_CAPACITY)
}

/// Create the bounded reader channel: reader thread → session consumer.
///
/// The returned sender is held by the reader thread; the receiver is the
/// session's byte-output stream.
pub fn reader_channel() -> (SyncSender<Vec<u8>>, Receiver<Vec<u8>>) {
    std::sync::mpsc::sync_channel(READER_QUEUE_CAPACITY)
}

/// Nonblocking enqueue of one byte chunk.
///
/// Returns [`QueueError::Full`] if the queue is at capacity (caller applies
/// backpressure) or [`QueueError::Closed`] if the receiver has been dropped.
/// Byte order is preserved: the chunk is enqueued as a single FIFO item.
pub fn try_send(sender: &SyncSender<Vec<u8>>, data: Vec<u8>) -> Result<(), QueueError> {
    sender.try_send(data).map_err(map_try_send_error)
}

/// Blocking enqueue of one byte chunk, bounded by `timeout`.
///
/// While the queue is full, polls [`try_send`] every
/// [`SEND_TIMEOUT_POLL`] (1 ms) and returns [`QueueError::Timeout`] as soon
/// as `deadline` has passed, [`QueueError::Closed`] if the receiver is
/// dropped, or `Ok(())` once the chunk is accepted. A zero-length timeout
/// performs exactly one nonblocking attempt.
///
/// The chunk is re-enqueued on every retry, so the ordering guarantee is
/// identical to a single successful [`try_send`]: FIFO per chunk.
pub fn send_timeout(
    sender: &SyncSender<Vec<u8>>,
    data: Vec<u8>,
    timeout: Duration,
) -> Result<(), QueueError> {
    let deadline = Instant::now() + timeout;
    let mut pending = data;
    loop {
        match sender.try_send(pending) {
            Ok(()) => return Ok(()),
            Err(TrySendError::Full(back)) => {
                if Instant::now() >= deadline {
                    return Err(QueueError::Timeout);
                }
                thread::sleep(SEND_TIMEOUT_POLL);
                pending = back;
            }
            Err(TrySendError::Disconnected(_)) => return Err(QueueError::Closed),
        }
    }
}

/// Map a std `TrySendError` to the session-facing [`WriteError`].
///
/// `Full` maps to [`WriteError::Full`] and `Disconnected` (receiver dropped)
/// maps to [`WriteError::Closed`]; the payload is discarded.
pub fn map_write_error<T>(err: TrySendError<T>) -> WriteError {
    match err {
        TrySendError::Full(_) => WriteError::Full,
        TrySendError::Disconnected(_) => WriteError::Closed,
    }
}

/// Map a std `SendError` (receiver dropped before a blocking send completed)
/// to [`WriteError::Closed`].
pub fn map_send_error<T>(_err: SendError<T>) -> WriteError {
    WriteError::Closed
}

impl From<QueueError> for WriteError {
    fn from(err: QueueError) -> Self {
        match err {
            QueueError::Full => WriteError::Full,
            QueueError::Closed => WriteError::Closed,
            QueueError::Timeout => WriteError::Timeout,
        }
    }
}

fn map_try_send_error(err: TrySendError<Vec<u8>>) -> QueueError {
    match err {
        TrySendError::Full(_) => QueueError::Full,
        TrySendError::Disconnected(_) => QueueError::Closed,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::mpsc::{Receiver, SyncSender, sync_channel};

    fn fill_to_capacity(sender: &SyncSender<Vec<u8>>, capacity: usize) {
        for i in 0..capacity {
            sender
                .try_send(vec![i as u8])
                .expect("queue should accept up to its capacity");
        }
    }

    #[test]
    fn writer_queue_is_bounded_at_capacity() {
        let (sender, _receiver) = writer_channel();
        assert_eq!(WRITER_QUEUE_CAPACITY, 64);
        fill_to_capacity(&sender, WRITER_QUEUE_CAPACITY);
        assert_eq!(
            try_send(&sender, vec![0xAA]),
            Err(QueueError::Full),
            "65th chunk on a 64-capacity queue must report Full"
        );
    }

    #[test]
    fn reader_queue_is_bounded_at_capacity() {
        let (sender, _receiver) = reader_channel();
        assert_eq!(READER_QUEUE_CAPACITY, 32);
        fill_to_capacity(&sender, READER_QUEUE_CAPACITY);
        assert_eq!(
            try_send(&sender, vec![0xBB]),
            Err(QueueError::Full),
            "33rd chunk on a 32-capacity queue must report Full"
        );
    }

    #[test]
    fn try_send_reports_closed_once_receiver_dropped() {
        let (sender, receiver) = writer_channel();
        drop(receiver);
        assert_eq!(try_send(&sender, vec![1]), Err(QueueError::Closed));
    }

    #[test]
    fn send_timeout_zero_returns_timeout_when_full() {
        let (sender, _receiver) = writer_channel();
        fill_to_capacity(&sender, WRITER_QUEUE_CAPACITY);
        assert_eq!(
            send_timeout(&sender, vec![2], Duration::ZERO),
            Err(QueueError::Timeout),
            "full queue plus zero deadline must time out without sleeping"
        );
    }

    #[test]
    fn send_timeout_succeeds_once_room_appears() {
        let (sender, receiver) = writer_channel();
        fill_to_capacity(&sender, WRITER_QUEUE_CAPACITY - 1);
        let drainer = thread::spawn(move || {
            let _: Vec<u8> = receiver.recv().expect("drainer receives one chunk");
        });
        let result = send_timeout(&sender, vec![3], Duration::from_secs(5));
        assert_eq!(result, Ok(()));
        drainer.join().expect("drainer finishes");
    }

    #[test]
    fn send_timeout_reports_closed_while_waiting() {
        let (sender, receiver) = writer_channel();
        fill_to_capacity(&sender, WRITER_QUEUE_CAPACITY);
        drop(receiver);
        assert_eq!(
            send_timeout(&sender, vec![4], Duration::from_secs(5)),
            Err(QueueError::Closed),
            "receiver dropped while full must report Closed, not Timeout"
        );
    }

    #[test]
    fn byte_order_preserved_across_fifo() {
        let (sender, receiver) = writer_channel();
        for chunk in [b"one".to_vec(), b"two".to_vec(), b"three".to_vec()] {
            try_send(&sender, chunk).expect("small queue has room");
        }
        drop(sender);
        let chunks: Vec<Vec<u8>> = receiver.iter().collect();
        assert_eq!(
            chunks,
            vec![b"one".to_vec(), b"two".to_vec(), b"three".to_vec()],
            "chunks must leave the queue in enqueue order"
        );
        let joined: Vec<u8> = chunks.into_iter().flatten().collect();
        assert_eq!(joined, b"onetwothree");
    }

    #[test]
    fn map_write_error_translates_std_errors() {
        assert!(matches!(
            map_write_error::<Vec<u8>>(TrySendError::Full(vec![1])),
            WriteError::Full
        ));
        assert!(matches!(
            map_write_error::<Vec<u8>>(TrySendError::Disconnected(vec![2])),
            WriteError::Closed
        ));
        assert!(matches!(
            map_send_error::<Vec<u8>>(SendError(vec![3])),
            WriteError::Closed
        ));
    }

    #[test]
    fn queue_error_maps_to_write_error() {
        assert!(matches!(
            WriteError::from(QueueError::Full),
            WriteError::Full
        ));
        assert!(matches!(
            WriteError::from(QueueError::Closed),
            WriteError::Closed
        ));
        assert!(matches!(
            WriteError::from(QueueError::Timeout),
            WriteError::Timeout
        ));
    }

    #[test]
    fn queue_error_display_distinguishes_variants() {
        assert_eq!(QueueError::Full.to_string(), "PTY queue is full");
        assert_eq!(QueueError::Closed.to_string(), "PTY queue is closed");
        assert_eq!(
            QueueError::Timeout.to_string(),
            "PTY queue timed out waiting for capacity"
        );
    }

    #[test]
    fn helpers_accept_any_bounded_sender() {
        // The helpers are channel-shape agnostic: any bounded SyncSender works.
        let (sender, receiver): (SyncSender<Vec<u8>>, Receiver<Vec<u8>>) = sync_channel(2);
        assert!(try_send(&sender, vec![1]).is_ok());
        assert!(try_send(&sender, vec![2]).is_ok());
        assert_eq!(try_send(&sender, vec![3]), Err(QueueError::Full));
        drop(sender);
        assert_eq!(receiver.recv().unwrap(), vec![1]);
        assert_eq!(receiver.recv().unwrap(), vec![2]);
        assert!(receiver.recv().is_err());
    }
}

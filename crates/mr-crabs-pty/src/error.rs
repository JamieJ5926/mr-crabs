//! Error types for PTY session creation, I/O, resizing, and teardown.
//!
//! [`PtyError`] covers the full session lifecycle; [`WriteError`] covers the
//! bounded writer queue specifically. Both preserve `io::Error` sources and
//! distinguish queue-full / queue-closed / timeout without hiding the
//! underlying `io::Error`.

use std::error::Error;
use std::fmt::{self, Display, Formatter};
use std::io;

use crate::PtySizeError;

/// Error type for PTY creation, resize, write, and teardown operations.
#[derive(Debug)]
pub enum PtyError {
    /// A general I/O error from a PTY system call (open, reads, signals).
    Io(io::Error),
    /// The bounded writer queue is full; the write must be retried later.
    QueueFull,
    /// The writer queue has been closed; no further writes are possible.
    QueueClosed,
    /// A blocking write timed out before space became available.
    Timeout,
    /// The requested PTY size is invalid (zero rows or columns).
    InvalidSize(PtySizeError),
    /// The child process could not be spawned.
    Spawn(io::Error),
    /// Resizing the PTY failed.
    Resize(io::Error),
}

impl PtyError {
    /// Returns `true` if the bounded writer queue was full.
    pub fn is_full(&self) -> bool {
        matches!(self, Self::QueueFull)
    }

    /// Returns `true` if the writer queue has been closed.
    pub fn is_closed(&self) -> bool {
        matches!(self, Self::QueueClosed)
    }

    /// Returns `true` if a blocking write timed out.
    pub fn is_timeout(&self) -> bool {
        matches!(self, Self::Timeout)
    }
}

impl Display for PtyError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io(err) => write!(formatter, "PTY I/O error: {err}"),
            Self::QueueFull => formatter.write_str("PTY writer queue is full"),
            Self::QueueClosed => formatter.write_str("PTY writer queue is closed"),
            Self::Timeout => formatter.write_str("PTY write timed out"),
            Self::InvalidSize(err) => write!(formatter, "invalid PTY size: {err}"),
            Self::Spawn(err) => write!(formatter, "failed to spawn PTY child: {err}"),
            Self::Resize(err) => write!(formatter, "failed to resize PTY: {err}"),
        }
    }
}

impl Error for PtyError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(err) | Self::Spawn(err) | Self::Resize(err) => Some(err),
            Self::InvalidSize(err) => Some(err),
            _ => None,
        }
    }
}

impl From<io::Error> for PtyError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

impl From<PtySizeError> for PtyError {
    fn from(err: PtySizeError) -> Self {
        Self::InvalidSize(err)
    }
}

/// Error type for writes to the bounded writer queue.
///
/// [`WriteError::Full`], [`WriteError::Closed`], and [`WriteError::Timeout`]
/// are distinct variants (never collapsed into `io::Error`), while
/// [`WriteError::Io`] preserves the underlying I/O failure and its source.
#[derive(Debug)]
pub enum WriteError {
    /// The bounded writer queue is full; the write must be retried later.
    Full,
    /// The writer queue has been closed; no further writes are possible.
    Closed,
    /// A blocking write timed out before space became available.
    Timeout,
    /// The underlying write failed with an I/O error.
    Io(io::Error),
}

impl WriteError {
    /// Returns `true` if the bounded writer queue was full.
    pub fn is_full(&self) -> bool {
        matches!(self, Self::Full)
    }

    /// Returns `true` if the writer queue has been closed.
    pub fn is_closed(&self) -> bool {
        matches!(self, Self::Closed)
    }

    /// Returns `true` if a blocking write timed out.
    pub fn is_timeout(&self) -> bool {
        matches!(self, Self::Timeout)
    }
}

impl Display for WriteError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::Full => formatter.write_str("PTY writer queue is full"),
            Self::Closed => formatter.write_str("PTY writer queue is closed"),
            Self::Timeout => formatter.write_str("PTY write timed out"),
            Self::Io(err) => write!(formatter, "PTY write I/O error: {err}"),
        }
    }
}

impl Error for WriteError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Io(err) => Some(err),
            _ => None,
        }
    }
}

impl From<io::Error> for WriteError {
    fn from(err: io::Error) -> Self {
        Self::Io(err)
    }
}

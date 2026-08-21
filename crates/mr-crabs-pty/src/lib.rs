//! Pure Rust PTY and process-lifecycle management for mr-crabs.
//!
//! This crate spawns and supervises child processes attached to a pseudo
//! terminal, with an explicit platform boundary ([`platform`]) that is
//! currently implemented for macOS only. It deliberately does not import
//! Alacritty's `tty` or event-loop stack; all PTY plumbing uses `rustix`
//! (openpty/termios/process) and `libc` directly.
//!
//! Design invariants:
//!
//! - All writer traffic flows through one **bounded** queue
//!   ([`queue`]); no unbounded channels exist in production PTY code.
//! - Reader output is delivered in order and is bounded per chunk.
//! - Optional [`OutputWake`] callbacks notify after output enqueue, reader
//!   termination, and exit publication; they carry no payload.
//! - Resizes coalesce; identical dimensions are ignored.
//! - The child becomes session and process-group leader with the PTY as its
//!   controlling terminal; the parent retains only the master side.
//! - Every spawned child is terminated and reaped on explicit shutdown;
//!   [`PtySession::Drop`] performs a bounded best-effort terminate/reap
//!   without panicking.
//! - No `unsafe` without a local `SAFETY` comment naming every invariant.
//!
//! # Layout
//!
//! - [`PtySize`] / [`PtySizeError`]: validated terminal geometry and its
//!   conversion to [`rustix::termios::Winsize`] (defined here in `lib.rs`).
//! - [`error`]: [`PtyError`] and [`WriteError`], distinguishing queue
//!   full/closed/timeout while preserving `io::Error` sources.
//! - [`command`]: [`CommandBuilder`] producing a deterministic spawn command.
//! - [`queue`]: bounded writer/reader channels and their capacities.
//! - [`platform`]: the internal macOS platform boundary (`spawn_pty`).
//! - [`session`]: [`PtySession`], [`PtyConfig`], [`ExitStatus`], and
//!   [`OutputWake`].
//!
//! Peer modules (command/platform/session/queue) are implemented by their
//! owning lanes; this root only declares them and re-exports their public
//! types.

// Peer lanes may carry items without full documentation; keep the crate
// warning-free without requiring docs from every lane.
#![allow(missing_docs)]

use std::error::Error;
use std::fmt::{self, Display, Formatter};

/// Declare the deterministic command builder ([`CommandBuilder`]).
pub mod command;
/// Declare the error types ([`PtyError`], [`WriteError`]).
pub mod error;
/// Declare the macOS platform boundary.
pub mod platform;
/// Declare the bounded writer/reader channels and capacities.
pub mod queue;
/// Declare the optional phase-timing probes.
pub mod phase;
/// Declare the PTY session lifecycle ([`PtySession`], [`PtyConfig`],
/// [`ExitStatus`]).
pub mod session;

pub use command::CommandBuilder;
pub use error::{PtyError, WriteError};
pub use session::{ExitStatus, OutputWake, PtyConfig, PtySession};

/// Validated PTY terminal geometry.
///
/// Columns and rows must both be nonzero; cell dimensions may be zero (for
/// example when pixel metrics are unknown). `cell_width`/`cell_height` are
/// per-cell sizes; the winsize pixel totals derive from them as
/// `cols * cell_width` / `rows * cell_height` (see [`to_winsize`]).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PtySize {
    pub cols: u16,
    pub rows: u16,
    pub cell_width: u16,
    pub cell_height: u16,
}

impl PtySize {
    pub fn new(
        cols: u16,
        rows: u16,
        cell_width: u16,
        cell_height: u16,
    ) -> Result<Self, PtySizeError> {
        if cols == 0 {
            Err(PtySizeError::ZeroColumns)
        } else if rows == 0 {
            Err(PtySizeError::ZeroRows)
        } else {
            Ok(Self {
                cols,
                rows,
                cell_width,
                cell_height,
            })
        }
    }

    /// Convert this size to the `rustix` winsize record used by the PTY
    /// ioctls (`tcgetwinsize` / `tcsetwinsize`).
    ///
    /// `ws_xpixel`/`ws_ypixel` are **totals**: the full grid pixel size
    /// (`cols * cell_width`, `rows * cell_height`), saturated at `u16`
    /// instead of wrapping.
    pub fn to_winsize(&self) -> rustix::termios::Winsize {
        rustix::termios::Winsize {
            ws_row: self.rows,
            ws_col: self.cols,
            ws_xpixel: u16::saturating_mul(self.cols, self.cell_width),
            ws_ypixel: u16::saturating_mul(self.rows, self.cell_height),
        }
    }
}

impl From<PtySize> for rustix::termios::Winsize {
    fn from(size: PtySize) -> Self {
        size.to_winsize()
    }
}

impl TryFrom<rustix::termios::Winsize> for PtySize {
    type Error = PtySizeError;

    fn try_from(winsize: rustix::termios::Winsize) -> Result<Self, Self::Error> {
        Self::new(
            winsize.ws_col,
            winsize.ws_row,
            winsize.ws_xpixel,
            winsize.ws_ypixel,
        )
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PtySizeError {
    ZeroColumns,
    ZeroRows,
}

impl Display for PtySizeError {
    fn fmt(&self, formatter: &mut Formatter<'_>) -> fmt::Result {
        match self {
            Self::ZeroColumns => formatter.write_str("PTY size must have at least one column"),
            Self::ZeroRows => formatter.write_str("PTY size must have at least one row"),
        }
    }
}

impl Error for PtySizeError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pty_winsize_pixels_are_totals() {
        let size = PtySize::new(80, 24, 8, 17).expect("valid size");
        let winsize = size.to_winsize();
        assert_eq!(winsize.ws_col, 80);
        assert_eq!(winsize.ws_row, 24);
        // Totals, not per-cell sizes: 80*8, 24*17.
        assert_eq!(winsize.ws_xpixel, 640);
        assert_eq!(winsize.ws_ypixel, 408);
        assert_eq!(rustix::termios::Winsize::from(size), winsize);
    }

    #[test]
    fn pty_winsize_pixels_saturate() {
        let size = PtySize::new(65535, 65535, 65535, 65535).expect("valid size");
        let winsize = size.to_winsize();
        assert_eq!(winsize.ws_xpixel, u16::MAX);
        assert_eq!(winsize.ws_ypixel, u16::MAX);
    }

    #[test]
    fn pty_winsize_unknown_cells_have_zero_totals() {
        let size = PtySize::new(80, 24, 0, 0).expect("valid size");
        let winsize = size.to_winsize();
        assert_eq!(winsize.ws_col, 80);
        assert_eq!(winsize.ws_row, 24);
        assert_eq!(winsize.ws_xpixel, 0);
        assert_eq!(winsize.ws_ypixel, 0);
    }
}

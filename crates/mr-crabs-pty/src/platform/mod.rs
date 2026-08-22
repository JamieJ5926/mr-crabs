//! Internal platform boundary for PTY spawning and child-process control.
//!
//! The crate currently implements macOS only. Any other target is a hard
//! compile error rather than a fake or stubbed implementation, so behavior
//! can never silently diverge on an unverified platform.
//! The macOS implementation ([`macos`]) exposes the low-level primitives the
//! session lifecycle composes:
//!
//! - [`spawn_pty_child`]: open a PTY pair, apply the initial size, and
//!   `fork`/`exec` a child that is its own session and process-group leader
//!   with the slave as its controlling terminal; the parent keeps only the
//!   master.
//! - [`set_winsize`]: apply terminal geometry to the master.
//! - [`waitpid_nonblock`]: reap a child without blocking (used by
//!   `try_wait`/`shutdown_and_reap`).
//! - [`waitpid_block`]: block in the kernel until a child exits (used by
//!   the exit waiter thread; retries `EINTR`, maps `ECHILD` to `None` via
//!   the shared `OnceLock`).
//! - [`kill_pgid`]: signal a child's process group.
#[cfg(target_os = "macos")]
pub mod macos;

#[cfg(not(target_os = "macos"))]
compile_error!("mr-crabs-pty macOS implementation only");

#[cfg(target_os = "macos")]
pub(crate) use macos::{kill_pgid, set_winsize, spawn_pty_child, waitpid_block, waitpid_nonblock};

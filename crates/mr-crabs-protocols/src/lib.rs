//! Bounded incremental terminal extended protocols for mr-crabs.
//!
//! This crate ports the Ghostty OSC/DCS/APC extended-protocol layer
//! (`src/terminal/osc.zig`, `src/terminal/dcs.zig`, `src/terminal/apc.zig`,
//! `src/terminal/tmux/control.zig`, `src/terminal/device_attributes.zig`,
//! `src/terminal/device_status.zig`, `src/terminal/modes.zig`,
//! `src/terminal/size_report.zig`, `src/terminfo/**`,
//! `src/terminal/snapshot/**`) into pure Rust.
//!
//! Design rules inherited from the oracle:
//!
//! * Every parser is **chunk-boundary invariant**: feeding the same byte
//!   stream in any split point produces the same command sequence.
//! * Every capture is explicitly bounded: fixed captures cap at
//!   [`osc::Parser::MAX_BUF`], allocating captures at a configurable
//!   [`osc::Parser::max_allocating_bytes`] (default
//!   [`osc::Parser::MAX_ALLOCATING_BUF`]); DCS at [`dcs::Handler::max_bytes`];
//!   tmux at [`tmux::ControlParser::max_bytes`]; APC per-protocol at
//!   [`apc::Handler::max_bytes`]. Overflowing a bound rejects the sequence
//!   and drops the remainder without further allocation.
//! * Malformed input never panics and never allocates unboundedly.
//! * No Zig/libghostty-vt runtime dependency and no shell-command execution:
//!   terminfo installation only writes files and returns command strings.

pub mod apc;
pub mod color;
pub mod dcs;
pub mod osc;
pub mod reports;
pub mod semantic_prompt;
pub mod sgr;
pub mod shell;
pub mod sink;
pub mod snapshot;
pub mod terminfo;
pub mod tmux;

/// The string terminator used to end an OSC command. Responses echo the
/// terminator used by the request (Ghostty `osc.Terminator`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Terminator {
    /// `ESC \`
    St,
    /// BEL (`0x07`)
    Bel,
}

impl Terminator {
    /// Initialize from the last byte seen; BEL selects [`Self::Bel`].
    pub fn init(ch: Option<u8>) -> Self {
        match ch {
            Some(0x07) => Self::Bel,
            _ => Self::St,
        }
    }

    /// The terminator bytes.
    pub fn bytes(self) -> &'static [u8] {
        match self {
            Self::St => b"\x1b\\",
            Self::Bel => b"\x07",
        }
    }
}

/// Global defaults matching Ghostty's constants.
pub mod limits {
    /// Maximum size of a "normal" (fixed-buffer) OSC capture.
    pub const OSC_MAX_BUF: usize = 2048;
    /// Maximum size of an OSC that requires dynamically allocated storage.
    pub const OSC_MAX_ALLOCATING_BUF: usize = 8 * 1024 * 1024;
    /// Maximum bytes any DCS command can take.
    pub const DCS_MAX_BYTES: usize = 1024 * 1024;
    /// Maximum tmux control-mode buffer in bytes.
    pub const TMUX_MAX_BYTES: usize = 1024 * 1024;
    /// Maximum bytes each APC protocol can buffer by default.
    pub const APC_KITTY_MAX_BYTES: usize = 65 * 1024 * 1024;
    pub const APC_GLYPH_MAX_BYTES: usize = 1024 * 1024;
    /// Default max bytes retained for unsupported APC identifiers (zero
    /// drops and ignores unknown APC values, matching Ghostty).
    pub const APC_UNKNOWN_MAX_BYTES: usize = 0;
    /// Maximum window title length accepted before truncation.
    pub const MAX_TITLE_LEN: usize = 1024;
    /// Maximum OSC 7 URL length accepted before truncation.
    pub const MAX_PWD_URL_LEN: usize = 4096;
    /// Maximum terminfo name reported for XTGETTCAP `TN`.
    pub const MAX_TERMINFO_NAME_BYTES: usize = 128;
}

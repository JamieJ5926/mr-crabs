//! Compact cell flag bits. Visual/wide/wrap values match Alacritty `Flags`
//! so snapshots and the Ghostty differential corpus stay byte-compatible.
//! `COMBINING` is compact-side only (`Cell::COMBINING`).

pub const INVERSE: u16 = 0b0000_0000_0000_0001;
pub const BOLD: u16 = 0b0000_0000_0000_0010;
pub const ITALIC: u16 = 0b0000_0000_0000_0100;
pub const UNDERLINE: u16 = 0b0000_0000_0000_1000;
pub const WRAPLINE: u16 = 0b0000_0000_0001_0000;
pub const WIDE_CHAR: u16 = 0b0000_0000_0010_0000;
pub const WIDE_CHAR_SPACER: u16 = 0b0000_0000_0100_0000;
pub const DIM: u16 = 0b0000_0000_1000_0000;
pub const HIDDEN: u16 = 0b0000_0001_0000_0000;
pub const STRIKEOUT: u16 = 0b0000_0010_0000_0000;
pub const LEADING_WIDE_CHAR_SPACER: u16 = 0b0000_0100_0000_0000;
pub const DOUBLE_UNDERLINE: u16 = 0b0000_1000_0000_0000;
pub const UNDERCURL: u16 = 0b0001_0000_0000_0000;
pub const DOTTED_UNDERLINE: u16 = 0b0010_0000_0000_0000;
pub const DASHED_UNDERLINE: u16 = 0b0100_0000_0000_0000;
pub const COMBINING: u16 = crate::Cell::COMBINING;

pub const ALL_UNDERLINES: u16 =
    UNDERLINE | DOUBLE_UNDERLINE | UNDERCURL | DOTTED_UNDERLINE | DASHED_UNDERLINE;

pub const WIDE_BITS: u16 = WIDE_CHAR | WIDE_CHAR_SPACER | LEADING_WIDE_CHAR_SPACER;

/// Attribute bits copied from the pen; wide/wrap/combining are cell-local.
pub const PEN_ATTRS: u16 = INVERSE | BOLD | ITALIC | DIM | HIDDEN | STRIKEOUT | ALL_UNDERLINES;

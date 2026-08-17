//! Kitty graphics protocol: command parsing, responses, image loading, and
//! execution.
//!
//! Provenance: `src/terminal/kitty/graphics_command.zig`,
//! `graphics_exec.zig`, `graphics_image.zig` (Ghostty source commit
//! `d2c70a8c7b9b6893c13640c02d7b6f9a1624f3f0`). The parser consumes bytes
//! after the APC `_G` and produces a `Command`; execution happens against
//! an `ImageStore` (`crate::store`) with a `GraphicsHost`.

pub mod command;
pub mod load;

pub use command::{
    Action, AnimationControl, AnimationFrameComposition, AnimationFrameLoading, Command,
    CommandParser, Control, CursorMovement, Delete, Display, ParseError, Quiet, Response,
    Transmission,
};
pub use load::{Limits, LoadingImage, TempFileLimit};

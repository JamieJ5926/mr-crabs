//! Integration seam between the graphics crate and the terminal/app shell.
//!
//! The S6 `mr-crabs-protocols` crate (or any dispatcher) feeds kitty APC
//! payloads to `kitty::CommandParser` and OSC 1337 values to
//! `iterm::parse_file_value`, then hands the resulting commands to
//! `store::ImageStore::execute` with a `GraphicsHost` implementation that
//! owns the PTY output, cursor state, and renderer bookkeeping. This keeps
//! the graphics crate fully self-contained: no dependency on the terminal
//! crate, no renderer, no Zig.

/// A host for side effects produced while executing image commands.
///
/// Implementations must be cheap and must not panic; the store calls these
/// only at command boundaries (never from hot render paths).
pub trait GraphicsHost {
    /// Deliver response bytes to the application (PTY output). The bytes are
    /// a complete APC response (`\x1b_G...\x1b\\`) or glyph reply.
    fn write_response(&mut self, bytes: &[u8]);

    /// Move the terminal cursor after a placement with `C=0` (kitty
    /// "cursor movement after display"): index `rows` times downward, then
    /// set the column to `col` on the current row. `rows` is already bounded
    /// by the terminal height; `col` is saturated.
    fn cursor_after_placement(&mut self, rows: u32, col: u32);

    /// The image/placement set changed (or a placement was pruned): the
    /// renderer should treat its texture state as dirty.
    fn storage_changed(&mut self);
}

/// A host that ignores every notification; useful for tests and for
/// headless processing where responses are collected separately.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoopHost;

impl GraphicsHost for NoopHost {
    fn write_response(&mut self, _bytes: &[u8]) {}
    fn cursor_after_placement(&mut self, _rows: u32, _col: u32) {}
    fn storage_changed(&mut self) {}
}

/// A host that records every notification for assertions (tests).
#[derive(Clone, Debug, Default)]
pub struct RecordingHost {
    pub responses: Vec<Vec<u8>>,
    pub cursor_moves: Vec<(u32, u32)>,
    pub storage_changes: usize,
}

impl RecordingHost {
    pub fn response_bytes(&self) -> Vec<u8> {
        let mut all = Vec::new();
        for r in &self.responses {
            all.extend_from_slice(r);
        }
        all
    }
}

impl GraphicsHost for RecordingHost {
    fn write_response(&mut self, bytes: &[u8]) {
        self.responses.push(bytes.to_vec());
    }
    fn cursor_after_placement(&mut self, rows: u32, col: u32) {
        self.cursor_moves.push((rows, col));
    }
    fn storage_changed(&mut self) {
        self.storage_changes += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recording_host_accumulates() {
        let mut h = RecordingHost::default();
        h.write_response(b"\x1b_Gi=1;OK\x1b\\");
        h.cursor_after_placement(2, 5);
        h.storage_changed();
        assert_eq!(h.responses.len(), 1);
        assert_eq!(h.cursor_moves, vec![(2, 5)]);
        assert_eq!(h.storage_changes, 1);
        assert_eq!(h.response_bytes(), b"\x1b_Gi=1;OK\x1b\\");
    }
}

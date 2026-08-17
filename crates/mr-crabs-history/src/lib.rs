//! S8: viewport scrolling, search, selection, hyperlinks, persistence, and
//! snapshot restore over the single `mr_crabs_terminal::Terminal` model.
//!
//! This crate never creates a second terminal model: viewport and search
//! consume the compact paged scrollback through the read-only
//! [`HistoryRead`](mr_crabs_terminal::HistoryRead) contract, selection works
//! over captured lines, hyperlink lookup reads the engine's OSC 8 state, and
//! snapshot restore drives the engine's own grid/mode/cursor setters.
//!
//! Module map (S8 parity manifest ownership):
//! - [`viewport`] — `viewport-scrolling`, `compressed-page-boundaries`,
//!   `uncompressed-page-boundaries`, `history-resize-reflow`
//! - [`search`] — `search-worker`, `search-cancellation`
//! - [`selection`] — `selection-gestures`
//! - [`hyperlinks`] — `hyperlink-interaction`
//! - [`persist`] — `history-persistence`
//! - [`replay`] — `snapshot-restore`, `replay-restore`
//! - alternate-screen transitions — [`replay`] plus the terminal engine's
//!   alt-history mark/truncation (`history-alt-screen-transitions`)
//!
//! Bounds: every payload, cache, queue, and result list has an explicit
//! size/count limit; see each module for its constants.

pub mod hyperlinks;
pub mod persist;
pub mod replay;
pub mod search;
pub mod selection;
pub mod viewport;

pub use hyperlinks::{HyperlinkSpan, hyperlink_at, hyperlink_span};
pub use persist::{HistoryFile, PersistConfig, PersistError};
pub use replay::{ReplayError, ReplayEvent, ReplayLog, TerminalSnapshot};
pub use search::{
    DEFAULT_SEARCH_LIMIT, MAX_NEEDLE_BYTES, SearchDirection, SearchMatch, SearchOutcome,
    SearchRequest, SearchSpan, SearchStart, SearchWorker, row_text, search_slice, search_sync,
};
pub use selection::{
    DEFAULT_WORD_BOUNDARIES, ExtractOptions, Selection, SelectionGesture, SelectionPoint,
    WordBoundaries, expand_line, expand_word, selection_text,
};
pub use viewport::{Viewport, ViewportRow, project_frame, viewport_row};

use mr_crabs_terminal::{Cell, NormalizedSnapshot};

/// Split a normalized snapshot's flat cell buffer into per-row vectors.
/// The visible rows are the search/selection/viewport suffix of the line
/// space (`history_len .. history_len + rows`).
pub fn visible_rows(snapshot: &NormalizedSnapshot) -> Vec<Vec<Cell>> {
    let cols = usize::from(snapshot.size.cols);
    let rows = usize::from(snapshot.size.rows);
    let mut out = Vec::with_capacity(rows);
    for row in 0..rows {
        let start = row * cols;
        out.push(snapshot.cells[start..start + cols].to_vec());
    }
    out
}

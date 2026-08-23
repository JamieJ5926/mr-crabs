//! Pooled frame-delta boundary between the terminal engine and the GPUI
//! element (S4).
//!
//! A [`FrameDelta`] is an owned snapshot of everything that changed since the
//! previous build: the engine hands it to the renderer and retains no borrow
//! of the terminal lock. [`FramePool`](crate::FramePool) recycles frames so
//! steady-state rendering performs no allocation growth.

use crate::{Cell, DamageKind, GridSize, Style};

/// A contiguous run of same-style cells within one row.
///
/// Runs are derived from the row's cells by coalescing adjacent cells whose
/// style index is equal. Wide character pairs (a `WIDE` cell followed by its
/// `WIDE_SPACER` cell) are never split across runs: the spacer always stays
/// in the wide cell's run so the renderer can draw the pair as one glyph.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Run {
    pub start_col: u16,
    pub len: u16,
    pub style: u16,
}

/// One changed visible row of the terminal grid.
///
/// `generation` is bumped whenever the row mutates (feed/resize/mode change);
/// renderers can compare it against a cached value to decide whether a row
/// batch is stale without diffing cells.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RowDelta {
    pub row: u16,
    pub generation: u64,
    pub cells: Vec<Cell>,
    pub runs: Vec<Run>,
}

/// Terminal cursor shape (subset of the ANSI/vte shapes; `Beam` maps to
/// `Bar`, and an invisible cursor is expressed via [`CursorState::visible`]).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CursorShape {
    #[default]
    Block,
    Bar,
    Underline,
    HollowBlock,
}

/// Full cursor rendering state for one frame.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CursorState {
    pub row: u16,
    pub col: u16,
    pub shape: CursorShape,
    pub blinking: bool,
    pub visible: bool,
    pub wrap_pending: bool,
}

/// Terminal viewport state for one frame.
///
/// `scroll_offset` is the current scroll offset in rows.
/// `history_rows` is the number of rows in the terminal history.
/// `alternate_screen` is whether the alternate screen is active.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TerminalViewport {
    pub scroll_offset: u32,
    pub history_rows: u32,
    pub alternate_screen: bool,
}

impl Default for CursorState {
    fn default() -> Self {
        Self {
            row: 0,
            col: 0,
            shape: CursorShape::Block,
            blinking: false,
            visible: true,
            wrap_pending: false,
        }
    }
}
/// Selection geometry kind projected into the visible grid.
///
/// `Linear` covers the row-major rectangle between the anchors; `Rectangular`
/// covers the exact block between the anchor columns on every spanned row.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SelectionKind {
    #[default]
    Linear,
    Rectangular,
}

/// Selection anchors `(row, col)` in the visible grid; `None` when there is
/// no selection.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct SelectionState {
    pub start: Option<(u16, u16)>,
    pub end: Option<(u16, u16)>,
    pub active: bool,
    pub kind: SelectionKind,
}

/// A grid point in renderer-neutral frame space: `row`/`col` are viewport
/// coordinates.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FramePoint {
    pub row: u16,
    pub col: u16,
}

/// A half-open row-major grid range in frame space: `start` inclusive,
/// `end` exclusive.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FrameRange {
    pub start: FramePoint,
    pub end: FramePoint,
}

/// One projected search match in a frame.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrameSearchMatch {
    pub range: FrameRange,
    pub current: bool,
}

/// One OSC 8 hyperlink span visible in a frame.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FrameHyperlink {
    pub range: FrameRange,
    pub id: Option<String>,
    pub uri: String,
}

/// Zero-sized placeholder for future image payloads (kitty graphics, iTerm2,
/// OSC 52). Always [`Default`]; the field exists so the frame contract does
/// not need to change when image support lands.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ImageDeltaPlaceholder {
    pub _private: (),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AnimationRegion {
    pub row: u16,
    pub col: u16,
    pub size: GridSize,
}

/// One frame of terminal output, owned by the caller.
///
/// `rows` holds a [`RowDelta`] per changed row: [`DamageKind::Clean`] yields
/// an empty list, `Partial` only the damaged rows, `Full` every visible row.
/// `styles` is the stable interned style table referenced by `Cell::style`
/// indices; indices are stable across frames so renderers may cache resolved
/// styles and run batches.
///
/// The private `spare_rows` stash retains retired [`RowDelta`] allocations
#[derive(Clone, Debug)]
pub struct FrameDelta {
    pub sequence: u64,
    pub style_epoch: u64,
    pub size: GridSize,
    pub damage: DamageKind,
    pub rows: Vec<RowDelta>,
    pub cursor: CursorState,
    pub selection: SelectionState,
    pub styles: Vec<Style>,
    pub images: ImageDeltaPlaceholder,
    pub viewport: TerminalViewport,
    pub search_matches: Vec<FrameSearchMatch>,
    pub hyperlinks: Vec<FrameHyperlink>,
    pub animation_region: Option<AnimationRegion>,
    pub(crate) spare_rows: Vec<RowDelta>,
}

impl PartialEq for FrameDelta {
    fn eq(&self, other: &Self) -> bool {
        self.sequence == other.sequence
            && self.style_epoch == other.style_epoch
            && self.size == other.size
            && self.damage == other.damage
            && self.rows == other.rows
            && self.cursor == other.cursor
            && self.selection == other.selection
            && self.styles == other.styles
            && self.images == other.images
            && self.viewport == other.viewport
            && self.search_matches == other.search_matches
            && self.hyperlinks == other.hyperlinks
            && self.animation_region == other.animation_region
    }
}
impl Eq for FrameDelta {}
impl FrameDelta {
    pub fn empty(size: GridSize) -> Self {
        Self {
            sequence: 0,
            style_epoch: 0,
            size,
            damage: DamageKind::Clean,
            rows: Vec::new(),
            cursor: CursorState::default(),
            selection: SelectionState::default(),
            styles: Vec::new(),
            images: ImageDeltaPlaceholder { _private: () },
            viewport: TerminalViewport::default(),
            search_matches: Vec::new(),
            hyperlinks: Vec::new(),
            animation_region: None,
            spare_rows: Vec::new(),
        }
    }

    /// Clear all frame content while retaining every allocation for reuse.
    ///
    /// Row slots (with their cell/run capacities) move into the private spare
    /// stash; the row and style vectors keep their capacities via `clear`.
    pub fn clear_for_reuse(&mut self) {
        for row in &mut self.rows {
            row.cells.clear();
            row.runs.clear();
        }
        self.spare_rows.append(&mut self.rows);
        self.styles.clear();
        self.search_matches.clear();
        self.hyperlinks.clear();
        self.animation_region = None;
    }

    /// Take an empty row slot, reusing a retired slot's allocations when one
    pub(crate) fn take_row(&mut self) -> RowDelta {
        self.spare_rows.pop().unwrap_or_else(|| RowDelta {
            row: 0,
            generation: 0,
            cells: Vec::new(),
            runs: Vec::new(),
        })
    }
}
/// Coalesce `cells` into same-style runs, refilling `out` without allocating
/// beyond its retained capacity.
///
/// A `WIDE` cell forces its immediate successor (the `WIDE_SPACER`) into the
/// same run regardless of style, so wide pairs are never split across runs.
///
/// # Canonical status
///
/// This is the canonical batching implementation for frame rows: the terminal
/// engine calls it from `Terminal::build_row_delta`, and every `RowDelta::runs`
/// batch a renderer consumes originates here. `mr_crabs_element` additionally
/// exports a style-only `batch_runs(&[Cell]) -> Vec<Run>` helper that has NO
/// `WIDE`/`WIDE_SPACER` handling and would split wide pairs; it is used by
/// element-internal tests only, never by frame production paths. Do not use it
/// to batch terminal rows, and keep the two in sync if that helper is ever
/// promoted to a production path.
pub fn batch_runs(cells: &[Cell], out: &mut Vec<Run>) {
    out.clear();
    if cells.is_empty() {
        return;
    }
    let mut start = 0usize;
    let mut i = 1usize;
    while i < cells.len() {
        let same_style = cells[i].style == cells[start].style;
        let previous_is_wide = cells[i - 1].flags & Cell::WIDE != 0;
        if same_style || previous_is_wide {
            i += 1;
        } else {
            out.push(Run {
                start_col: u16::try_from(start).expect("row fits u16"),
                len: u16::try_from(i - start).expect("run fits u16"),
                style: cells[start].style,
            });
            start = i;
            i += 1;
        }
    }
    out.push(Run {
        start_col: u16::try_from(start).expect("row fits u16"),
        len: u16::try_from(cells.len() - start).expect("run fits u16"),
        style: cells[start].style,
    });
}

#[cfg(test)]
mod tests {
    use super::{
        FrameDelta, FrameHyperlink, FramePoint, FrameRange, FrameSearchMatch, Run,
        TerminalViewport, batch_runs,
    };
    use crate::delta::{
        CursorState, DamageKind, GridSize, ImageDeltaPlaceholder, RowDelta, SelectionState,
    };
    use crate::{Cell, Style};

    #[test]
    fn batching_coalesces_equal_styles() {
        let cells = vec![
            Cell {
                content: u32::from('a'),
                style: 0,
                flags: 0,
            },
            Cell {
                content: u32::from('b'),
                style: 0,
                flags: 0,
            },
            Cell {
                content: u32::from('c'),
                style: 1,
                flags: 0,
            },
            Cell {
                content: u32::from('d'),
                style: 1,
                flags: 0,
            },
            Cell {
                content: u32::from('e'),
                style: 1,
                flags: 0,
            },
            Cell {
                content: u32::from('f'),
                style: 0,
                flags: 0,
            },
        ];
        let mut runs = Vec::new();
        batch_runs(&cells, &mut runs);
        assert_eq!(
            runs,
            vec![
                Run {
                    start_col: 0,
                    len: 2,
                    style: 0
                },
                Run {
                    start_col: 2,
                    len: 3,
                    style: 1
                },
                Run {
                    start_col: 5,
                    len: 1,
                    style: 0
                },
            ]
        );
    }

    #[test]
    fn batching_keeps_wide_pairs_together() {
        // The spacer cell carries a *different* style than the wide cell:
        // batching by style alone would split the pair, which is forbidden.
        let cells = vec![
            Cell {
                content: u32::from('a'),
                style: 0,
                flags: 0,
            },
            Cell {
                content: u32::from('b'),
                style: 0,
                flags: 0,
            },
            Cell {
                content: u32::from('界'),
                style: 1,
                flags: Cell::WIDE,
            },
            Cell {
                content: u32::from(' '),
                style: 2,
                flags: Cell::WIDE_SPACER,
            },
            Cell {
                content: u32::from('c'),
                style: 2,
                flags: 0,
            },
        ];
        let mut runs = Vec::new();
        batch_runs(&cells, &mut runs);
        assert_eq!(
            runs,
            vec![
                Run {
                    start_col: 0,
                    len: 2,
                    style: 0
                },
                // Wide char + spacer stay in one run (style of the wide cell).
                Run {
                    start_col: 2,
                    len: 2,
                    style: 1
                },
                Run {
                    start_col: 4,
                    len: 1,
                    style: 2
                },
            ]
        );
    }

    #[test]
    fn batching_keeps_trailing_wide_pair_together() {
        // A wide pair at the end of the row: the spacer still joins the wide
        // cell's run even though the loop has no following cell to compare.
        let cells = vec![
            Cell {
                content: u32::from('a'),
                style: 0,
                flags: 0,
            },
            Cell {
                content: u32::from('界'),
                style: 1,
                flags: Cell::WIDE,
            },
            Cell {
                content: u32::from(' '),
                style: 2,
                flags: Cell::WIDE_SPACER,
            },
        ];
        let mut runs = Vec::new();
        batch_runs(&cells, &mut runs);
        assert_eq!(
            runs,
            vec![
                Run {
                    start_col: 0,
                    len: 1,
                    style: 0
                },
                Run {
                    start_col: 1,
                    len: 2,
                    style: 1
                },
            ]
        );
    }

    #[test]
    fn batching_handles_empty_and_single_cell_rows() {
        let mut runs = Vec::new();
        batch_runs(&[], &mut runs);
        assert!(runs.is_empty());

        batch_runs(&[Cell::default()], &mut runs);
        assert_eq!(
            runs,
            vec![Run {
                start_col: 0,
                len: 1,
                style: 0
            }]
        );
    }

    #[test]
    fn clear_for_reuse_retains_row_and_style_capacities() {
        let mut frame = FrameDelta {
            sequence: 7,
            style_epoch: 0,
            size: GridSize::new(8, 3),
            damage: DamageKind::Full,
            rows: vec![
                RowDelta {
                    row: 0,
                    generation: 1,
                    cells: vec![Cell::default(); 8],
                    runs: vec![Run {
                        start_col: 0,
                        len: 8,
                        style: 0,
                    }],
                },
                RowDelta {
                    row: 1,
                    generation: 1,
                    cells: vec![Cell::default(); 8],
                    runs: vec![Run {
                        start_col: 0,
                        len: 8,
                        style: 0,
                    }],
                },
            ],
            cursor: CursorState::default(),
            selection: SelectionState::default(),
            styles: vec![Style::default(); 4],
            viewport: TerminalViewport::default(),
            images: ImageDeltaPlaceholder::default(),
            search_matches: vec![FrameSearchMatch {
                range: FrameRange {
                    start: FramePoint { row: 0, col: 0 },
                    end: FramePoint { row: 0, col: 2 },
                },
                current: true,
            }],
            hyperlinks: vec![FrameHyperlink {
                range: FrameRange {
                    start: FramePoint { row: 0, col: 0 },
                    end: FramePoint { row: 0, col: 3 },
                },
                id: None,
                uri: "https://example.com".to_string(),
            }],
            animation_region: None,
            spare_rows: Vec::new(),
        };
        let rows_cap = frame.rows.capacity();
        let styles_cap = frame.styles.capacity();
        let slot_caps: Vec<(usize, usize)> = frame
            .rows
            .iter()
            .map(|row| (row.cells.capacity(), row.runs.capacity()))
            .collect();

        frame.clear_for_reuse();

        assert!(frame.rows.capacity() >= rows_cap, "rows capacity retained");
        assert!(
            frame.styles.capacity() >= styles_cap,
            "styles capacity retained"
        );
        // Row slots moved into the spare stash with their capacities intact.
        assert_eq!(frame.spare_rows.len(), slot_caps.len());
        for (slot, (cells_cap, runs_cap)) in frame.spare_rows.iter().zip(&slot_caps) {
            assert!(
                slot.cells.capacity() >= *cells_cap,
                "cells capacity retained"
            );
            assert!(slot.runs.capacity() >= *runs_cap, "runs capacity retained");
        }

        // Equality ignores the spare stash.
        let other = frame.clone();
        assert_eq!(frame, other);
    }

    #[test]
    fn clear_for_reuse_clears_search_matches_and_hyperlinks_retaining_capacity() {
        let mut frame = FrameDelta::empty(GridSize::new(4, 3));
        frame.search_matches.push(FrameSearchMatch {
            range: FrameRange {
                start: FramePoint { row: 0, col: 0 },
                end: FramePoint { row: 0, col: 4 },
            },
            current: true,
        });
        frame.hyperlinks.push(FrameHyperlink {
            range: FrameRange {
                start: FramePoint { row: 1, col: 0 },
                end: FramePoint { row: 1, col: 5 },
            },
            id: Some("id".to_string()),
            uri: "https://example.com".to_string(),
        });
        let search_cap = frame.search_matches.capacity();
        let link_cap = frame.hyperlinks.capacity();
        frame.clear_for_reuse();
        assert!(frame.search_matches.is_empty());
        assert!(frame.hyperlinks.is_empty());
        assert!(frame.search_matches.capacity() >= search_cap);
        assert!(frame.hyperlinks.capacity() >= link_cap);
    }

    #[test]
    fn take_row_reuses_retired_slot() {
        let mut frame = FrameDelta {
            sequence: 1,
            style_epoch: 0,
            size: GridSize::new(8, 3),
            damage: DamageKind::Clean,
            rows: Vec::new(),
            cursor: CursorState::default(),
            selection: SelectionState::default(),
            styles: Vec::new(),
            viewport: TerminalViewport::default(),
            images: ImageDeltaPlaceholder::default(),
            search_matches: Vec::new(),
            hyperlinks: Vec::new(),
            animation_region: None,
            spare_rows: vec![RowDelta {
                row: 99,
                generation: 42,
                cells: Vec::with_capacity(64),
                runs: Vec::with_capacity(16),
            }],
        };
        let slot = frame.take_row();
        assert_eq!(slot.generation, 42);
        assert!(slot.cells.capacity() >= 64);
        assert!(slot.runs.capacity() >= 16);
        assert!(frame.spare_rows.is_empty());
        // Exhausted stash falls back to a fresh slot.
        let fresh = frame.take_row();
        assert_eq!(fresh.cells.capacity(), 0);
        assert_eq!(fresh.runs.capacity(), 0);
    }

    #[test]
    fn frame_equality_observes_viewport_metadata() {
        let left = FrameDelta::empty(GridSize::new(8, 3));
        let mut right = left.clone();
        right.viewport = TerminalViewport {
            scroll_offset: 2,
            history_rows: 7,
            alternate_screen: false,
        };
        assert_ne!(left, right);

        right.viewport = TerminalViewport::default();
        assert_eq!(left, right);
    }
}

//! Paint-ready render cache over [`FrameDelta`]s.
//!
//! [`RenderCache::apply_frame`] turns a frame's row deltas into retained
//! [`RowBatch`]es (text runs plus background rects). Vectors are cleared and
//! refilled in place, keeping their capacities, so an unchanged same-capacity
//! frame performs no allocation and requests no redraw. Capacity growth only
//! happens when a frame actually requires more rows/batches than have ever
//! been retained, and the grown capacity is itself retained for reuse.

use gpui::SharedString;
use mr_crabs_terminal::{Cell, DamageKind, FrameDelta, RowDelta};

/// A paint-ready text run: `len` cells at `col` with `style`, and the
/// glyph text (spaces, empty cells, and wide-cell spacers excluded) to shape
/// through GPUI's text system.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RunBatch {
    pub col: u16,
    pub len: u16,
    pub style: u16,
    /// Raw terminal attribute flags shared by every cell in this run.
    pub flags: u16,
    pub text: SharedString,
    /// Terminal-cell width consumed by each text character in `text`
    /// (1 for ordinary cells, 2 for wide glyphs whose spacer cell is
    /// consumed). Parallel to `text` in character order, so the paint pass
    /// can anchor every shaped glyph to its terminal cell origin
    /// (`col + prefix sum of widths`) instead of the shaper's natural
    /// advance.
    pub glyph_widths: Vec<u16>,
}

/// A paint-ready background rectangle covering `len` cells at `col` with
/// `style`. Derived directly from the frame's style runs, so adjacent cells
/// with the same style merge into a single rect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RectBatch {
    pub col: u16,
    pub len: u16,
    pub style: u16,
    /// Raw terminal attribute flags shared by every cell in this rectangle.
    pub flags: u16,
}

/// The retained batches for one grid row.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RowBatch {
    pub row: u16,
    pub runs: Vec<RunBatch>,
    pub backgrounds: Vec<RectBatch>,
}

impl RowBatch {
    fn new(row: u16) -> Self {
        Self {
            row,
            runs: Vec::new(),
            backgrounds: Vec::new(),
        }
    }
}

/// Snapshot of the retained capacities after the last rebuild, for tests and
/// the unchanged-frame fast path.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Capacities {
    pub rows: usize,
    pub runs: usize,
    pub backgrounds: usize,
}

/// Result of applying a frame to the cache.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CacheAction {
    /// Whether the grid batches changed and must be repainted.
    pub needs_redraw: bool,
    /// Whether a subsequent animation frame should be scheduled (cursor
    /// blink). The element requests animation frames only while the cursor
    /// blinks; output-pump frames are the application's responsibility.
    pub needs_animation: bool,
}

/// The retained, allocation-reusing paint cache.
pub struct RenderCache {
    /// Row batches in ascending row order; every row of the current grid
    /// that has ever been damaged has a slot here.
    batches: Vec<RowBatch>,
    /// Sequence of the last applied frame.
    last_sequence: Option<u64>,
    /// Capacity snapshot taken after the last rebuild.
    capacities: Capacities,
    /// Forced-rebuild flag (font/atlas invalidation).
    pending_rebuild: bool,
}

impl Default for RenderCache {
    fn default() -> Self {
        Self::new()
    }
}

impl RenderCache {
    pub fn new() -> Self {
        Self {
            batches: Vec::new(),
            last_sequence: None,
            capacities: Capacities::default(),
            pending_rebuild: false,
        }
    }

    /// Apply a frame and return what must happen for painting.
    ///
    /// - `Clean` damage with the same sequence as the last applied frame:
    ///   nothing changed, no rebuild, no allocation, `needs_redraw == false`.
    /// - `Clean` with a newer sequence: cursor/selection state changed but the
    ///   grid is identical, so retained batches stay valid — repaint without
    ///   rebuilding (no allocation).
    /// - Any damaged frame: rebuild the affected rows in place, reusing
    ///   retained row/run/background vectors.
    ///
    /// `frame.rows` must be in ascending row order (the terminal lane emits
    /// row deltas top-down).
    pub fn apply_frame(&mut self, frame: &FrameDelta) -> CacheAction {
        #[cfg(feature = "phase-timing")]
        let _g = crate::phase::Guard::new("render_cache_apply");
        let needs_animation = frame.cursor.visible && frame.cursor.blinking;

        if frame.damage == DamageKind::Clean && self.last_sequence == Some(frame.sequence) {
            // Identical frame: retained batches remain valid and the grid
            // requires no repaint.
            let needs_redraw = self.pending_rebuild || self.batches.capacity() < frame.rows.len();
            if needs_redraw {
                self.rebuild(frame);
            }
            return CacheAction {
                needs_redraw,
                needs_animation,
            };
        }

        if frame.damage != DamageKind::Clean || self.last_sequence.is_none() || self.pending_rebuild
        {
            self.rebuild(frame);
        }
        // Clean + newer sequence: grid unchanged, retained batches stay
        // valid; only the cursor/selection overlays need repainting.
        self.last_sequence = Some(frame.sequence);
        CacheAction {
            needs_redraw: true,
            needs_animation,
        }
    }

    /// Invalidate the cache after an atlas/font change: retained vectors are
    /// kept but every batch is dropped, and the next frame (even `Clean`)
    /// forces a rebuild so glyphs are re-shaped with the new font.
    ///
    /// Callers should pair this with a `Full` damage frame from the terminal
    /// so the visible grid is resent; a `Clean` frame rebuilds to an empty
    /// batch set.
    pub fn reset_for_font_change(&mut self) {
        self.batches.clear();
        self.last_sequence = None;
        self.capacities = Capacities::default();
        self.pending_rebuild = true;
    }

    /// Retained capacity snapshot: `(rows, runs + backgrounds)`.
    pub fn capacities(&self) -> (usize, usize) {
        (
            self.capacities.rows,
            self.capacities.runs + self.capacities.backgrounds,
        )
    }

    /// Full capacity snapshot as a struct.
    pub fn snapshot_capacities(&self) -> Capacities {
        self.capacities
    }

    /// The retained row batches, for painting and tests.
    pub fn batches(&self) -> &[RowBatch] {
        &self.batches
    }

    /// The sequence of the last applied frame.
    pub fn last_sequence(&self) -> Option<u64> {
        self.last_sequence
    }

    fn rebuild(&mut self, frame: &FrameDelta) {
        // Merge frame rows into the retained batches in place. Batches are
        // kept in ascending row order; each row delta either reuses its
        // existing slot (clear + refill, capacity retained) or is inserted
        // when it has never been seen before. Insertion may grow the vec, but
        // only beyond the retained capacity, and that growth is retained.
        let mut old_i = 0usize;
        for rd in &frame.rows {
            while old_i < self.batches.len() && self.batches[old_i].row < rd.row {
                old_i += 1;
            }
            if old_i < self.batches.len() && self.batches[old_i].row == rd.row {
                let batch = &mut self.batches[old_i];
                batch.runs.clear();
                batch.backgrounds.clear();
                fill_batch(batch, rd);
                old_i += 1;
            } else {
                let mut batch = RowBatch::new(rd.row);
                fill_batch(&mut batch, rd);
                self.batches.insert(old_i, batch);
                old_i += 1;
            }
        }
        // A Full frame accounts for every row of the grid; any retained rows
        // beyond it are stale (e.g. after a shrink) and must be dropped.
        if frame.damage == DamageKind::Full {
            self.batches.truncate(old_i);
        }
        self.capacities = self.measure_capacities();
        self.last_sequence = Some(frame.sequence);
        self.pending_rebuild = false;
    }

    fn measure_capacities(&self) -> Capacities {
        Capacities {
            rows: self.batches.capacity(),
            runs: self.batches.iter().map(|b| b.runs.capacity()).sum(),
            backgrounds: self.batches.iter().map(|b| b.backgrounds.capacity()).sum(),
        }
    }
}

/// Refill one row batch from a row delta, reusing retained vectors.
fn fill_batch(batch: &mut RowBatch, rd: &RowDelta) {
    const PRESENTATION_FLAGS: u16 = 0x7b8f;
    let cells = &rd.cells;
    let mut background_start = 0usize;
    while background_start < cells.len() {
        let style = cells[background_start].style;
        let flags = cells[background_start].flags & PRESENTATION_FLAGS;
        let mut background_end = background_start + 1;
        while background_end < cells.len()
            && cells[background_end].style == style
            && cells[background_end].flags & PRESENTATION_FLAGS == flags
        {
            background_end += 1;
        }
        batch.backgrounds.push(RectBatch {
            col: background_start as u16,
            len: (background_end - background_start) as u16,
            style,
            flags,
        });
        background_start = background_end;
    }

    let cells = &rd.cells;
    let mut segment_start = 0usize;
    while segment_start < cells.len() {
        let style = cells[segment_start].style;
        let flags = cells[segment_start].flags & PRESENTATION_FLAGS;
        let mut segment_end = segment_start + 1;
        while segment_end < cells.len()
            && cells[segment_end].style == style
            && cells[segment_end].flags & PRESENTATION_FLAGS == flags
        {
            segment_end += 1;
        }
        let paints = |cell: Cell| {
            cell.flags & Cell::WIDE_SPACER == 0
                && char::from_u32(cell.content).is_some_and(|ch| ch != ' ' && ch != '\0')
        };
        let Some(first_offset) = cells[segment_start..segment_end]
            .iter()
            .position(|cell| paints(*cell))
        else {
            segment_start = segment_end;
            continue;
        };
        let last_offset = cells[segment_start..segment_end]
            .iter()
            .rposition(|cell| paints(*cell))
            .expect("first painted cell establishes a last painted cell");
        let first = segment_start + first_offset;
        let last = segment_start + last_offset;
        let mut text = String::new();
        let mut glyph_widths = Vec::new();
        for cell in &cells[first..=last] {
            if cell.flags & Cell::WIDE_SPACER != 0 {
                // The wide character's own cell accounts for both columns;
                // the spacer contributes no text and no extra width.
                continue;
            }
            if let Some(ch) = char::from_u32(cell.content) {
                text.push(if ch == '\0' { ' ' } else { ch });
                glyph_widths.push(if cell.flags & Cell::WIDE != 0 { 2 } else { 1 });
            }
        }
        batch.runs.push(RunBatch {
            col: first as u16,
            len: (last + 1 - first) as u16,
            style,
            flags,
            text: text.into(),
            glyph_widths,
        });
        segment_start = segment_end;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mr_crabs_terminal::{
        CursorShape, CursorState, FramePool, GridSize, ImageDeltaPlaceholder, Run, SelectionState,
        Style as TermStyle,
    };

    fn frame(
        sequence: u64,
        damage: DamageKind,
        rows: Vec<RowDelta>,
        cursor: CursorState,
    ) -> FrameDelta {
        let mut pool = FramePool::new(1);
        let mut frame = pool.acquire(sequence, GridSize::new(4, 4));
        frame.damage = damage;
        frame.rows = rows;
        frame.cursor = cursor;
        frame.selection = SelectionState {
            start: None,
            end: None,
            active: false,
            kind: mr_crabs_terminal::SelectionKind::Linear,
        };
        frame.styles.push(TermStyle::default());
        frame.images = ImageDeltaPlaceholder::default();
        frame
    }

    fn row_delta(row: u16, cells: Vec<Cell>, runs: Vec<Run>) -> RowDelta {
        RowDelta {
            row,
            generation: 1,
            cells,
            runs,
        }
    }

    const CELL_A: Cell = Cell {
        content: 'a' as u32,
        style: 0,
        flags: 0,
    };
    const CELL_B: Cell = Cell {
        content: 'b' as u32,
        style: 0,
        flags: 0,
    };
    const CELL_SPACE: Cell = Cell {
        content: ' ' as u32,
        style: 0,
        flags: 0,
    };

    fn cursor(blinking: bool) -> CursorState {
        CursorState {
            row: 0,
            col: 0,
            shape: CursorShape::Block,
            blinking,
            visible: true,
            wrap_pending: false,
        }
    }

    fn row_cells(values: &[Cell]) -> RowDelta {
        let runs = crate::batch_runs(values);
        row_delta(0, values.to_vec(), runs)
    }

    #[test]
    fn first_partial_frame_rebuilds() {
        let mut cache = RenderCache::new();
        let f = frame(
            1,
            DamageKind::Partial,
            vec![row_delta(
                0,
                vec![CELL_A, CELL_B],
                vec![Run {
                    start_col: 0,
                    len: 2,
                    style: 0,
                }],
            )],
            cursor(false),
        );
        let action = cache.apply_frame(&f);
        assert!(action.needs_redraw);
        assert!(!action.needs_animation);
        assert_eq!(cache.batches().len(), 1);
        assert_eq!(cache.batches()[0].row, 0);
        assert_eq!(cache.batches()[0].runs.len(), 1);
        assert_eq!(cache.batches()[0].runs[0].text, "ab");
        assert_eq!(cache.batches()[0].runs[0].glyph_widths, vec![1, 1]);
        assert_eq!(cache.batches()[0].backgrounds.len(), 1);
        assert_eq!(cache.last_sequence(), Some(1));
    }

    #[test]
    fn clean_same_sequence_requests_no_redraw_and_no_allocation() {
        let mut cache = RenderCache::new();
        let warm = frame(
            1,
            DamageKind::Partial,
            vec![row_delta(
                0,
                vec![CELL_A, CELL_B],
                vec![Run {
                    start_col: 0,
                    len: 2,
                    style: 0,
                }],
            )],
            cursor(false),
        );
        cache.apply_frame(&warm);
        let before = cache.capacities();

        let idle = frame(1, DamageKind::Clean, Vec::new(), cursor(false));
        let action = cache.apply_frame(&idle);
        assert!(!action.needs_redraw);
        assert!(!action.needs_animation);
        // No rebuild happened: retained capacities are untouched and the
        // batches still describe the warm-up frame.
        assert_eq!(cache.capacities(), before);
        assert_eq!(cache.batches().len(), 1);
        assert_eq!(cache.batches()[0].runs[0].text, "ab");
        assert_eq!(cache.last_sequence(), Some(1));
    }

    #[test]
    fn clean_new_sequence_repaints_without_rebuild() {
        let mut cache = RenderCache::new();
        let warm = frame(
            1,
            DamageKind::Partial,
            vec![row_delta(
                0,
                vec![CELL_A, CELL_B],
                vec![Run {
                    start_col: 0,
                    len: 2,
                    style: 0,
                }],
            )],
            cursor(false),
        );
        cache.apply_frame(&warm);
        let before = cache.capacities();

        // A clean frame with a newer sequence (cursor moved): the grid is
        // unchanged, so retained batches stay valid — repaint, no rebuild,
        // no allocation.
        let clean = frame(2, DamageKind::Clean, Vec::new(), cursor(false));
        let action = cache.apply_frame(&clean);
        assert!(action.needs_redraw);
        assert_eq!(cache.capacities(), before);
        assert_eq!(cache.batches().len(), 1);
        assert_eq!(cache.last_sequence(), Some(2));
    }

    #[test]
    fn repeated_partial_frames_reuse_retained_vectors() {
        let mut cache = RenderCache::new();
        let rows = vec![row_delta(
            0,
            vec![CELL_A, CELL_B],
            vec![Run {
                start_col: 0,
                len: 2,
                style: 0,
            }],
        )];
        cache.apply_frame(&frame(1, DamageKind::Partial, rows.clone(), cursor(false)));
        let warm = cache.capacities();
        assert!(warm.0 >= 1 && warm.1 >= 1);

        // Same-capacity partial frames (same rows, same run counts) must not
        // grow the retained capacities.
        for seq in 2..10u64 {
            let f = frame(seq, DamageKind::Partial, rows.clone(), cursor(false));
            let action = cache.apply_frame(&f);
            assert!(action.needs_redraw);
            assert_eq!(cache.capacities(), warm, "capacities grew at seq {seq}");
            assert_eq!(cache.batches().len(), 1);
        }
    }

    #[test]
    fn full_frame_replaces_all_rows_and_trims_stale() {
        let mut cache = RenderCache::new();
        cache.apply_frame(&frame(
            1,
            DamageKind::Full,
            vec![
                row_delta(
                    0,
                    vec![CELL_A],
                    vec![Run {
                        start_col: 0,
                        len: 1,
                        style: 0,
                    }],
                ),
                row_delta(
                    1,
                    vec![CELL_B],
                    vec![Run {
                        start_col: 0,
                        len: 1,
                        style: 0,
                    }],
                ),
            ],
            cursor(false),
        ));
        assert_eq!(cache.batches().len(), 2);

        // Shrunk grid: a Full frame over one row must drop the stale row.
        let shrunk = frame(
            2,
            DamageKind::Full,
            vec![row_delta(
                0,
                vec![CELL_A],
                vec![Run {
                    start_col: 0,
                    len: 1,
                    style: 0,
                }],
            )],
            cursor(false),
        );
        cache.apply_frame(&shrunk);
        assert_eq!(cache.batches().len(), 1);
        assert_eq!(cache.batches()[0].row, 0);
    }

    #[test]
    fn partial_frame_keeps_untouched_rows() {
        let mut cache = RenderCache::new();
        cache.apply_frame(&frame(
            1,
            DamageKind::Full,
            vec![
                row_delta(
                    0,
                    vec![CELL_A],
                    vec![Run {
                        start_col: 0,
                        len: 1,
                        style: 0,
                    }],
                ),
                row_delta(
                    1,
                    vec![CELL_B],
                    vec![Run {
                        start_col: 0,
                        len: 1,
                        style: 0,
                    }],
                ),
            ],
            cursor(false),
        ));
        // Damage only row 0; row 1 must survive untouched.
        let partial = frame(
            2,
            DamageKind::Partial,
            vec![row_delta(
                0,
                vec![CELL_B],
                vec![Run {
                    start_col: 0,
                    len: 1,
                    style: 0,
                }],
            )],
            cursor(false),
        );
        cache.apply_frame(&partial);
        assert_eq!(cache.batches().len(), 2);
        assert_eq!(cache.batches()[0].runs[0].text, "b");
        assert_eq!(cache.batches()[1].runs[0].text, "b");
    }

    #[test]
    fn spaces_and_wide_spacers_produce_no_text_runs() {
        let wide = Cell {
            content: u32::from('界'),
            style: 0,
            flags: Cell::WIDE,
        };
        let spacer = Cell {
            content: 0,
            style: 0,
            flags: Cell::WIDE_SPACER,
        };
        let cells = vec![CELL_SPACE, wide, spacer, CELL_A];
        let runs = vec![Run {
            start_col: 0,
            len: 4,
            style: 0,
        }];
        let mut cache = RenderCache::new();
        cache.apply_frame(&frame(
            1,
            DamageKind::Partial,
            vec![row_delta(0, cells, runs)],
            cursor(false),
        ));

        let batch = &cache.batches()[0];
        // One text run: the wide glyph plus 'a' (space and spacer skipped).
        assert_eq!(batch.runs.len(), 1);
        assert_eq!(batch.runs[0].col, 1);
        assert_eq!(batch.runs[0].len, 3);
        assert_eq!(batch.runs[0].text, "界a");
        // Cell widths parallel the text characters: the wide glyph consumes
        // two terminal cells (its spacer is not in the text), 'a' one.
        assert_eq!(batch.runs[0].glyph_widths, vec![2, 1]);
        // Backgrounds still cover every cell.
        assert_eq!(batch.backgrounds.len(), 1);
        assert_eq!(batch.backgrounds[0].len, 4);
    }

    #[test]
    fn style_boundaries_split_runs() {
        let mut cache = RenderCache::new();
        let cells = vec![
            Cell {
                content: u32::from('x'),
                style: 0,
                flags: 0,
            },
            Cell {
                content: u32::from('y'),
                style: 1,
                flags: 0,
            },
            Cell {
                content: u32::from('z'),
                style: 1,
                flags: 0,
            },
        ];
        let runs = vec![
            Run {
                start_col: 0,
                len: 1,
                style: 0,
            },
            Run {
                start_col: 1,
                len: 2,
                style: 1,
            },
        ];
        cache.apply_frame(&frame(
            1,
            DamageKind::Partial,
            vec![row_delta(0, cells, runs)],
            cursor(false),
        ));
        let batch = &cache.batches()[0];
        assert_eq!(batch.runs.len(), 2);
        assert_eq!(
            (
                batch.runs[0].col,
                batch.runs[0].style,
                batch.runs[0].text.as_str()
            ),
            (0, 0, "x")
        );
        assert_eq!(
            (
                batch.runs[1].col,
                batch.runs[1].style,
                batch.runs[1].text.as_str()
            ),
            (1, 1, "yz")
        );
        assert_eq!(batch.backgrounds.len(), 2);
    }

    #[test]
    fn reset_for_font_change_forces_rebuild() {
        let mut cache = RenderCache::new();
        cache.apply_frame(&frame(
            1,
            DamageKind::Partial,
            vec![row_cells(&[CELL_A])],
            cursor(false),
        ));
        assert!(!cache.batches().is_empty());

        cache.reset_for_font_change();
        assert!(cache.batches().is_empty());
        assert_eq!(cache.last_sequence(), None);
        assert_eq!(cache.capacities(), (0, 0));

        // Even a Clean frame rebuilds (to the frame's rows) after the reset.
        let clean = frame(2, DamageKind::Clean, Vec::new(), cursor(false));
        let action = cache.apply_frame(&clean);
        assert!(action.needs_redraw);
        assert_eq!(cache.last_sequence(), Some(2));
        // A clean frame carries no rows, so the rebuilt cache is empty.
        assert!(cache.batches().is_empty());
    }

    #[test]
    fn blinking_cursor_requests_animation() {
        let mut cache = RenderCache::new();
        let f = frame(
            1,
            DamageKind::Partial,
            vec![row_delta(
                0,
                vec![CELL_A],
                vec![Run {
                    start_col: 0,
                    len: 1,
                    style: 0,
                }],
            )],
            cursor(true),
        );
        let action = cache.apply_frame(&f);
        assert!(action.needs_animation);
    }
}

//! Viewport scrolling across hot and compressed history (S8).
//!
//! A [`Viewport`] is a scroll offset over the *line space*: logical history
//! lines `[0, history_len)` followed by the visible grid rows
//! `[history_len, history_len + rows)`. Offset `0` shows the visible grid;
//! offset `history_len` shows the top of the buffer. Storage page state
//! (hot vs compressed) is invisible to the viewport — [`viewport_row`]
//! decompresses cold pages through the terminal's bounded read cache — so
//! scrolling behavior is identical across compressed/uncompressed page
//! boundaries. History lines keep their original widths across resizes
//! (`history-resize-reflow`); the viewport reports each row's own `cols`.

use mr_crabs_terminal::{
    Cell, DamageKind, FrameDelta, HistoryRead, NormalizedSnapshot, RowDelta, Terminal,
    TerminalError, TerminalMode, TerminalViewport, batch_runs,
};

/// Primary-screen scroll position plus alternate-screen visibility.
///
/// The saved primary offset is retained while the alternate screen is active;
/// the effective [`Viewport::offset`] is zero there so primary history is
/// never projected behind alternate-screen content.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Viewport {
    primary_offset: usize,
    alternate_screen: bool,
}

impl Viewport {
    pub const fn new() -> Self {
        Self {
            primary_offset: 0,
            alternate_screen: false,
        }
    }

    pub const fn offset(&self) -> usize {
        if self.alternate_screen {
            0
        } else {
            self.primary_offset
        }
    }

    pub const fn alternate_screen(&self) -> bool {
        self.alternate_screen
    }

    /// Synchronize screen state. Entering alternate screen preserves the
    /// primary offset; leaving restores it, clamped to retained history.
    pub fn sync_screen(&mut self, alternate_screen: bool, history_lines: usize) {
        self.alternate_screen = alternate_screen;
        self.primary_offset = self.primary_offset.min(history_lines);
    }

    /// Clamp the saved primary offset into `[0, history_lines]`.
    pub fn clamp(&mut self, history_lines: usize) {
        self.primary_offset = self.primary_offset.min(history_lines);
    }

    /// Scroll toward the top of the primary buffer by `amount` lines.
    pub fn scroll_up(&mut self, amount: usize, history_lines: usize) {
        if !self.alternate_screen {
            self.primary_offset = self
                .primary_offset
                .saturating_add(amount)
                .min(history_lines);
        }
    }

    /// Scroll toward the bottom of the primary buffer by `amount` lines.
    pub fn scroll_down(&mut self, amount: usize) {
        if !self.alternate_screen {
            self.primary_offset = self.primary_offset.saturating_sub(amount);
        }
    }

    /// Preserve the displayed primary content when retained history grows.
    /// A live primary viewport remains at the bottom. At a capped store,
    /// where the retained length does not grow, the offset stays unchanged.
    pub fn note_history_growth(&mut self, previous_history_lines: usize, history_lines: usize) {
        if self.primary_offset != 0 {
            self.primary_offset = self
                .primary_offset
                .saturating_add(history_lines.saturating_sub(previous_history_lines))
                .min(history_lines);
        }
    }

    pub fn to_top(&mut self, history_lines: usize) {
        if !self.alternate_screen {
            self.primary_offset = history_lines;
        }
    }

    pub fn reset(&mut self) {
        if !self.alternate_screen {
            self.primary_offset = 0;
        }
    }

    /// Absolute line index of the top viewport row.
    pub fn top_line(&self, history_lines: usize) -> usize {
        history_lines.saturating_sub(self.offset().min(history_lines))
    }

    /// Absolute line index shown at viewport row `row` (`0` = top), or
    /// `None` when the viewport exposes fewer lines than `row` (possible
    /// when the buffer is shorter than the grid... the line space always has
    /// `history_len + rows` lines, so this is `None` only for `row >= rows`).
    pub fn absolute_line(&self, row: u16, history_lines: usize, rows: u16) -> Option<usize> {
        if usize::from(row) >= usize::from(rows) {
            return None;
        }
        self.top_line(history_lines).checked_add(usize::from(row))
    }
}

/// One viewport row: an absolute line index plus its cells and width.
/// History rows carry their original (possibly pre-resize) width.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ViewportRow {
    pub absolute: usize,
    pub cols: u16,
    pub cells: Vec<Cell>,
}

/// Resolve viewport row `row` against the current history and a captured
/// snapshot of the visible grid. Returns `None` for out-of-range rows or
/// unreadable (corrupt) history pages; never fabricates content.
pub fn viewport_row<R: HistoryRead + ?Sized>(
    reader: &mut R,
    snapshot: &NormalizedSnapshot,
    viewport: &Viewport,
    row: u16,
) -> Option<ViewportRow> {
    let history_lines = reader.history_len();
    let rows = snapshot.size.rows;
    let absolute = viewport.absolute_line(row, history_lines, rows)?;
    if absolute < history_lines {
        let mut cells = Vec::new();
        if !reader.read_history_line(absolute, &mut cells) {
            return None;
        }
        let cols = reader.history_line_cols(absolute)?;
        let cols = u16::try_from(cols).ok()?;
        Some(ViewportRow {
            absolute,
            cols,
            cells,
        })
    } else {
        let screen_row = absolute - history_lines;
        let cols = snapshot.size.cols;
        let start = screen_row * usize::from(cols);
        Some(ViewportRow {
            absolute,
            cols,
            cells: snapshot.cells[start..start + usize::from(cols)].to_vec(),
        })
    }
}

/// Apply renderer-neutral viewport state and, when scrolled, replace the live
/// delta rows with a full projection over retained history plus live rows.
///
/// The live path only stamps metadata; it does not take another terminal
/// snapshot. Alternate-screen detection is synchronized before reading the
/// effective offset, so primary history is never projected behind it.
pub fn project_frame(
    terminal: &mut Terminal,
    viewport: &mut Viewport,
    frame: &mut FrameDelta,
) -> Result<(), TerminalError> {
    let history_lines = terminal.history_len();
    viewport.sync_screen(terminal.has_mode(TerminalMode::AltScreen), history_lines);
    frame.viewport = TerminalViewport {
        scroll_offset: u32::try_from(viewport.offset()).unwrap_or(u32::MAX),
        history_rows: u32::try_from(history_lines).unwrap_or(u32::MAX),
        alternate_screen: viewport.alternate_screen(),
    };
    if viewport.offset() == 0 {
        return Ok(());
    }

    let snapshot = terminal.snapshot();
    let cols = usize::from(snapshot.size.cols);
    // Gather projected rows, distinguishing history origin from visible snapshot
    // rows. The style slice must not be held across viewport_row reads, so
    // collect first and reborrow after.
    let mut projected: Vec<(u16, ViewportRow, bool)> =
        Vec::with_capacity(usize::from(snapshot.size.rows));
    for row in 0..snapshot.size.rows {
        let vr = viewport_row(terminal, &snapshot, viewport, row)
            .ok_or(TerminalError::StyleCompactionCorrupt)?;
        let is_history = vr.absolute < history_lines;
        projected.push((row, vr, is_history));
    }

    // Seed frame-local style table from the visible snapshot (already frame-local).
    let frame_size = snapshot.size;
    frame.styles = snapshot.styles;
    let mut style_to_local: std::collections::HashMap<_, u16> =
        std::collections::HashMap::with_capacity(frame.styles.len());
    for (idx, style) in frame.styles.iter().enumerate() {
        let id = idx as u16;
        style_to_local.entry(style.clone()).or_insert(id);
    }
    // Prepare per-frame remap for history-origin global IDs only. Visible
    // snapshot cells are already frame-local and must not be remapped.
    let global_len = terminal.global_styles().len();
    let mut global_to_local: Vec<u16> = vec![u16::MAX; global_len];
    if global_len > 0 {
        global_to_local[0] = 0;
    }
    let mut rows = Vec::with_capacity(projected.len());
    for (row_idx, vr, is_history) in projected {
        let mut cells = vr.cells;
        cells.truncate(cols);
        cells.resize(cols, Cell::default());
        if is_history {
            // Reborrow global styles for each row to satisfy the borrow rule
            // (do not hold slice across viewport_row, already satisfied; here
            // we reborrow per-row to keep the slice short-lived).
            let global = terminal.global_styles();
            for cell in &mut cells {
                let gid = usize::from(cell.style);
                if gid >= global.len() {
                    return Err(TerminalError::StyleCompactionCorrupt);
                }
                let mapped = global_to_local[gid];
                if mapped != u16::MAX {
                    cell.style = mapped;
                } else {
                    let style = &global[gid];
                    if let Some(&existing) = style_to_local.get(style) {
                        global_to_local[gid] = existing;
                        cell.style = existing;
                    } else if frame.styles.len() <= 65_535 {
                        let local = frame.styles.len() as u16;
                        frame.styles.push(style.clone());
                        style_to_local.insert(style.clone(), local);
                        global_to_local[gid] = local;
                        cell.style = local;
                    } else {
                        return Err(TerminalError::StyleCompactionCapacity);
                    }
                }
            }
        }
        let mut runs = Vec::new();
        batch_runs(&cells, &mut runs);
        rows.push(RowDelta {
            row: row_idx,
            generation: frame.sequence,
            cells,
            runs,
        });
    }
    frame.size = frame_size;
    frame.damage = DamageKind::Full;
    frame.rows = rows;
    frame.cursor.visible = false;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{Viewport, project_frame, viewport_row};
    use mr_crabs_terminal::{
        DamageKind, FrameDelta, GridSize, HistoryRead, NormalizedSnapshot, Terminal,
    };

    fn snapshot_for(term: &Terminal) -> NormalizedSnapshot {
        term.snapshot()
    }

    #[test]
    fn offset_math_is_clamped_and_exact() {
        let mut vp = Viewport::new();
        assert_eq!(vp.offset(), 0);
        // Offset 0 = visible grid: top line is history_len.
        assert_eq!(vp.top_line(5), 5);
        vp.scroll_up(3, 5);
        assert_eq!(vp.offset(), 3);
        assert_eq!(vp.top_line(5), 2);
        vp.scroll_up(99, 5);
        assert_eq!(vp.offset(), 5, "clamped to history length");
        assert_eq!(vp.top_line(5), 0);
        vp.scroll_down(2);
        assert_eq!(vp.offset(), 3);
        vp.reset();
        assert_eq!(vp.offset(), 0);
        let mut pinned = Viewport::new();
        pinned.scroll_up(2, 5);
        pinned.note_history_growth(5, 8);
        assert_eq!(pinned.offset(), 5);
        assert_eq!(pinned.top_line(8), 3, "absolute top line stays pinned");
        pinned.note_history_growth(8, 6);
        assert_eq!(pinned.offset(), 5, "history shrink only clamps");
        pinned.clamp(4);
        assert_eq!(pinned.offset(), 4);

        let mut bottom = Viewport::new();
        bottom.note_history_growth(5, 8);
        assert_eq!(bottom.offset(), 0, "live bottom follows new output");
        vp.to_top(5);
        assert_eq!(vp.offset(), 5);
        // absolute_line maps viewport rows onto the line space.
        let mut vp = Viewport::new();
        vp.scroll_up(2, 5);
        assert_eq!(vp.absolute_line(0, 5, 3), Some(3));
        assert_eq!(vp.absolute_line(2, 5, 3), Some(5));
        assert_eq!(vp.absolute_line(3, 5, 3), None);
    }

    #[test]
    fn alternate_screen_isolates_and_restores_primary_offset() {
        let mut vp = Viewport::new();
        vp.scroll_up(3, 5);
        vp.sync_screen(true, 5);
        assert!(vp.alternate_screen());
        assert_eq!(vp.offset(), 0);
        vp.scroll_up(2, 5);
        vp.scroll_down(2);
        vp.to_top(5);
        vp.reset();
        assert_eq!(vp.offset(), 0, "alternate mutations are ignored");

        vp.sync_screen(false, 2);
        assert!(!vp.alternate_screen());
        assert_eq!(
            vp.offset(),
            2,
            "saved primary offset is restored and clamped"
        );
    }

    #[test]
    fn projection_stamps_live_metadata_and_materializes_scrolled_rows() {
        let mut term = Terminal::new(GridSize::new(5, 2)).unwrap();
        term.feed(b"old1\r\nold2\r\nlive!")
            .expect("viewport live projection fixture feed should succeed for old1/old2/live");
        assert_eq!(term.history_len(), 1);
        let mut pool = mr_crabs_terminal::frame_pool_default();
        let mut live = term.build_frame_delta(&mut pool);
        let live_damage = live.damage;
        let mut vp = Viewport::new();
        project_frame(&mut term, &mut vp, &mut live).expect("project_frame");
        assert_eq!(
            live.viewport,
            mr_crabs_terminal::TerminalViewport {
                scroll_offset: 0,
                history_rows: 1,
                alternate_screen: false,
            }
        );
        assert_eq!(
            live.damage, live_damage,
            "live projection does not force full"
        );
        let mut scrolled = term.build_frame_delta(&mut pool);
        vp.scroll_up(1, term.history_len());
        project_frame(&mut term, &mut vp, &mut scrolled).expect("project_frame");
        assert_eq!(scrolled.damage, DamageKind::Full);
        assert!(!scrolled.cursor.visible);
        assert_eq!(scrolled.rows.len(), 2);
        assert_eq!(
            scrolled.rows[0].cells[..4]
                .iter()
                .map(|cell| char::from_u32(cell.content).expect("text cell"))
                .collect::<String>(),
            "old1"
        );
        assert_eq!(
            scrolled.rows[1].cells[..4]
                .iter()
                .map(|cell| char::from_u32(cell.content).expect("text cell"))
                .collect::<String>(),
            "old2"
        );
        let hot_rows = scrolled.rows.clone();

        term.force_compress_all();
        let mut cold = term.build_frame_delta(&mut pool);
        project_frame(&mut term, &mut vp, &mut cold).expect("project_frame");
        assert_eq!(cold.rows.len(), hot_rows.len());
        for (cold_row, hot_row) in cold.rows.iter().zip(&hot_rows) {
            assert_eq!(
                cold_row.cells, hot_row.cells,
                "cold cells are byte-identical"
            );
            assert_eq!(cold_row.runs, hot_row.runs, "cold runs are byte-identical");
        }
    }

    #[test]
    fn projection_never_paints_primary_history_in_alternate_screen() {
        let mut term = Terminal::new(GridSize::new(5, 2)).unwrap();
        term.feed(b"old1\r\nold2\r\nlive!")
            .expect("viewport alt-screen fixture feed should succeed for old1/old2/live");
        let mut vp = Viewport::new();
        vp.scroll_up(1, term.history_len());
        let mut pool = mr_crabs_terminal::frame_pool_default();

        term.feed(b"\x1b[?1049hALT")
            .expect("viewport alt-screen fixture feed should succeed for alt enter");
        let mut alternate = term.build_frame_delta(&mut pool);
        project_frame(&mut term, &mut vp, &mut alternate).expect("project_frame");
        assert!(alternate.viewport.alternate_screen);
        assert_eq!(alternate.viewport.scroll_offset, 0);
        assert_eq!(vp.offset(), 0);
        let alternate_text = alternate
            .rows
            .iter()
            .flat_map(|row| row.cells.iter())
            .filter_map(|cell| char::from_u32(cell.content))
            .collect::<String>();
        assert!(alternate_text.contains("ALT"));
        assert!(!alternate_text.contains("old1"));

        vp.scroll_up(99, term.history_len());
        term.feed(b"\x1b[?1049l")
            .expect("viewport alt-screen fixture feed should succeed for alt exit");
        let mut primary = term.build_frame_delta(&mut pool);
        project_frame(&mut term, &mut vp, &mut primary).expect("project_frame");
        assert!(!primary.viewport.alternate_screen);
        assert_eq!(vp.offset(), 1);
        assert_eq!(primary.viewport.scroll_offset, 1);
        assert_eq!(
            primary.rows[0].cells[..4]
                .iter()
                .map(|cell| char::from_u32(cell.content).expect("text cell"))
                .collect::<String>(),
            "old1"
        );
    }
    #[test]
    fn capped_history_growth_keeps_offset_within_metadata() {
        let mut term = Terminal::new_with_config(
            GridSize::new(4, 2),
            mr_crabs_terminal::ScrollbackConfig {
                max_lines: 2,
                ..mr_crabs_terminal::ScrollbackConfig::default()
            },
        )
        .unwrap();
        term.feed(b"a001\r\na002\r\na003\r\n")
            .expect("viewport capped history fixture feed should succeed for a001/a002/a003");
        assert_eq!(term.history_len(), 2);
        let mut vp = Viewport::new();
        vp.scroll_up(1, term.history_len());
        let before = term.history_len();
        term.feed(b"a004\r\n")
            .expect("viewport capped history fixture feed should succeed for a004");
        let after = term.history_len();
        assert_eq!(after, before, "bounded store evicts instead of growing");
        vp.note_history_growth(before, after);
        assert_eq!(vp.offset(), 1);
        let mut frame = FrameDelta::empty(GridSize::new(4, 2));
        project_frame(&mut term, &mut vp, &mut frame).expect("project_frame");
    }

    #[test]
    fn viewport_reads_hot_and_cold_history_identically() {
        let size = GridSize::new(8, 3);
        let mut term = Terminal::new_with_config(
            size,
            mr_crabs_terminal::ScrollbackConfig {
                max_lines: 1000,
                hot_page_lines: 2,
                max_queued_jobs: 4,
                max_pending_completions: 4,
            },
        )
        .unwrap();
        // CRLF: "L{i:04}abc" is exactly one 8-column row per feed; a bare
        // LF would leave the cursor in the last column and wrap, adding a
        // second row per feed. Five feeds on a three-row grid scroll three
        // lines (feeds 3-5).
        for i in 0..5 {
            term.feed(format!("L{i:04}abc\r\n").as_bytes())
                .unwrap_or_else(|error| {
                    panic!("viewport hot/cold fixture feed should succeed for L{i:04}abc: {error}")
                });
        }
        let snap = snapshot_for(&term);
        assert_eq!(term.history_len(), 3, "three lines scrolled out");
        let mut vp = Viewport::new();
        vp.to_top(term.history_len());
        let hot_top = viewport_row(&mut term, &snap, &vp, 0).expect("row 0");
        assert_eq!(hot_top.absolute, 0);
        assert_eq!(hot_top.cols, 8);
        assert_eq!(
            hot_top.cells[0].content,
            u32::from('L'),
            "history row 0 is L0000abc"
        );

        // Force every page cold, then read the same rows again.
        term.force_compress_all();
        assert_eq!(term.history_len(), 3);
        let cold_top = viewport_row(&mut term, &snap, &vp, 0).expect("row 0 cold");
        assert_eq!(cold_top.cells, hot_top.cells, "cold read is byte-identical");

        // Scroll down: offset 1 shows history line 2 at the top. History
        // lines are oldest-first ("L0000abc", "L0001abc", "L0002abc"), so
        // line 2 is the feed-2 row: cells[4] is '2', not 'a'.
        let mut vp2 = Viewport::new();
        vp2.scroll_up(1, term.history_len());
        let row = viewport_row(&mut term, &snap, &vp2, 0).expect("row");
        assert_eq!(row.absolute, 2);
        assert_eq!(row.cells[0].content, u32::from('L'));
        assert_eq!(row.cells[1].content, u32::from('0'));
        assert_eq!(row.cells[4].content, u32::from('2'));

        // Offset 0 exposes the visible grid rows through the snapshot.
        let vp3 = Viewport::new();
        let visible = viewport_row(&mut term, &snap, &vp3, 0).expect("visible row");
        assert_eq!(visible.absolute, term.history_len());
        assert_eq!(visible.cells.len(), 8);
    }

    #[test]
    fn resize_keeps_old_history_widths_and_viewport_adapts() {
        let mut term = Terminal::new(GridSize::new(6, 2)).unwrap();
        // CRLF keeps each feed on its own row at column 0; a bare LF would
        // leave the cursor in the last column, wrap the next feed's first
        // character, and scroll two rows per feed.
        for i in 0..4 {
            term.feed(format!("W{i}xxx\r\n").as_bytes())
                .unwrap_or_else(|error| {
                    panic!("viewport resize fixture feed should succeed for W{i}xxx: {error}")
                });
        }
        let narrow = term.history_len();
        // Four one-row feeds on a two-row grid: feeds 2-4 each scroll one
        // line ("W0xxx", "W1xxx", "W2xxx").
        assert_eq!(narrow, 3);
        // Resize wider; new lines take the new width, old lines keep 6.
        term.resize(GridSize::new(10, 2)).unwrap();
        term.feed("LONGERLINE\r\n".as_bytes())
            .expect("viewport resize fixture feed should succeed for LONGERLINE");
        term.feed("ANOTHERONE\r\n".as_bytes())
            .expect("viewport resize fixture feed should succeed for ANOTHERONE");
        assert!(term.history_len() >= 3);
        let snap = snapshot_for(&term);
        let mut vp = Viewport::new();
        vp.to_top(term.history_len());
        let old = viewport_row(&mut term, &snap, &vp, 0).expect("old-width row");
        assert_eq!(old.cols, 6, "pre-resize history keeps its width");
        assert_eq!(old.cells.len(), 6);
        // A bottom viewport exposes the visible grid with the new width.
        let bottom = Viewport::new();
        let last =
            viewport_row(&mut term, &snap, &bottom, snap.size.rows - 1).expect("last visible row");
        assert_eq!(last.cols, 10, "post-resize rows use the new width");
    }

    #[test]
    fn history_read_trait_backs_viewport() {
        let mut term = Terminal::new(GridSize::new(4, 2)).unwrap();
        // "abcd" fills the row and leaves a pending wrap; the bare LF moves
        // down without a carriage return, so "efgh" wraps at its first
        // character: the wrap scrolls "abcd" out and the trailing LF scrolls
        // "efgh" out, leaving two history lines.
        term.feed(b"abcd\n")
            .expect("viewport history-read fixture feed should succeed for abcd");
        let reader = &mut term as &mut dyn HistoryRead;
        assert_eq!(reader.history_len(), 0);
        term.feed(b"efgh\n")
            .expect("viewport history-read fixture feed should succeed for efgh");
        let reader = &mut term as &mut dyn HistoryRead;
        assert_eq!(reader.history_len(), 2);
        let mut out = Vec::new();
        assert!(reader.read_history_line(0, &mut out));
        assert_eq!(out[0].content, u32::from('a'), "scrolled row is the oldest");
        assert!(!reader.read_history_line(9, &mut out));
        assert!(reader.read_history_line(0, &mut out));
    }
}

//! Production compact terminal engine: one row/page authority for the live
//! primary screen and scrollback. Alternate screen is an independent grid.
//! Full-screen scroll appends/moves `CompactRow` descriptors; cells are not
//! converted from Alacritty or scanned solely to enter history.

use std::collections::VecDeque;
use std::mem;
use std::ops::Range;
use std::sync::Arc;

use vte::ansi::cursor_icon::CursorIcon;
use vte::ansi::{
    Attr, CharsetIndex, ClearMode, CursorShape, CursorStyle, Handler, Hyperlink, KeyboardModes,
    KeyboardModesApplyBehavior, LineClearMode, Mode, ModifyOtherKeys, NamedMode, NamedPrivateMode,
    PrivateMode, Rgb, ScpCharPath, ScpUpdateMode, StandardCharset, TabulationClearMode,
};

use crate::compact::flags;
#[cfg(test)]
use crate::compact::row::{CompactPage, PAGE_LINES};
use crate::compact::row::{CompactRow, RowExtras};
use crate::compact::state::{
    COLOR_COUNT, CursorPos, KEYBOARD_STACK_MAX, KittyApply, ModeBits, Pen, SavedCursor,
    TITLE_STACK_MAX, TabStops, clamp_scroll_region, default_cursor_style, map_charset,
};
use crate::compact::width::char_width;
use crate::side_tables::{GraphemeTable, HyperlinkTable, StyleRemap, StyleTable};
use crate::{
    Cell, CombiningMarks, CursorSnapshot, DamageKind, GridSize, HistoryRead, HyperlinkInfo,
    NormalizedSnapshot, SnapshotHyperlink, Style, TerminalError, TerminalMode,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Damage {
    Clean,
    Partial,
    Full,
}

struct Screen {
    history: VecDeque<CompactRow>,
    active: VecDeque<CompactRow>,
    cursor: CursorPos,
    saved: SavedCursor,
    max_history: usize,
}

impl Screen {
    fn new(size: GridSize, max_history: usize) -> Self {
        Self {
            history: VecDeque::new(),
            active: (0..size.rows)
                .map(|_| CompactRow::blank(size.cols))
                .collect(),
            cursor: CursorPos::default(),
            saved: SavedCursor::default(),
            max_history,
        }
    }

    fn reset(&mut self, size: GridSize) {
        self.history.clear();
        self.active = (0..size.rows)
            .map(|_| CompactRow::blank(size.cols))
            .collect();
        self.cursor = CursorPos::default();
        self.saved = SavedCursor::default();
    }
}

/// Compact VT engine. Live screen and scrollback share `CompactRow`.
pub struct CompactEngine {
    size: GridSize,
    primary: Screen,
    alternate: Screen,
    alt_active: bool,
    scroll_region: Range<u16>,
    modes: ModeBits,
    pen: Pen,
    charsets: [StandardCharset; 4],
    active_charset: CharsetIndex,
    tabs: TabStops,
    styles: StyleTable,
    graphemes: GraphemeTable,
    hyperlinks: HyperlinkTable,
    colors: [Option<Rgb>; COLOR_COUNT],
    title: Option<String>,
    title_stack: Vec<Option<String>>,
    keyboard_stack: Vec<KeyboardModes>,
    inactive_keyboard_stack: Vec<KeyboardModes>,
    cursor_style: Option<CursorStyle>,
    default_cursor_blink: bool,
    damage: Damage,
    damaged_rows: Vec<bool>,
    pending_replies: Vec<String>,
    capture_history: bool,
    recycled_rows: Vec<Arc<[Cell]>>,
    #[cfg(test)]
    next_page_generation: u64,
}

pub(crate) struct EngineStyleRemap {
    pub(crate) pen: Pen,
    pub(crate) primary_saved: SavedCursor,
    pub(crate) alternate_saved: SavedCursor,
    pub(crate) primary_active: VecDeque<CompactRow>,
    pub(crate) primary_history: VecDeque<CompactRow>,
    pub(crate) alternate_active: VecDeque<CompactRow>,
    pub(crate) alternate_history: VecDeque<CompactRow>,
    pub(crate) staged_table: StyleTable,
}

impl CompactEngine {
    pub fn new(size: GridSize) -> Result<Self, TerminalError> {
        Self::new_with_history(size, 100_000)
    }

    pub fn new_with_history(size: GridSize, max_history: usize) -> Result<Self, TerminalError> {
        if size.cols == 0 {
            return Err(TerminalError::ZeroColumns);
        }
        if size.rows == 0 {
            return Err(TerminalError::ZeroRows);
        }
        Ok(Self {
            size,
            primary: Screen::new(size, max_history),
            alternate: Screen::new(size, 0),
            alt_active: false,
            scroll_region: 0..size.rows,
            modes: ModeBits::default_live(),
            pen: Pen::default(),
            charsets: [StandardCharset::Ascii; 4],
            active_charset: CharsetIndex::G0,
            tabs: TabStops::new(usize::from(size.cols)),
            styles: StyleTable::new(),
            graphemes: GraphemeTable::new(),
            hyperlinks: HyperlinkTable::new(),
            colors: [None; COLOR_COUNT],
            title: None,
            title_stack: Vec::new(),
            keyboard_stack: Vec::new(),
            inactive_keyboard_stack: Vec::new(),
            cursor_style: None,
            default_cursor_blink: false,
            damage: Damage::Full,
            damaged_rows: vec![true; usize::from(size.rows)],
            pending_replies: Vec::new(),
            capture_history: max_history != 0,
            recycled_rows: if max_history == 0 {
                Vec::new()
            } else {
                (0..1_024)
                    .map(|_| CompactRow::blank(size.cols).cells)
                    .collect()
            },
            #[cfg(test)]
            next_page_generation: 1,
        })
    }

    pub(crate) fn recycle_rows(&mut self, rows: Vec<Arc<[Cell]>>) {
        let cols = usize::from(self.size.cols);
        let remaining = 1_024usize.saturating_sub(self.recycled_rows.len());
        self.recycled_rows.extend(
            rows.into_iter()
                .filter(|cells| cells.len() == cols)
                .take(remaining),
        );
    }

    /// Prepend stored scrollback rows into primary history for same-width
    /// taller resize. Zero-copy: moves `CompactRow` descriptors; preserves
    /// occupancy/wrapped/generation. No-op in alternate screen.
    pub(crate) fn prepend_storage_history(&mut self, rows: Vec<CompactRow>) {
        if rows.is_empty() || self.alt_active {
            return;
        }
        // Only primary history is eligible; alternate is independent.
        self.primary.history.extend(rows);
    }

    fn blank_row(&mut self) -> CompactRow {
        while let Some(cells) = self.recycled_rows.pop() {
            if cells.len() != usize::from(self.size.cols) {
                continue;
            }
            return CompactRow {
                cells,
                cols: self.size.cols,
                occupancy: 0,
                first_occupied: self.size.cols,
                wrapped: false,
                generation: 1,
                extras: None,
            };
        }
        CompactRow::blank(self.size.cols)
    }

    pub fn set_history_limit(&mut self, max_history: usize) {
        self.primary.max_history = max_history;
        self.capture_history = max_history != 0;
        if !self.capture_history {
            self.primary.history.clear();
        } else {
            self.trim_history();
        }
    }

    pub const fn size(&self) -> GridSize {
        self.size
    }

    pub fn title(&self) -> Option<&str> {
        self.title.as_deref()
    }

    pub fn set_window_title(&mut self, title: Option<String>) {
        self.title = title.map(|text| {
            if text.len() > 1024 {
                text.chars().take(1024).collect()
            } else {
                text
            }
        });
    }

    pub fn modes(&self) -> Vec<TerminalMode> {
        self.modes.to_terminal_modes()
    }

    pub fn has_mode(&self, mode: TerminalMode) -> bool {
        self.modes.has_terminal_mode(mode)
    }

    pub fn cursor(&self) -> CursorSnapshot {
        let pos = self.screen().cursor;
        CursorSnapshot {
            row: pos.row,
            col: pos.col,
            wrap_pending: pos.wrap_pending,
        }
    }

    pub fn take_damage(&mut self) -> DamageKind {
        let kind = match self.damage {
            Damage::Clean => DamageKind::Clean,
            Damage::Partial => DamageKind::Partial,
            Damage::Full => DamageKind::Full,
        };
        self.damage = Damage::Clean;
        for flag in &mut self.damaged_rows {
            *flag = false;
        }
        kind
    }

    pub(crate) fn damage_kind(&self) -> DamageKind {
        match self.damage {
            Damage::Clean => DamageKind::Clean,
            Damage::Partial => DamageKind::Partial,
            Damage::Full => DamageKind::Full,
        }
    }

    pub(crate) fn damaged_rows(&self) -> &[bool] {
        &self.damaged_rows
    }

    pub(crate) fn touch_cursor_damage(&mut self) {
        self.mark_cursor();
    }

    pub fn take_replies(&mut self) -> Vec<String> {
        mem::take(&mut self.pending_replies)
    }

    pub fn set_default_cursor_blink(&mut self, blinking: bool) {
        self.default_cursor_blink = blinking;
    }

    pub fn cursor_style(&self) -> CursorStyle {
        self.cursor_style
            .unwrap_or_else(|| default_cursor_style(self.default_cursor_blink))
    }

    /// Drain rows that just left the primary viewport via full-screen scroll.
    /// Each item is a row descriptor (`Arc` bump), not a cell scan/copy.
    pub fn take_scrolled_rows(&mut self) -> Vec<CompactRow> {
        self.primary.history.drain(..).collect()
    }

    #[cfg(test)]
    pub fn history_pages(&self) -> Vec<CompactPage> {
        let cols = self.size.cols;
        let mut pages = Vec::new();
        let mut generation = self.next_page_generation;
        for chunk in self
            .primary
            .history
            .iter()
            .cloned()
            .collect::<Vec<_>>()
            .chunks(PAGE_LINES)
        {
            pages.push(CompactPage::new(chunk.to_vec(), cols, generation));
            generation = generation.wrapping_add(1);
        }
        pages
    }

    pub fn history_len(&self) -> usize {
        self.primary.history.len()
    }

    pub fn history_line_cols(&self, index: usize) -> Option<usize> {
        self.primary
            .history
            .get(index)
            .map(|row| usize::from(row.cols))
    }

    pub fn read_history_line(&mut self, index: usize, out: &mut Vec<Cell>) -> bool {
        let Some(row) = self.primary.history.get(index) else {
            return false;
        };
        out.clear();
        out.resize(usize::from(row.cols), Cell::default());
        let first = usize::from(row.first_occupied);
        let end = usize::from(row.occupancy);
        if first < end {
            out[first..end].copy_from_slice(&row.cells[first..end]);
        }
        true
    }

    pub fn visible_rows(&self) -> Vec<Vec<Cell>> {
        self.screen()
            .active
            .iter()
            .map(|row| {
                let mut cells = vec![Cell::default(); usize::from(row.cols)];
                let first = usize::from(row.first_occupied);
                let end = usize::from(row.occupancy);
                if first < end {
                    cells[first..end].copy_from_slice(&row.cells[first..end]);
                }
                cells
            })
            .collect()
    }

    #[cfg(test)]
    pub fn push_history_line(&mut self, cols: u16, cells: &[Cell]) {
        let mut owned = cells.to_vec();
        owned.resize(usize::from(cols), Cell::default());
        self.primary
            .history
            .push_back(CompactRow::new(owned, false));
        self.trim_history();
    }

    pub fn clear_history(&mut self) {
        self.primary.history.clear();
    }

    pub fn global_styles(&self) -> &[crate::Style] {
        self.styles.as_slice()
    }

    pub fn visible_cells_global(&self) -> Vec<Cell> {
        let cols = usize::from(self.size.cols);
        let rows = usize::from(self.size.rows);
        let mut cells = Vec::with_capacity(cols * rows);
        for row in self.screen().active.iter() {
            for col in 0..cols {
                let mut cell = if col < usize::from(row.first_occupied)
                    || col >= usize::from(row.occupancy)
                {
                    Cell::default()
                } else {
                    row.cells[col]
                };
                cell.flags &= !flags::COMBINING;
                cells.push(cell);
            }
        }
        debug_assert_eq!(cells.len(), cols * rows);
        cells
    }

    pub fn replace_style_table(&mut self, styles: &[Style]) -> Result<(), TerminalError> {
        let table = StyleTable::from_exact(styles)?;
        self.styles = table;
        Ok(())
    }

    pub(crate) fn style_count(&self) -> usize {
        self.styles.len()
    }

    pub(crate) fn style_epoch(&self) -> u64 {
        self.styles.epoch()
    }



    pub(crate) fn stage_style_remap(
        &self,
        remap: &StyleRemap,
    ) -> Result<EngineStyleRemap, TerminalError> {
        remap.validate_for(self.styles.as_slice())?;
        let staged_table = self.styles.stage_remapped(remap)?;

        // Remap pens: preserve style_dirty and all other fields, only style id changes.
        let mut pen = self.pen;
        pen.style = remap.map(pen.style)?;
        let mut primary_saved = self.primary.saved;
        primary_saved.pen.style = remap.map(primary_saved.pen.style)?;
        let mut alternate_saved = self.alternate.saved;
        alternate_saved.pen.style = remap.map(alternate_saved.pen.style)?;

        fn remap_rows(
            src: &VecDeque<CompactRow>,
            remap: &StyleRemap,
        ) -> Result<VecDeque<CompactRow>, TerminalError> {
            let mut out = VecDeque::with_capacity(src.len());
            for row in src.iter() {
                let cols = usize::from(row.cols);
                let len = row.cells.len();
                let first = usize::from(row.first_occupied.min(row.cols));
                let mut end = usize::from(row.occupancy.min(row.cols));
                if row.wrapped && end < cols {
                    end = cols;
                }
                let occupied_empty = first >= end || first >= len;
                let occ_start = if occupied_empty { cols } else { first };
                let occ_end = if occupied_empty { cols } else { end.min(len) };

                // Clone backing into new owned buffer; preserve metadata exactly.
                let mut cells: Vec<Cell> = row.cells.as_ref().to_vec();
                debug_assert_eq!(cells.len(), len);
                let mut changed = false;
                if !occupied_empty {
                    for cell in cells.iter_mut().take(occ_end).skip(occ_start) {
                        let old = cell.style;
                        let mapped = remap.map(old)?;
                        changed |= mapped != old;
                        cell.style = mapped;
                    }
                    // Sanitize outside occupied range to default cell (covers stale dead ids).
                    for idx in 0..occ_start.min(cells.len()) {
                        if !cells[idx].is_default() {
                            cells[idx] = Cell::default();
                            changed = true;
                        } else if cells[idx].style != 0 {
                            cells[idx].style = 0;
                            changed = true;
                        }
                    }
                    for cell in cells.iter_mut().skip(occ_end) {
                        if !cell.is_default() {
                            *cell = Cell::default();
                            changed = true;
                        } else if cell.style != 0 {
                            cell.style = 0;
                            changed = true;
                        }
                    }
                } else {
                    // Visually empty but may hold stale styles; sanitize entire row.
                    for cell in cells.iter_mut() {
                        if !cell.is_default() || cell.style != 0 {
                            *cell = Cell::default();
                            changed = true;
                        }
                    }
                }

                let generation = if changed {
                    row.generation.wrapping_add(1)
                } else {
                    row.generation
                };
                let mut new_row = CompactRow::from_parts(
                    cells.into(),
                    row.cols,
                    row.occupancy,
                    row.first_occupied,
                    row.wrapped,
                    generation,
                );
                // Preserve RowExtras exactly (no filtering).
                new_row.extras = row.extras.clone();
                // If content changed and generation wasn't bumped (should have), ensure bump already applied.
                // Generation already set via from_parts; no extra bump needed.
                out.push_back(new_row);
            }
            Ok(out)
        }

        let primary_active = remap_rows(&self.primary.active, remap)?;
        let primary_history = remap_rows(&self.primary.history, remap)?;
        let alternate_active = remap_rows(&self.alternate.active, remap)?;
        let alternate_history = remap_rows(&self.alternate.history, remap)?;

        Ok(EngineStyleRemap {
            pen,
            primary_saved,
            alternate_saved,
            primary_active,
            primary_history,
            alternate_active,
            alternate_history,
            staged_table,
        })
    }

    pub(crate) fn commit_style_remap(&mut self, staged: EngineStyleRemap) {
        // Infallible: field assignments/swaps only, no allocation or fallible calls.
        self.pen = staged.pen;
        self.primary.saved = staged.primary_saved;
        self.alternate.saved = staged.alternate_saved;
        self.primary.active = staged.primary_active;
        self.primary.history = staged.primary_history;
        self.alternate.active = staged.alternate_active;
        self.alternate.history = staged.alternate_history;
        self.styles = staged.staged_table;
        self.recycled_rows.clear();
        self.mark_full();
        // Row generations already bumped in staged rows where backing changed.
    }

    pub(crate) fn collect_live_style_ids(
        &self,
        out: &mut std::collections::BTreeSet<u16>,
    ) {
        out.insert(self.pen.style);
        out.insert(self.primary.saved.pen.style);
        out.insert(self.alternate.saved.pen.style);
        for row in self
            .primary
            .active
            .iter()
            .chain(self.primary.history.iter())
            .chain(self.alternate.active.iter())
            .chain(self.alternate.history.iter())
        {
            let cols = usize::from(row.cols);
            let first = usize::from(row.first_occupied.min(row.cols));
            let mut end = usize::from(row.occupancy.min(row.cols));
            if row.wrapped && end < cols {
                end = cols;
            }
            if first >= end || first >= row.cells.len() {
                out.insert(0);
                continue;
            }
            for cell in &row.cells[first..end.min(row.cells.len())] {
                out.insert(cell.style);
            }
        }
    }

    fn validate_visible_grid(
        &self,
        cells: &[Cell],
        styles_len: usize,
    ) -> Result<(), TerminalError> {
        let cols = usize::from(self.size.cols);
        let rows = usize::from(self.size.rows);
        if cells.len() != cols * rows {
            return Err(TerminalError::RestoreSizeMismatch);
        }
        for cell in cells {
            if usize::from(cell.style) >= styles_len {
                return Err(TerminalError::RestoreStyleTable);
            }
        }
        Ok(())
    }

    fn apply_visible_grid(
        &mut self,
        cells: &[Cell],
        combining_marks: &[CombiningMarks],
        hyperlinks: &[SnapshotHyperlink],
    ) {
        let cols = usize::from(self.size.cols);
        let rows = usize::from(self.size.rows);
        self.graphemes = GraphemeTable::new();
        self.hyperlinks = HyperlinkTable::new();
        for row in 0..rows {
            let start = row * cols;
            let slice = &cells[start..start + cols];
            let mut built = CompactRow::new(slice.to_vec(), false);
            if let Some(last) = slice.last() {
                if last.flags & flags::WRAPLINE != 0 {
                    built.wrapped = true;
                }
            }
            self.screen_mut().active[row] = built;
        }
        for marks in combining_marks {
            let row = (marks.cell_index as usize) / cols;
            let col = (marks.cell_index as usize) % cols;
            if row >= rows {
                continue;
            }
            let id = self.graphemes.intern(marks.codepoints.clone());
            let row_ref = &mut self.screen_mut().active[row];
            if let Some(cell) = row_ref.cells_mut().get_mut(col) {
                cell.flags |= flags::COMBINING;
            }
            row_ref.extras_mut().combining.insert(col as u16, id);
            row_ref.bump();
        }
        for link in hyperlinks {
            let row = (link.cell_index as usize) / cols;
            let col = (link.cell_index as usize) % cols;
            if row >= rows {
                continue;
            }
            let id = self
                .hyperlinks
                .intern(link.id.clone().unwrap_or_default(), link.uri.clone());
            let row_ref = &mut self.screen_mut().active[row];
            row_ref.extras_mut().hyperlinks.insert(col as u16, id);
            row_ref.bump();
        }
        self.mark_full();
    }

    pub fn snapshot(&self) -> NormalizedSnapshot {
        let cols = usize::from(self.size.cols);
        let rows = usize::from(self.size.rows);
        let source_styles = self.styles.as_slice();
        let mut style_remap = vec![u16::MAX; source_styles.len()];
        let mut styles = Vec::with_capacity(source_styles.len());
        if let Some(default) = source_styles.first() {
            style_remap[0] = 0;
            styles.push(default.clone());
        }
        let mut cells = Vec::with_capacity(cols * rows);
        let mut combining_marks = Vec::new();
        let mut hyperlinks = Vec::new();
        for (row_idx, row) in self.screen().active.iter().enumerate() {
            for (col_idx, source_cell) in row.cells.iter().enumerate() {
                let mut cell = if col_idx < usize::from(row.first_occupied)
                    || col_idx >= usize::from(row.occupancy)
                {
                    Cell::default()
                } else {
                    *source_cell
                };
                let source_style = usize::from(cell.style);
                cell.style = if source_style < style_remap.len() {
                    if style_remap[source_style] == u16::MAX {
                        let mapped =
                            u16::try_from(styles.len()).expect("snapshot style table overflow u16");
                        style_remap[source_style] = mapped;
                        styles.push(source_styles[source_style].clone());
                    }
                    style_remap[source_style]
                } else {
                    0
                };

                let cell_index = (row_idx * cols + col_idx) as u32;
                if cell.flags & flags::COMBINING != 0 {
                    if let Some(extras) = row.extras.as_ref() {
                        if let Some(&gid) = extras.combining.get(&(col_idx as u16)) {
                            if let Some(marks) = self.graphemes.get(gid) {
                                combining_marks.push(CombiningMarks {
                                    cell_index,
                                    codepoints: marks.to_vec(),
                                });
                            }
                        }
                    }
                }
                if let Some(extras) = row.extras.as_ref() {
                    if let Some(&hid) = extras.hyperlinks.get(&(col_idx as u16)) {
                        if let Some(link) = self.hyperlinks.get(hid) {
                            hyperlinks.push(SnapshotHyperlink {
                                cell_index,
                                id: explicit_hyperlink_id(&link.id),
                                uri: link.uri.clone(),
                            });
                        }
                    }
                }
                cell.flags &= !flags::COMBINING;
                cells.push(cell);
            }
        }
        NormalizedSnapshot {
            size: self.size,
            cursor: self.cursor(),
            cells,
            styles,
            combining_marks,
            hyperlinks,
            modes: self.modes(),
        }
    }

    pub fn restore_visible_grid(
        &mut self,
        cells: &[Cell],
        styles: &[Style],
        combining_marks: &[CombiningMarks],
        hyperlinks: &[SnapshotHyperlink],
    ) -> Result<(), TerminalError> {
        self.validate_visible_grid(cells, styles.len())?;
        let table = StyleTable::from_exact(styles)?;
        self.styles = table;
        self.apply_visible_grid(cells, combining_marks, hyperlinks);
        Ok(())
    }

    pub fn restore_visible_grid_global(
        &mut self,
        cells: &[Cell],
        combining_marks: &[CombiningMarks],
        hyperlinks: &[SnapshotHyperlink],
    ) -> Result<(), TerminalError> {
        let styles_len = self.styles.as_slice().len();
        self.validate_visible_grid(cells, styles_len)?;
        self.apply_visible_grid(cells, combining_marks, hyperlinks);
        Ok(())
    }


    pub fn restore_cursor(&mut self, cursor: CursorSnapshot) -> Result<(), TerminalError> {
        if cursor.row >= self.size.rows || cursor.col >= self.size.cols {
            return Err(TerminalError::RestoreSizeMismatch);
        }
        self.screen_mut().cursor = CursorPos {
            row: cursor.row,
            col: cursor.col,
            wrap_pending: cursor.wrap_pending,
        };
        Ok(())
    }

    pub fn restore_modes(&mut self, modes: &[TerminalMode]) {
        self.modes = ModeBits::default_live();
        for mode in [
            TerminalMode::ShowCursor,
            TerminalMode::LineWrap,
            TerminalMode::AlternateScroll,
            TerminalMode::UrgencyHints,
        ] {
            if !modes.contains(&mode) {
                match mode {
                    TerminalMode::ShowCursor => self.modes.remove(ModeBits::show_cursor()),
                    TerminalMode::LineWrap => self.modes.remove(ModeBits::line_wrap()),
                    TerminalMode::AlternateScroll => {
                        self.modes.remove(ModeBits::alternate_scroll())
                    }
                    TerminalMode::UrgencyHints => self.modes.remove(ModeBits::urgency_hints()),
                    _ => {}
                }
            }
        }
        for mode in modes {
            match *mode {
                TerminalMode::ShowCursor => self.modes.insert(ModeBits::show_cursor()),
                TerminalMode::AppCursor => self.modes.insert(ModeBits::app_cursor()),
                TerminalMode::AppKeypad => self.modes.insert(ModeBits::app_keypad()),
                TerminalMode::MouseReportClick => {
                    self.modes.remove(ModeBits::mouse_mode());
                    self.modes.insert(ModeBits::mouse_report_click());
                }
                TerminalMode::BracketedPaste => self.modes.insert(ModeBits::bracketed_paste()),
                TerminalMode::SgrMouse => {
                    self.modes.remove(ModeBits::utf8_mouse());
                    self.modes.insert(ModeBits::sgr_mouse());
                }
                TerminalMode::MouseMotion => {
                    self.modes.remove(ModeBits::mouse_mode());
                    self.modes.insert(ModeBits::mouse_motion());
                }
                TerminalMode::LineWrap => self.modes.insert(ModeBits::line_wrap()),
                TerminalMode::LineFeedNewLine => self.modes.insert(ModeBits::line_feed_new_line()),
                TerminalMode::Origin => self.modes.insert(ModeBits::origin()),
                TerminalMode::Insert => self.modes.insert(ModeBits::insert_mode()),
                TerminalMode::FocusInOut => self.modes.insert(ModeBits::focus_in_out()),
                TerminalMode::AltScreen => {
                    if !self.alt_active {
                        self.swap_alt();
                    }
                }
                TerminalMode::MouseDrag => {
                    self.modes.remove(ModeBits::mouse_mode());
                    self.modes.insert(ModeBits::mouse_drag());
                }
                TerminalMode::Utf8Mouse => {
                    self.modes.remove(ModeBits::sgr_mouse());
                    self.modes.insert(ModeBits::utf8_mouse());
                }
                TerminalMode::AlternateScroll => self.modes.insert(ModeBits::alternate_scroll()),
                TerminalMode::Vi => {}
                TerminalMode::UrgencyHints => self.modes.insert(ModeBits::urgency_hints()),
                TerminalMode::DisambiguateEscCodes => {
                    self.modes
                        .apply_kitty(KeyboardModes::DISAMBIGUATE_ESC_CODES, KittyApply::Union);
                }
                TerminalMode::ReportEventTypes => {
                    self.modes
                        .apply_kitty(KeyboardModes::REPORT_EVENT_TYPES, KittyApply::Union);
                }
                TerminalMode::ReportAlternateKeys => {
                    self.modes
                        .apply_kitty(KeyboardModes::REPORT_ALTERNATE_KEYS, KittyApply::Union);
                }
                TerminalMode::ReportAllKeysAsEsc => {
                    self.modes
                        .apply_kitty(KeyboardModes::REPORT_ALL_KEYS_AS_ESC, KittyApply::Union);
                }
                TerminalMode::ReportAssociatedText => {
                    self.modes
                        .apply_kitty(KeyboardModes::REPORT_ASSOCIATED_TEXT, KittyApply::Union);
                }
            }
        }
        if !modes.contains(&TerminalMode::AltScreen) && self.alt_active {
            self.swap_alt();
        }
        self.mark_full();
    }

    pub fn hyperlink_at(&self, row: u16, col: u16) -> Option<HyperlinkInfo> {
        let screen = self.screen();
        let row = screen.active.get(usize::from(row))?;
        let extras = row.extras.as_ref()?;
        let id = extras.hyperlinks.get(&col)?;
        let link = self.hyperlinks.get(*id)?;
        Some(HyperlinkInfo {
            id: explicit_hyperlink_id(&link.id),
            uri: link.uri.clone(),
        })
    }

    pub fn resize(&mut self, size: GridSize) -> Result<(), TerminalError> {
        if size.cols == 0 {
            return Err(TerminalError::ZeroColumns);
        }
        if size.rows == 0 {
            return Err(TerminalError::ZeroRows);
        }
        if size == self.size {
            return Ok(());
        }
        let old = self.size;
        self.reflow_screen(true, old, size);
        self.reflow_screen(false, old, size);
        self.size = size;
        self.scroll_region = 0..size.rows;
        self.tabs.resize(usize::from(size.cols));
        self.damaged_rows = vec![true; usize::from(size.rows)];
        self.mark_full();
        Ok(())
    }

    fn reflow_screen(&mut self, primary: bool, old: GridSize, new: GridSize) {
        let screen = if primary {
            &mut self.primary
        } else {
            &mut self.alternate
        };
        // Capture primary cursor state before the grid is rebuilt so a taller
        // primary grid can restore history rows underneath the cursor.
        let history_before = screen.history.len();
        let cursor_row_before = screen.cursor.row;
        let saved_row_before = screen.saved.pos.row;
        let reflow = primary && new.cols != old.cols;
        let mut stream: Vec<CompactRow> = screen.history.drain(..).collect();
        stream.extend(screen.active.drain(..));
        if reflow {
            stream = reflow_rows(stream, new.cols);
        } else if new.cols != old.cols {
            for row in &mut stream {
                resize_row_width(row, new.cols);
            }
        }
        let keep = usize::from(new.rows);
        if stream.len() < keep {
            stream.resize_with(keep, || CompactRow::blank(new.cols));
        }
        let split = stream.len().saturating_sub(keep);
        if primary {
            screen.history.extend(stream.drain(..split));
            while screen.history.len() > screen.max_history {
                screen.history.pop_front();
            }
        } else {
            let _ = stream.drain(..split);
        }
        screen.active = stream.into();
        if primary && new.cols == old.cols && new.rows > old.rows {
            // Same-width height growth: pull the most recent eligible primary
            // history rows back into the visible grid. The stream/split logic
            // above already moved row identities; the cursors must follow the
            // restored prefix so the logical line stays with its row.
            let restored = history_before.saturating_sub(screen.history.len());
            if restored > 0 {
                let restored_u16 = u16::try_from(restored).unwrap_or(u16::MAX);
                screen.cursor.row = cursor_row_before
                    .saturating_add(restored_u16)
                    .min(new.rows.saturating_sub(1));
                screen.saved.pos.row = saved_row_before
                    .saturating_add(restored_u16)
                    .min(new.rows.saturating_sub(1));
            } else {
                screen.cursor.row = cursor_row_before.min(new.rows.saturating_sub(1));
                screen.saved.pos.row = saved_row_before.min(new.rows.saturating_sub(1));
            }
            screen.cursor.col = screen.cursor.col.min(new.cols.saturating_sub(1));
            screen.cursor.wrap_pending = false;
            screen.saved.pos.col = screen.saved.pos.col.min(new.cols.saturating_sub(1));
        } else {
            screen.cursor.row = screen.cursor.row.min(new.rows.saturating_sub(1));
            screen.cursor.col = screen.cursor.col.min(new.cols.saturating_sub(1));
            screen.cursor.wrap_pending = false;
            screen.saved.pos.row = screen.saved.pos.row.min(new.rows.saturating_sub(1));
            screen.saved.pos.col = screen.saved.pos.col.min(new.cols.saturating_sub(1));
        }
    }

    fn screen(&self) -> &Screen {
        if self.alt_active {
            &self.alternate
        } else {
            &self.primary
        }
    }

    fn screen_mut(&mut self) -> &mut Screen {
        if self.alt_active {
            &mut self.alternate
        } else {
            &mut self.primary
        }
    }

    fn trim_history(&mut self) {
        while self.primary.history.len() > self.primary.max_history {
            self.primary.history.pop_front();
        }
    }

    fn mark_full(&mut self) {
        if matches!(self.damage, Damage::Full) {
            return;
        }
        self.damage = Damage::Full;
        self.damaged_rows.fill(true);
    }

    fn mark_row(&mut self, row: u16) {
        if matches!(self.damage, Damage::Full) {
            return;
        }
        self.damage = Damage::Partial;
        if let Some(flag) = self.damaged_rows.get_mut(usize::from(row)) {
            *flag = true;
        }
    }

    fn mark_cursor(&mut self) {
        let row = self.screen().cursor.row;
        self.mark_row(row);
    }

    fn last_col(&self) -> u16 {
        self.size.cols.saturating_sub(1)
    }

    fn origin_row(&self) -> u16 {
        if self.modes.contains(ModeBits::origin()) {
            self.scroll_region.start
        } else {
            0
        }
    }

    fn origin_max_row(&self) -> u16 {
        if self.modes.contains(ModeBits::origin()) {
            self.scroll_region.end.saturating_sub(1)
        } else {
            self.size.rows.saturating_sub(1)
        }
    }

    fn swap_alt(&mut self) {
        if !self.alt_active {
            self.alternate.cursor = self.primary.cursor;
            self.primary.saved = SavedCursor {
                pos: self.primary.cursor,
                pen: self.pen,
                charsets: self.charsets,
                active_charset: self.active_charset,
            };
            for row in &mut self.alternate.active {
                row.reset_blank(self.size.cols);
            }
            self.alternate.history.clear();
        }
        mem::swap(&mut self.keyboard_stack, &mut self.inactive_keyboard_stack);
        let mode = self
            .keyboard_stack
            .last()
            .copied()
            .unwrap_or(KeyboardModes::NO_MODE);
        self.modes.apply_kitty(mode, KittyApply::Replace);
        self.alt_active = !self.alt_active;
        self.modes.toggle_alt();
        self.mark_full();
    }

    fn wrapline(&mut self) {
        if !self.modes.contains(ModeBits::line_wrap()) {
            return;
        }
        let row = self.screen().cursor.row;
        self.screen_mut().active[usize::from(row)].set_wrapped(true);
        self.mark_row(row);
        let next = row.saturating_add(1);
        if next == self.scroll_region.end {
            self.scroll_up_relative(self.scroll_region.start, 1);
        } else if next < self.size.rows {
            self.screen_mut().cursor.row = next;
        }
        self.screen_mut().cursor.col = 0;
        self.screen_mut().cursor.wrap_pending = false;
        self.mark_cursor();
    }

    fn write_at_cursor(&mut self, c: char, extra_flags: u16) {
        let c = map_charset(self.charsets[charset_index(self.active_charset)], c);
        let row = self.screen().cursor.row;
        let col = self.screen().cursor.col;
        self.clear_wide_at(row, col);
        self.pen.intern(&mut self.styles);
        let style = self.pen.style;
        let cell_flags = (self.pen.flags & flags::PEN_ATTRS) | extra_flags;
        let hyperlink = self.pen.hyperlink;
        let row_ref = &mut self.screen_mut().active[usize::from(row)];
        // SAFETY: active-grid rows are exclusively owned; see the ASCII run
        // path for the move/recycle ownership invariant.
        let cells =
            unsafe { row_ref.cells_for_unique_write_range(usize::from(col), usize::from(col) + 1) };
        if let Some(cell) = cells.get_mut(usize::from(col)) {
            cell.content = u32::from(c);
            cell.style = style;
            cell.flags = cell_flags;
        }
        row_ref.clear_extras_at(col);
        if let Some(hid) = hyperlink {
            row_ref.extras_mut().hyperlinks.insert(col, hid);
        }
        self.mark_row(row);
    }

    fn clear_wide_at(&mut self, row: u16, col: u16) {
        let cols = self.size.cols;
        let flags_here = self.screen().active[usize::from(row)]
            .cells
            .get(usize::from(col))
            .map(|cell| cell.flags)
            .unwrap_or(0);
        if flags_here & flags::WIDE_BITS == 0 {
            return;
        }
        {
            let row_ref = &mut self.screen_mut().active[usize::from(row)];
            let cells = row_ref.cells_mut();
            if flags_here & flags::WIDE_CHAR != 0 {
                if let Some(next) = cells.get_mut(usize::from(col.saturating_add(1))) {
                    next.flags &= !flags::WIDE_CHAR_SPACER;
                }
            } else if col > 0 {
                if let Some(prev) = cells.get_mut(usize::from(col - 1)) {
                    prev.flags &= !flags::WIDE_CHAR;
                    prev.content = u32::from(' ');
                }
            }
        }
        if col <= 1 && row > 0 {
            let prev_row = &mut self.screen_mut().active[usize::from(row - 1)];
            if let Some(last) = prev_row
                .cells_mut()
                .get_mut(usize::from(cols.saturating_sub(1)))
            {
                last.flags &= !flags::LEADING_WIDE_CHAR_SPACER;
            }
            self.mark_row(row - 1);
        }
        self.mark_row(row);
    }

    fn attach_combining(&mut self, c: char) {
        let mut col = self.screen().cursor.col;
        if !self.screen().cursor.wrap_pending {
            col = col.saturating_sub(1);
        }
        let row = self.screen().cursor.row;
        if let Some(cell) = self.screen().active[usize::from(row)]
            .cells
            .get(usize::from(col))
        {
            if cell.flags & flags::WIDE_CHAR_SPACER != 0 && col > 0 {
                col -= 1;
            }
        }
        let existing = self.screen().active[usize::from(row)]
            .extras
            .as_ref()
            .and_then(|extras| extras.combining.get(&col).copied())
            .and_then(|id| self.graphemes.get(id).map(|marks| marks.to_vec()))
            .unwrap_or_default();
        let mut cluster = existing;
        cluster.push(u32::from(c));
        let id = self.graphemes.intern(cluster);
        let row_ref = &mut self.screen_mut().active[usize::from(row)];
        if let Some(cell) = row_ref.cells_mut().get_mut(usize::from(col)) {
            cell.flags |= flags::COMBINING;
        }
        row_ref.extras_mut().combining.insert(col, id);
        row_ref.bump();
        self.mark_row(row);
    }

    fn insert_cells(&mut self, count: usize) {
        let row = self.screen().cursor.row;
        let col = usize::from(self.screen().cursor.col);
        let cols = usize::from(self.size.cols);
        let count = count.min(cols.saturating_sub(col));
        if count == 0 {
            return;
        }
        let erased = self.pen.erased_cell(&mut self.styles);
        let row_ref = &mut self.screen_mut().active[usize::from(row)];
        let cells = row_ref.cells_mut();
        for src in (col..cols - count).rev() {
            cells.swap(src + count, src);
        }
        for cell in &mut cells[col..col + count] {
            *cell = erased;
        }
        row_ref.recompute_occupancy();
        row_ref.bump();
        self.mark_row(row);
    }

    #[inline]
    fn scroll_up_relative(&mut self, origin: u16, mut lines: usize) {
        let region_end = self.scroll_region.end;
        let height = usize::from(region_end.saturating_sub(origin));
        lines = lines.min(height);
        if lines == 0 {
            return;
        }
        let to_history = !self.alt_active && origin == 0;
        let capture = self.capture_history;
        if to_history && !capture {
            // No-history path: recycle the leaving allocation in place. This
            // avoids one CompactRow::blank allocation per scroll line and
            // never enqueues into pending history.
            for _ in 0..lines {
                let mut recycled = self
                    .screen_mut()
                    .active
                    .remove(usize::from(origin))
                    .expect("scroll origin is inside active grid");
                if recycled.cells.len() != usize::from(self.size.cols) {
                    recycled = CompactRow::blank(self.size.cols);
                } else {
                    for cell in recycled.cells_mut() {
                        *cell = Cell::default();
                    }
                    recycled.cols = self.size.cols;
                    recycled.occupancy = 0;
                    recycled.wrapped = false;
                    recycled.extras = None;
                    recycled.bump();
                }
                self.screen_mut()
                    .active
                    .insert(usize::from(region_end.saturating_sub(1)), recycled);
            }
            self.mark_full();
            return;
        }
        let full_screen = origin == 0 && usize::from(region_end) == self.screen().active.len();
        if full_screen {
            // Full-screen scroll: pop_front + push_back is O(1) per line.
            // remove(0)+insert(end) would shift the whole 24-row deque.
            for _ in 0..lines {
                let removed = self
                    .screen_mut()
                    .active
                    .pop_front()
                    .expect("scroll origin is inside active grid");
                if to_history {
                    self.primary.history.push_back(removed);
                }
                let blank = self.blank_row();
                self.screen_mut().active.push_back(blank);
            }
        } else {
            for _ in 0..lines {
                let removed = self
                    .screen_mut()
                    .active
                    .remove(usize::from(origin))
                    .expect("scroll origin is inside active grid");
                if to_history {
                    self.primary.history.push_back(removed);
                }
                let blank = self.blank_row();
                self.screen_mut()
                    .active
                    .insert(usize::from(region_end.saturating_sub(1)), blank);
            }
        }
        if to_history {
            self.trim_history();
        }
        self.mark_full();
    }

    fn scroll_down_relative(&mut self, origin: u16, mut lines: usize) {
        let region_end = self.scroll_region.end;
        let height = usize::from(region_end.saturating_sub(origin));
        lines = lines.min(height);
        if lines == 0 {
            return;
        }
        for _ in 0..lines {
            let _ = self
                .screen_mut()
                .active
                .remove(usize::from(region_end.saturating_sub(1)));
            let cols = self.size.cols;
            self.screen_mut()
                .active
                .insert(usize::from(origin), CompactRow::blank(cols));
        }
        self.mark_full();
    }

    pub(crate) fn reset_sgr_attributes(&mut self) {
        self.pen.apply_attr(Attr::Reset);
    }

    pub(crate) fn input_plain_ascii(&mut self, bytes: &[u8]) {
        let mut start = 0usize;
        for (index, byte) in bytes.iter().copied().enumerate() {
            if !matches!(byte, b'\n' | b'\r') {
                continue;
            }
            if start < index {
                self.input_ascii_run(&bytes[start..index]);
            }
            if byte == b'\n' {
                self.linefeed();
            } else {
                self.carriage_return();
            }
            start = index + 1;
        }
        if start < bytes.len() {
            self.input_ascii_run(&bytes[start..]);
        }
    }

    #[inline]
    fn input_ascii_run(&mut self, text: &[u8]) {
        let mut offset = 0;
        while offset < text.len() {
            if self.screen().cursor.wrap_pending {
                self.wrapline();
            }
            let row = self.screen().cursor.row;
            let col = usize::from(self.screen().cursor.col);
            let columns = usize::from(self.size.cols);
            let take = (text.len() - offset).min(columns.saturating_sub(col));
            if take == 0 {
                break;
            }
            let active_row = &self.screen().active[usize::from(row)];
            let occupied_start = col.max(usize::from(active_row.first_occupied));
            let occupied_end = (col + take).min(usize::from(active_row.occupancy));
            let has_wide = occupied_start < occupied_end
                && active_row.cells[occupied_start..occupied_end]
                    .iter()
                    .any(|cell| cell.flags & flags::WIDE_BITS != 0);
            if has_wide {
                self.input(char::from(text[offset]));
                offset += 1;
                continue;
            }
            self.pen.intern(&mut self.styles);
            let style = self.pen.style;
            let pen_flags = self.pen.flags & flags::PEN_ATTRS;
            let hyperlink = self.pen.hyperlink;
            {
                let row_ref = &mut self.screen_mut().active[usize::from(row)];
                {
                    // SAFETY: active-grid rows are exclusively owned. Rows leave the
                    // grid by move, and only successfully compressed (therefore
                    // detached) row allocations enter the recycle pool.
                    let cells = unsafe { row_ref.cells_for_unique_write_range(col, col + take) };
                    for (idx, &byte) in text[offset..offset + take].iter().enumerate() {
                        let cell = &mut cells[col + idx];
                        cell.content = u32::from(byte);
                        cell.style = style;
                        cell.flags = pen_flags;
                    }
                }
                if row_ref.extras.is_some() || hyperlink.is_some() {
                    for idx in 0..take {
                        row_ref.clear_extras_at((col + idx) as u16);
                        if let Some(hid) = hyperlink {
                            row_ref
                                .extras_mut()
                                .hyperlinks
                                .insert((col + idx) as u16, hid);
                        }
                    }
                }
            }
            self.mark_row(row);
            offset += take;
            if col + take < columns {
                self.screen_mut().cursor.col = (col + take) as u16;
            } else {
                self.screen_mut().cursor.col = self.last_col();
                self.screen_mut().cursor.wrap_pending = true;
            }
        }
    }
}

impl HistoryRead for CompactEngine {
    fn history_len(&self) -> usize {
        CompactEngine::history_len(self)
    }

    fn history_line_cols(&self, index: usize) -> Option<usize> {
        CompactEngine::history_line_cols(self, index)
    }

    fn read_history_line(&mut self, index: usize, out: &mut Vec<Cell>) -> bool {
        CompactEngine::read_history_line(self, index, out)
    }
}

// -------------------------------------------------------------------
// Style-cardinality census helpers (Wave A). No compaction.
// -------------------------------------------------------------------

#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct EngineCensusDiag {
    pub active_rows: usize,
    pub history_rows: usize,
    pub pens: usize,
    pub total_cells_scanned: usize,
    pub total_occupied_cells: usize,
}

impl CompactEngine {

    #[cfg(test)]
    #[inline]
    fn occupied_range_from_metadata(row: &crate::compact::row::CompactRow) -> std::ops::Range<usize> {
        let cols = usize::from(row.cols);
        let first = usize::from(row.first_occupied.min(row.cols));
        let mut end = usize::from(row.occupancy.min(row.cols));
        if row.wrapped && end < cols {
            end = cols;
            // Mirror CompactRow::recompute_occupancy wrapped elongation.
            let first = first.min(cols.saturating_sub(1));
            if first >= end || first >= row.cells.len() {
                return 0..0;
            }
            return first..end.min(row.cells.len());
        }
        if first >= end || first >= row.cells.len() {
            return 0..0;
        }
        first..end.min(row.cells.len())
    }

    #[inline]
    #[cfg(test)]
    fn collect_row_styles(cells: &[Cell], range: std::ops::Range<usize>, out: &mut std::collections::BTreeSet<u16>) {
        // Single frozen placed-cell rule: style of every placed cell inside the
        // occupied range, plus style 0 for any blank row. Exhaustive and
        // optimized must both use this or an inline identical loop.
        if range.is_empty() {
            out.insert(0);
            return;
        }
        for cell in &cells[range] { out.insert(cell.style); }
    }
    /// Optimized engine census: current pen + primary.saved + alternate.saved
    /// + primary/alternate active + primary/alternate history. Uses production
    ///   metadata occupancy; exhaustive uses independent scan-plus-wrap.
    #[cfg(test)]
    pub(crate) fn census_engine_styles_optimized(&self, out: &mut std::collections::BTreeSet<u16>) -> EngineCensusDiag {
        let mut diag = EngineCensusDiag { active_rows: 0, history_rows: 0, pens: 3, total_cells_scanned: 0, total_occupied_cells: 0 };
        out.insert(self.pen.style);
        out.insert(self.primary.saved.pen.style);
        out.insert(self.alternate.saved.pen.style);
        for row in self.primary.active.iter().chain(self.primary.history.iter()) {
            diag.total_cells_scanned += row.cells.len();
            let range = Self::occupied_range_from_metadata(row);
            if range.is_empty() {
                Self::collect_row_styles(&row.cells, 0..0, out);
            } else {
                diag.total_occupied_cells += range.len();
                Self::collect_row_styles(&row.cells, range, out);
            }
        }
        diag.active_rows = self.primary.active.len();
        diag.history_rows = self.primary.history.len();
        for row in self.alternate.active.iter().chain(self.alternate.history.iter()) {
            diag.total_cells_scanned += row.cells.len();
            let range = Self::occupied_range_from_metadata(row);
            if range.is_empty() {
                Self::collect_row_styles(&row.cells, 0..0, out);
            } else {
                diag.total_occupied_cells += range.len();
                Self::collect_row_styles(&row.cells, range, out);
            }
        }
        diag.active_rows += self.alternate.active.len();
        diag.history_rows += self.alternate.history.len();
        diag
    }

    /// Exhaustive engine census: independently derived scan, same frozen
    /// `scan_row_for_styles` occupied rule and `collect_row_styles` placed-cell
    /// rule. Intentionally duplicates the loop rather than delegating so
    /// metadata/placement errors diverge between optimized and exhaustive.
    /// Preserves wrapped-to-full-row semantics: exhaustive scan extends to
    /// `cells.len()` when `wrapped` is set, matching storage/optimized.
    #[cfg(test)]
    pub(crate) fn census_engine_styles_exhaustive(&self, out: &mut std::collections::BTreeSet<u16>) -> EngineCensusDiag {
        let mut diag = EngineCensusDiag { active_rows: 0, history_rows: 0, pens: 3, total_cells_scanned: 0, total_occupied_cells: 0 };
        out.insert(self.pen.style);
        out.insert(self.primary.saved.pen.style);
        out.insert(self.alternate.saved.pen.style);
        for row in self.primary.active.iter().chain(self.primary.history.iter()) {
            diag.total_cells_scanned += row.cells.len();
            // Inline exhaustive scan (no delegation) — same predicate, separate code path.
            let range = if row.cells.is_empty() { None } else {
                let first = row.cells.iter().position(|c| !c.is_default()).unwrap_or(row.cells.len());
                if first == row.cells.len() { None } else {
                    let mut last = row.cells.len();
                    while last > first && row.cells[last - 1].is_default() { last -= 1; }
                    if row.wrapped { last = row.cells.len(); }
                    Some(first..last)
                }
            };
            if let Some(r) = range {
                diag.total_occupied_cells += r.len();
                for cell in &row.cells[r] { out.insert(cell.style); }
            } else {
                out.insert(0);
            }
        }
        diag.active_rows = self.primary.active.len();
        diag.history_rows = self.primary.history.len();
        for row in self.alternate.active.iter().chain(self.alternate.history.iter()) {
            diag.total_cells_scanned += row.cells.len();
            let range = if row.cells.is_empty() { None } else {
                let first = row.cells.iter().position(|c| !c.is_default()).unwrap_or(row.cells.len());
                if first == row.cells.len() { None } else {
                    let mut last = row.cells.len();
                    while last > first && row.cells[last - 1].is_default() { last -= 1; }
                    if row.wrapped { last = row.cells.len(); }
                    Some(first..last)
                }
            };
            if let Some(r) = range {
                diag.total_occupied_cells += r.len();
                for cell in &row.cells[r] { out.insert(cell.style); }
            } else {
                out.insert(0);
            }
        }
        diag.active_rows += self.alternate.active.len();
        diag.history_rows += self.alternate.history.len();
        diag
    }
}



impl CompactEngine {
    pub(crate) fn input_text_run(&mut self, text: &str) {
        Handler::input_run(self, text);
    }
}

impl Handler for CompactEngine {
    fn set_cursor_style(&mut self, style: Option<CursorStyle>) {
        self.cursor_style = style;
    }

    fn set_cursor_shape(&mut self, shape: CursorShape) {
        let style = self
            .cursor_style
            .get_or_insert_with(|| default_cursor_style(self.default_cursor_blink));
        style.shape = shape;
    }

    fn input(&mut self, c: char) {
        let width = match char_width(c) {
            Some(width) => width,
            None => return,
        };
        if width == 0 {
            self.attach_combining(c);
            return;
        }
        if self.screen().cursor.wrap_pending {
            self.wrapline();
        }
        let columns = usize::from(self.size.cols);
        if self.modes.contains(ModeBits::insert_mode())
            && usize::from(self.screen().cursor.col) + width < columns
        {
            self.insert_cells(width);
        }
        if width == 1 {
            self.write_at_cursor(c, 0);
        } else {
            if usize::from(self.screen().cursor.col) + 1 >= columns {
                if self.modes.contains(ModeBits::line_wrap()) {
                    self.write_at_cursor(' ', flags::LEADING_WIDE_CHAR_SPACER);
                    self.wrapline();
                } else {
                    self.screen_mut().cursor.wrap_pending = true;
                    return;
                }
            }
            self.write_at_cursor(c, flags::WIDE_CHAR);
            let col = self.screen().cursor.col;
            self.screen_mut().cursor.col = col.saturating_add(1);
            self.write_at_cursor(' ', flags::WIDE_CHAR_SPACER);
        }
        if usize::from(self.screen().cursor.col) + 1 < columns {
            let col = self.screen().cursor.col;
            self.screen_mut().cursor.col = col.saturating_add(1);
        } else {
            self.screen_mut().cursor.wrap_pending = true;
        }
    }

    fn input_run(&mut self, text: &str) {
        if self.modes.contains(ModeBits::insert_mode())
            || self.charsets[charset_index(self.active_charset)] != StandardCharset::Ascii
        {
            for c in text.chars() {
                self.input(c);
            }
            return;
        }
        let mut start = 0;
        for (index, c) in text.char_indices() {
            if !matches!(c, ' '..='~') {
                if start < index {
                    self.input_ascii_run(&text.as_bytes()[start..index]);
                }
                self.input(c);
                start = index + c.len_utf8();
            }
        }
        if start < text.len() {
            self.input_ascii_run(&text.as_bytes()[start..]);
        }
    }

    fn goto(&mut self, line: i32, col: usize) {
        let y_off = i32::from(self.origin_row());
        let max_y = i32::from(self.origin_max_row());
        let row = (line + y_off).clamp(0, max_y) as u16;
        let col = col.min(usize::from(self.last_col())) as u16;
        self.mark_cursor();
        self.screen_mut().cursor.row = row;
        self.screen_mut().cursor.col = col;
        self.screen_mut().cursor.wrap_pending = false;
        self.mark_cursor();
    }

    fn goto_line(&mut self, line: i32) {
        let col = usize::from(self.screen().cursor.col);
        self.goto(line, col);
    }

    fn goto_col(&mut self, col: usize) {
        let row = i32::from(self.screen().cursor.row);
        self.goto(row, col);
    }

    fn insert_blank(&mut self, count: usize) {
        self.insert_cells(count);
    }

    fn move_up(&mut self, rows: usize) {
        let line = i32::from(self.screen().cursor.row).saturating_sub(rows as i32);
        let col = usize::from(self.screen().cursor.col);
        self.goto(line, col);
    }

    fn move_down(&mut self, rows: usize) {
        let line = i32::from(self.screen().cursor.row).saturating_add(rows as i32);
        let col = usize::from(self.screen().cursor.col);
        self.goto(line, col);
    }

    fn identify_terminal(&mut self, intermediate: Option<char>) {
        match intermediate {
            None => self.pending_replies.push("\x1b[?6c".to_string()),
            Some('>') => self.pending_replies.push("\x1b[>0;1;1c".to_string()),
            _ => {}
        }
    }

    fn device_status(&mut self, arg: usize) {
        match arg {
            5 => self.pending_replies.push("\x1b[0n".to_string()),
            6 => {
                let pos = self.screen().cursor;
                self.pending_replies
                    .push(format!("\x1b[{};{}R", pos.row + 1, pos.col + 1));
            }
            _ => {}
        }
    }

    fn move_forward(&mut self, cols: usize) {
        let last = self.last_col();
        let next = self
            .screen()
            .cursor
            .col
            .saturating_add(cols as u16)
            .min(last);
        self.mark_cursor();
        self.screen_mut().cursor.col = next;
        self.screen_mut().cursor.wrap_pending = false;
        self.mark_cursor();
    }

    fn move_backward(&mut self, cols: usize) {
        let next = self.screen().cursor.col.saturating_sub(cols as u16);
        self.mark_cursor();
        self.screen_mut().cursor.col = next;
        self.screen_mut().cursor.wrap_pending = false;
        self.mark_cursor();
    }

    fn move_down_and_cr(&mut self, rows: usize) {
        let line = i32::from(self.screen().cursor.row).saturating_add(rows as i32);
        self.goto(line, 0);
    }

    fn move_up_and_cr(&mut self, rows: usize) {
        let line = i32::from(self.screen().cursor.row).saturating_sub(rows as i32);
        self.goto(line, 0);
    }

    fn put_tab(&mut self, mut count: u16) {
        if self.screen().cursor.wrap_pending {
            self.wrapline();
            return;
        }
        while self.screen().cursor.col < self.size.cols && count != 0 {
            count -= 1;
            let mapped = map_charset(self.charsets[charset_index(self.active_charset)], '\t');
            let row = self.screen().cursor.row;
            let col = self.screen().cursor.col;
            {
                let row_ref = &mut self.screen_mut().active[usize::from(row)];
                if let Some(cell) = row_ref.cells_mut().get_mut(usize::from(col)) {
                    if cell.content == u32::from(' ') {
                        cell.content = u32::from(if mapped == '\t' { ' ' } else { mapped });
                    }
                }
            }
            loop {
                if self.screen().cursor.col + 1 == self.size.cols {
                    break;
                }
                let next = self.screen().cursor.col + 1;
                self.screen_mut().cursor.col = next;
                if self.tabs.is_set(usize::from(next)) {
                    break;
                }
            }
        }
        self.mark_cursor();
    }

    fn backspace(&mut self) {
        if self.screen().cursor.col > 0 {
            let col = self.screen().cursor.col;
            self.screen_mut().cursor.col = col - 1;
            self.screen_mut().cursor.wrap_pending = false;
            self.mark_cursor();
        }
    }

    fn carriage_return(&mut self) {
        self.mark_cursor();
        self.screen_mut().cursor.col = 0;
        self.screen_mut().cursor.wrap_pending = false;
        self.mark_cursor();
    }

    #[inline]
    fn linefeed(&mut self) {
        let next = self.screen().cursor.row.saturating_add(1);
        if next == self.scroll_region.end {
            self.scroll_up_relative(self.scroll_region.start, 1);
        } else if next < self.size.rows {
            self.mark_cursor();
            self.screen_mut().cursor.row = next;
            self.mark_cursor();
        }
    }

    fn bell(&mut self) {}

    fn substitute(&mut self) {}

    fn newline(&mut self) {
        self.linefeed();
        if self.modes.contains(ModeBits::line_feed_new_line()) {
            self.carriage_return();
        }
    }

    fn set_horizontal_tabstop(&mut self) {
        self.tabs.set(usize::from(self.screen().cursor.col), true);
    }

    fn scroll_up(&mut self, rows: usize) {
        self.scroll_up_relative(self.scroll_region.start, rows);
    }

    fn scroll_down(&mut self, rows: usize) {
        self.scroll_down_relative(self.scroll_region.start, rows);
    }

    fn insert_blank_lines(&mut self, count: usize) {
        let origin = self.screen().cursor.row;
        if self.scroll_region.contains(&origin) {
            self.scroll_down_relative(origin, count);
        }
    }

    fn delete_lines(&mut self, count: usize) {
        let origin = self.screen().cursor.row;
        let count = count.min(usize::from(self.size.rows.saturating_sub(origin)));
        if count > 0 && self.scroll_region.contains(&origin) {
            self.scroll_up_relative(origin, count);
        }
    }

    fn erase_chars(&mut self, count: usize) {
        let row = self.screen().cursor.row;
        let start = usize::from(self.screen().cursor.col);
        let end = (start + count).min(usize::from(self.size.cols));
        let erased = self.pen.erased_cell(&mut self.styles);
        self.screen_mut().active[usize::from(row)].fill_erased(start, end, erased);
        self.mark_row(row);
    }

    fn delete_chars(&mut self, count: usize) {
        let row = self.screen().cursor.row;
        let cols = usize::from(self.size.cols);
        let start = usize::from(self.screen().cursor.col);
        let count = count.min(cols);
        let end = (start + count).min(cols.saturating_sub(1).max(start));
        let erased = self.pen.erased_cell(&mut self.styles);
        let row_ref = &mut self.screen_mut().active[usize::from(row)];
        let cells = row_ref.cells_mut();
        let num = cols.saturating_sub(end);
        for offset in 0..num {
            if start + offset < cols && end + offset < cols {
                cells.swap(start + offset, end + offset);
            }
        }
        let clear_from = cols.saturating_sub(count);
        for cell in &mut cells[clear_from..] {
            *cell = erased;
        }
        row_ref.recompute_occupancy();
        row_ref.bump();
        self.mark_row(row);
    }

    fn move_backward_tabs(&mut self, count: u16) {
        for _ in 0..count {
            let col = usize::from(self.screen().cursor.col);
            if let Some(prev) = self.tabs.prev(col) {
                self.screen_mut().cursor.col = prev as u16;
            } else {
                self.screen_mut().cursor.col = 0;
                break;
            }
        }
        self.mark_cursor();
    }

    fn move_forward_tabs(&mut self, count: u16) {
        for _ in 0..count {
            let col = usize::from(self.screen().cursor.col);
            if let Some(next) = self.tabs.next(col) {
                self.screen_mut().cursor.col = next as u16;
            } else {
                self.screen_mut().cursor.col = self.last_col();
                break;
            }
        }
        self.mark_cursor();
    }

    fn save_cursor_position(&mut self) {
        let saved = SavedCursor {
            pos: self.screen().cursor,
            pen: self.pen,
            charsets: self.charsets,
            active_charset: self.active_charset,
        };
        self.screen_mut().saved = saved;
    }

    fn restore_cursor_position(&mut self) {
        self.mark_cursor();
        let saved = self.screen().saved;
        self.screen_mut().cursor = saved.pos;
        self.pen = saved.pen;
        self.charsets = saved.charsets;
        self.active_charset = saved.active_charset;
        self.mark_cursor();
    }

    fn clear_line(&mut self, mode: LineClearMode) {
        if matches!(mode, LineClearMode::Right) && self.screen().cursor.wrap_pending {
            return;
        }
        let row = self.screen().cursor.row;
        let col = usize::from(self.screen().cursor.col);
        let cols = usize::from(self.size.cols);
        let (left, right) = match mode {
            LineClearMode::Right => (col, cols),
            LineClearMode::Left => (0, (col + 1).min(cols)),
            LineClearMode::All => (0, cols),
        };
        let erased = self.pen.erased_cell(&mut self.styles);
        self.screen_mut().active[usize::from(row)].fill_erased(left, right, erased);
        self.mark_row(row);
    }

    fn clear_screen(&mut self, mode: ClearMode) {
        let erased = self.pen.erased_cell(&mut self.styles);
        let cols = usize::from(self.size.cols);
        match mode {
            ClearMode::Above => {
                let cursor = self.screen().cursor;
                if cursor.row > 0 {
                    for row in 0..cursor.row {
                        self.screen_mut().active[usize::from(row)].fill_erased(0, cols, erased);
                    }
                }
                let end = (usize::from(cursor.col) + 1).min(cols);
                self.screen_mut().active[usize::from(cursor.row)].fill_erased(0, end, erased);
            }
            ClearMode::Below => {
                let cursor = self.screen().cursor;
                self.screen_mut().active[usize::from(cursor.row)].fill_erased(
                    usize::from(cursor.col),
                    cols,
                    erased,
                );
                for row in (cursor.row + 1)..self.size.rows {
                    self.screen_mut().active[usize::from(row)].fill_erased(0, cols, erased);
                }
            }
            ClearMode::All => {
                if self.alt_active {
                    for row in &mut self.screen_mut().active {
                        row.fill_erased(0, cols, erased);
                    }
                } else if !self.capture_history {
                    // Recycle allocations: clear each active row in place.
                    for row in &mut self.screen_mut().active {
                        row.fill_erased(0, cols, erased);
                    }
                    // History is already empty when capture is disabled; keep
                    // the invariant without allocating/scanning a new history
                    // descriptor per scroll line.
                } else {
                    let count = self.primary.active.len();
                    for _ in 0..count {
                        let removed = self
                            .primary
                            .active
                            .pop_front()
                            .expect("active grid is non-empty");
                        self.primary.history.push_back(removed);
                        self.primary
                            .active
                            .push_back(CompactRow::blank(self.size.cols));
                    }
                    self.trim_history();
                }
            }
            ClearMode::Saved => self.primary.history.clear(),
        }
        self.mark_full();
    }

    fn clear_tabs(&mut self, mode: TabulationClearMode) {
        match mode {
            TabulationClearMode::Current => {
                self.tabs.set(usize::from(self.screen().cursor.col), false);
            }
            TabulationClearMode::All => self.tabs.clear_all(),
        }
    }

    fn set_tabs(&mut self, interval: u16) {
        self.tabs.clear_all();
        if interval == 0 {
            return;
        }
        let cols = usize::from(self.size.cols);
        let step = usize::from(interval);
        for col in (step..cols).step_by(step) {
            self.tabs.set(col, true);
        }
    }

    fn reset_state(&mut self) {
        if self.alt_active {
            self.swap_alt();
        }
        self.active_charset = CharsetIndex::G0;
        self.charsets = [StandardCharset::Ascii; 4];
        self.cursor_style = None;
        self.primary.reset(self.size);
        self.alternate.reset(self.size);
        self.scroll_region = 0..self.size.rows;
        self.tabs = TabStops::new(usize::from(self.size.cols));
        self.title_stack.clear();
        self.title = None;
        self.keyboard_stack.clear();
        self.inactive_keyboard_stack.clear();
        self.pen = Pen::default();
        self.modes.reset_keep_vi();
        self.mark_full();
    }

    fn reverse_index(&mut self) {
        if self.screen().cursor.row == self.scroll_region.start {
            self.scroll_down(1);
        } else {
            self.mark_cursor();
            let row = self.screen().cursor.row;
            self.screen_mut().cursor.row = row.saturating_sub(1);
            self.mark_cursor();
        }
    }

    fn terminal_attribute(&mut self, attr: Attr) {
        self.pen.apply_attr(attr);
    }

    fn set_mode(&mut self, mode: Mode) {
        match mode {
            Mode::Named(NamedMode::Insert) => self.modes.insert(ModeBits::insert_mode()),
            Mode::Named(NamedMode::LineFeedNewLine) => {
                self.modes.insert(ModeBits::line_feed_new_line())
            }
            Mode::Unknown(_) => {}
        }
    }

    fn unset_mode(&mut self, mode: Mode) {
        match mode {
            Mode::Named(NamedMode::Insert) => {
                self.modes.remove(ModeBits::insert_mode());
                self.mark_full();
            }
            Mode::Named(NamedMode::LineFeedNewLine) => {
                self.modes.remove(ModeBits::line_feed_new_line())
            }
            Mode::Unknown(_) => {}
        }
    }

    fn report_mode(&mut self, mode: Mode) {
        let state = match mode {
            Mode::Named(NamedMode::Insert) => {
                if self.modes.contains(ModeBits::insert_mode()) {
                    1
                } else {
                    2
                }
            }
            Mode::Named(NamedMode::LineFeedNewLine) => {
                if self.modes.contains(ModeBits::line_feed_new_line()) {
                    1
                } else {
                    2
                }
            }
            Mode::Unknown(_) => 0,
        };
        self.pending_replies
            .push(format!("\x1b[{};{}$y", mode.raw(), state));
    }

    fn set_private_mode(&mut self, mode: PrivateMode) {
        let named = match mode {
            PrivateMode::Named(named) => named,
            PrivateMode::Unknown(_) => return,
        };
        match named {
            NamedPrivateMode::UrgencyHints => self.modes.insert(ModeBits::urgency_hints()),
            NamedPrivateMode::SwapScreenAndSetRestoreCursor => {
                if !self.alt_active {
                    self.swap_alt();
                }
            }
            NamedPrivateMode::ShowCursor => self.modes.insert(ModeBits::show_cursor()),
            NamedPrivateMode::CursorKeys => self.modes.insert(ModeBits::app_cursor()),
            NamedPrivateMode::ReportMouseClicks => {
                self.modes.remove(ModeBits::mouse_mode());
                self.modes.insert(ModeBits::mouse_report_click());
            }
            NamedPrivateMode::ReportCellMouseMotion => {
                self.modes.remove(ModeBits::mouse_mode());
                self.modes.insert(ModeBits::mouse_drag());
            }
            NamedPrivateMode::ReportAllMouseMotion => {
                self.modes.remove(ModeBits::mouse_mode());
                self.modes.insert(ModeBits::mouse_motion());
            }
            NamedPrivateMode::ReportFocusInOut => self.modes.insert(ModeBits::focus_in_out()),
            NamedPrivateMode::BracketedPaste => self.modes.insert(ModeBits::bracketed_paste()),
            NamedPrivateMode::SgrMouse => {
                self.modes.remove(ModeBits::utf8_mouse());
                self.modes.insert(ModeBits::sgr_mouse());
            }
            NamedPrivateMode::Utf8Mouse => {
                self.modes.remove(ModeBits::sgr_mouse());
                self.modes.insert(ModeBits::utf8_mouse());
            }
            NamedPrivateMode::AlternateScroll => self.modes.insert(ModeBits::alternate_scroll()),
            NamedPrivateMode::LineWrap => self.modes.insert(ModeBits::line_wrap()),
            NamedPrivateMode::Origin => {
                self.modes.insert(ModeBits::origin());
                self.goto(0, 0);
            }
            NamedPrivateMode::ColumnMode => {
                self.set_scrolling_region(1, None);
                let cols = usize::from(self.size.cols);
                for row in &mut self.screen_mut().active {
                    row.fill_erased(0, cols, Cell::default());
                }
                self.mark_full();
            }
            NamedPrivateMode::BlinkingCursor => {
                let style = self
                    .cursor_style
                    .get_or_insert_with(|| default_cursor_style(self.default_cursor_blink));
                style.blinking = true;
                self.modes.insert(ModeBits::blinking_cursor());
            }
            NamedPrivateMode::SyncUpdate => {}
        }
    }

    fn unset_private_mode(&mut self, mode: PrivateMode) {
        let named = match mode {
            PrivateMode::Named(named) => named,
            PrivateMode::Unknown(_) => return,
        };
        match named {
            NamedPrivateMode::UrgencyHints => self.modes.remove(ModeBits::urgency_hints()),
            NamedPrivateMode::SwapScreenAndSetRestoreCursor => {
                if self.alt_active {
                    self.swap_alt();
                }
            }
            NamedPrivateMode::ShowCursor => self.modes.remove(ModeBits::show_cursor()),
            NamedPrivateMode::CursorKeys => self.modes.remove(ModeBits::app_cursor()),
            NamedPrivateMode::ReportMouseClicks => {
                self.modes.remove(ModeBits::mouse_report_click())
            }
            NamedPrivateMode::ReportCellMouseMotion => self.modes.remove(ModeBits::mouse_drag()),
            NamedPrivateMode::ReportAllMouseMotion => self.modes.remove(ModeBits::mouse_motion()),
            NamedPrivateMode::ReportFocusInOut => self.modes.remove(ModeBits::focus_in_out()),
            NamedPrivateMode::BracketedPaste => self.modes.remove(ModeBits::bracketed_paste()),
            NamedPrivateMode::SgrMouse => self.modes.remove(ModeBits::sgr_mouse()),
            NamedPrivateMode::Utf8Mouse => self.modes.remove(ModeBits::utf8_mouse()),
            NamedPrivateMode::AlternateScroll => self.modes.remove(ModeBits::alternate_scroll()),
            NamedPrivateMode::LineWrap => self.modes.remove(ModeBits::line_wrap()),
            NamedPrivateMode::Origin => self.modes.remove(ModeBits::origin()),
            NamedPrivateMode::ColumnMode => {
                self.set_scrolling_region(1, None);
                let cols = usize::from(self.size.cols);
                for row in &mut self.screen_mut().active {
                    row.fill_erased(0, cols, Cell::default());
                }
                self.mark_full();
            }
            NamedPrivateMode::BlinkingCursor => {
                let style = self
                    .cursor_style
                    .get_or_insert_with(|| default_cursor_style(self.default_cursor_blink));
                style.blinking = false;
                self.modes.remove(ModeBits::blinking_cursor());
            }
            NamedPrivateMode::SyncUpdate => {}
        }
    }

    fn report_private_mode(&mut self, mode: PrivateMode) {
        let state: u8 = match mode {
            PrivateMode::Named(named) => {
                let on = match named {
                    NamedPrivateMode::CursorKeys => self.modes.contains(ModeBits::app_cursor()),
                    NamedPrivateMode::Origin => self.modes.contains(ModeBits::origin()),
                    NamedPrivateMode::LineWrap => self.modes.contains(ModeBits::line_wrap()),
                    NamedPrivateMode::BlinkingCursor => self.cursor_style().blinking,
                    NamedPrivateMode::ShowCursor => self.modes.contains(ModeBits::show_cursor()),
                    NamedPrivateMode::ReportMouseClicks => {
                        self.modes.contains(ModeBits::mouse_report_click())
                    }
                    NamedPrivateMode::ReportCellMouseMotion => {
                        self.modes.contains(ModeBits::mouse_drag())
                    }
                    NamedPrivateMode::ReportAllMouseMotion => {
                        self.modes.contains(ModeBits::mouse_motion())
                    }
                    NamedPrivateMode::ReportFocusInOut => {
                        self.modes.contains(ModeBits::focus_in_out())
                    }
                    NamedPrivateMode::Utf8Mouse => self.modes.contains(ModeBits::utf8_mouse()),
                    NamedPrivateMode::SgrMouse => self.modes.contains(ModeBits::sgr_mouse()),
                    NamedPrivateMode::AlternateScroll => {
                        self.modes.contains(ModeBits::alternate_scroll())
                    }
                    NamedPrivateMode::UrgencyHints => {
                        self.modes.contains(ModeBits::urgency_hints())
                    }
                    NamedPrivateMode::SwapScreenAndSetRestoreCursor => self.alt_active,
                    NamedPrivateMode::BracketedPaste => {
                        self.modes.contains(ModeBits::bracketed_paste())
                    }
                    NamedPrivateMode::SyncUpdate => false,
                    NamedPrivateMode::ColumnMode => {
                        self.pending_replies
                            .push(format!("\x1b[?{};0$y", mode.raw()));
                        return;
                    }
                };
                if on { 1 } else { 2 }
            }
            PrivateMode::Unknown(_) => 0,
        };
        self.pending_replies
            .push(format!("\x1b[?{};{}$y", mode.raw(), state));
    }

    fn set_scrolling_region(&mut self, top: usize, bottom: Option<usize>) {
        let bottom = bottom.unwrap_or(usize::from(self.size.rows));
        if let Some(region) = clamp_scroll_region(top.saturating_sub(1), bottom, self.size.rows) {
            self.scroll_region = region;
            self.goto(0, 0);
        }
    }

    fn set_keypad_application_mode(&mut self) {
        self.modes.insert(ModeBits::app_keypad());
    }

    fn unset_keypad_application_mode(&mut self) {
        self.modes.remove(ModeBits::app_keypad());
    }

    fn set_active_charset(&mut self, index: CharsetIndex) {
        self.active_charset = index;
    }

    fn configure_charset(&mut self, index: CharsetIndex, charset: StandardCharset) {
        self.charsets[charset_index(index)] = charset;
    }

    fn set_color(&mut self, index: usize, color: Rgb) {
        if index < COLOR_COUNT {
            if index != 258 && self.colors[index] != Some(color) {
                self.mark_full();
            }
            self.colors[index] = Some(color);
        }
    }

    fn dynamic_color_sequence(&mut self, prefix: String, index: usize, terminator: &str) {
        let color = self
            .colors
            .get(index)
            .copied()
            .flatten()
            .unwrap_or(Rgb { r: 0, g: 0, b: 0 });
        self.pending_replies.push(format!(
            "\x1b]{};rgb:{:02x}{:02x}/{:02x}{:02x}/{:02x}{:02x}{}",
            prefix, color.r, color.r, color.g, color.g, color.b, color.b, terminator
        ));
    }

    fn reset_color(&mut self, index: usize) {
        if index < COLOR_COUNT {
            if index != 258 && self.colors[index].is_some() {
                self.mark_full();
            }
            self.colors[index] = None;
        }
    }

    fn clipboard_store(&mut self, _: u8, _: &[u8]) {}

    fn clipboard_load(&mut self, _: u8, _: &str) {}

    fn decaln(&mut self) {
        let glyph = Cell {
            content: u32::from('E'),
            style: 0,
            flags: 0,
        };
        let cols = usize::from(self.size.cols);
        for row in &mut self.screen_mut().active {
            row.fill_erased(0, cols, glyph);
        }
        self.mark_full();
    }

    fn push_title(&mut self) {
        if self.title_stack.len() >= TITLE_STACK_MAX {
            self.title_stack.remove(0);
        }
        self.title_stack.push(self.title.clone());
    }

    fn pop_title(&mut self) {
        if let Some(title) = self.title_stack.pop() {
            self.title = title;
        }
    }

    fn text_area_size_pixels(&mut self) {}

    fn text_area_size_chars(&mut self) {
        self.pending_replies
            .push(format!("\x1b[8;{};{}t", self.size.rows, self.size.cols));
    }

    fn set_hyperlink(&mut self, link: Option<Hyperlink>) {
        self.pen.hyperlink = link.map(|link| {
            self.hyperlinks
                .intern(link.id.unwrap_or_default(), link.uri)
        });
    }

    fn set_mouse_cursor_icon(&mut self, _: CursorIcon) {}

    fn report_keyboard_mode(&mut self) {
        let bits = self
            .keyboard_stack
            .last()
            .copied()
            .unwrap_or(KeyboardModes::NO_MODE)
            .bits();
        self.pending_replies.push(format!("\x1b[?{bits}u"));
    }

    fn push_keyboard_mode(&mut self, mode: KeyboardModes) {
        if self.keyboard_stack.len() >= KEYBOARD_STACK_MAX {
            self.keyboard_stack.remove(0);
        }
        self.keyboard_stack.push(mode);
        self.modes.apply_kitty(mode, KittyApply::Replace);
    }

    fn pop_keyboard_modes(&mut self, to_pop: u16) {
        let new_len = self
            .keyboard_stack
            .len()
            .saturating_sub(usize::from(to_pop));
        self.keyboard_stack.truncate(new_len);
        let mode = self
            .keyboard_stack
            .last()
            .copied()
            .unwrap_or(KeyboardModes::NO_MODE);
        self.modes.apply_kitty(mode, KittyApply::Replace);
    }

    fn set_keyboard_mode(&mut self, mode: KeyboardModes, behavior: KeyboardModesApplyBehavior) {
        let apply = match behavior {
            KeyboardModesApplyBehavior::Replace => KittyApply::Replace,
            KeyboardModesApplyBehavior::Union => KittyApply::Union,
            KeyboardModesApplyBehavior::Difference => KittyApply::Difference,
        };
        self.modes.apply_kitty(mode, apply);
    }

    fn set_modify_other_keys(&mut self, _: ModifyOtherKeys) {}

    fn report_modify_other_keys(&mut self) {
        self.pending_replies.push("\x1b[>4;0m".to_string());
    }

    fn set_scp(&mut self, _: ScpCharPath, _: ScpUpdateMode) {}
}

fn charset_index(index: CharsetIndex) -> usize {
    match index {
        CharsetIndex::G0 => 0,
        CharsetIndex::G1 => 1,
        CharsetIndex::G2 => 2,
        CharsetIndex::G3 => 3,
    }
}

fn explicit_hyperlink_id(id: &str) -> Option<String> {
    if id.is_empty() || id.ends_with("_alacritty") {
        None
    } else {
        Some(id.to_owned())
    }
}

fn resize_row_width(row: &mut CompactRow, cols: u16) {
    let mut cells = row.cells.as_ref().to_vec();
    cells.resize(usize::from(cols), Cell::default());
    let wrapped = row.wrapped && cols >= row.cols;
    *row = CompactRow::new(cells, wrapped).with_generation(row.generation.wrapping_add(1));
}

fn reflow_rows(rows: Vec<CompactRow>, cols: u16) -> Vec<CompactRow> {
    let mut out = Vec::new();
    let mut pending: Vec<Cell> = Vec::new();
    let mut pending_extras = RowExtras::default();
    let flush = |pending: &mut Vec<Cell>,
                 extras: &mut RowExtras,
                 out: &mut Vec<CompactRow>,
                 wrapped: bool| {
        if pending.is_empty() {
            return;
        }
        pending.resize(usize::from(cols), Cell::default());
        let mut row = CompactRow::new(mem::take(pending), wrapped);
        if !extras.is_empty() {
            row.extras = Some(std::sync::Arc::new(mem::take(extras)));
        }
        out.push(row);
    };
    for src in rows {
        let src_cols = usize::from(src.cols);
        for col in 0..src_cols {
            if pending.len() == usize::from(cols) {
                flush(&mut pending, &mut pending_extras, &mut out, true);
            }
            let cell = src.cells.get(col).copied().unwrap_or_default();
            if let Some(extras) = src.extras.as_ref() {
                if let Some(&gid) = extras.combining.get(&(col as u16)) {
                    pending_extras.combining.insert(pending.len() as u16, gid);
                }
                if let Some(&hid) = extras.hyperlinks.get(&(col as u16)) {
                    pending_extras.hyperlinks.insert(pending.len() as u16, hid);
                }
            }
            pending.push(cell);
        }
        if !src.wrapped {
            flush(&mut pending, &mut pending_extras, &mut out, false);
        }
    }
    flush(&mut pending, &mut pending_extras, &mut out, false);
    out
}

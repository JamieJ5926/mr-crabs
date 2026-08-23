//! App-layer input dock: derived snapshot of the live prompt row.
//!
//! [`InputDockSnapshot`] is rebuilt from [`NormalizedSnapshot`] plus
//! [`SemanticPromptState`]. It never owns a command `String`. Hidden is the
//! fail-closed default: missing OSC 133, alt-screen, mouse reporting,
//! palette, scrollback, or a right-only prompt all omit the overlay.

use std::sync::Arc;

use mr_crabs_element::CellMetrics;
use mr_crabs_protocols::semantic_prompt::PromptKind;
use mr_crabs_protocols::shell::SemanticPromptState;
use mr_crabs_terminal::{
    Cell, CombiningMarks, CursorState, DamageKind, FrameDelta, GridSize, NormalizedSnapshot,
    RowDelta, Style, TerminalMode, TerminalViewport, batch_runs,
};

use super::pane::{PaneId, PaneModel};

/// Derived dock visibility. Never stored as a line buffer.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InputDockState {
    Hidden,
    ShellInputActive,
}

/// Half-open `[start_col, end_col)` on snapshot row `row`.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DockSourceSpan {
    pub row: u16,
    pub start_col: u16,
    pub end_col: u16,
}

impl DockSourceSpan {
    pub fn col_count(self) -> u16 {
        self.end_col.saturating_sub(self.start_col).max(1)
    }
}

/// Cursor of the projected input, in source-column space.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DockCursor {
    pub source_col: u16,
    pub visible: bool,
    pub wrap_pending: bool,
}

/// Footer label: OSC 7 basename or `"~"`. Never parsed as a command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DockFooter {
    pub label: Arc<str>,
}

/// Layout-independent dock snapshot, safe to retain across Clean frames.
#[derive(Clone, Debug, PartialEq)]
pub struct InputDockSnapshot {
    pub state: InputDockState,
    pub pane_id: PaneId,
    pub source: DockSourceSpan,
    pub cells: Arc<[Cell]>,
    pub styles: Arc<[Style]>,
    pub combining: Arc<[CombiningMarks]>,
    pub cursor: DockCursor,
    pub footer: DockFooter,
    pub prompt_kind: Option<PromptKind>,
    pub generation: u64,
}

/// Cells, styles, and combining marks copied from one source span.
#[derive(Clone, Debug, PartialEq)]
pub struct DockSpanProjection {
    pub cells: Arc<[Cell]>,
    pub styles: Arc<[Style]>,
    pub combining: Arc<[CombiningMarks]>,
}

/// Window-space mapping from a dock cell hit back onto the source row.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DockCoordinateMap {
    pub pane_id: PaneId,
    pub metrics: CellMetrics,
    pub pane_origin: PointF,
    pub source_row: u16,
    pub source_start_col: u16,
    pub projected_cols: u16,
    pub cell_origin: PointF,
}

/// Pixel point in window space (f32, no GPUI types in the model).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PointF {
    pub x: f32,
    pub y: f32,
}

/// Hit classification for a window-space pointer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DockHit {
    Miss,
    Separator,
    Footer,
    Chevron,
    Cells { col: u16, frac_x: f32 },
}

/// Pixel bounds in window space.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DockBounds {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl DockBounds {
    pub fn contains(self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.x + self.width && y >= self.y && y < self.y + self.height
    }
}

/// Pixel layout of mask / separator / dock / footer / chevron.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct InputDockLayout {
    pub mask: DockBounds,
    pub separator: DockBounds,
    pub dock: DockBounds,
    pub footer: DockBounds,
    pub chevron: DockBounds,
    pub cell_origin: PointF,
    pub map: DockCoordinateMap,
}

pub const SEPARATOR_H: f32 = 1.0;
pub const DOCK_H: f32 = 55.0;
pub const FOOTER_H: f32 = 31.0;
pub const CHROME_TOTAL: f32 = 87.0;
pub const PAD_X: f32 = 24.0;
pub const PAD_Y: f32 = 18.0;
pub const CHEVRON_X_INSET: f32 = 26.0;
pub const CHEVRON_GAP: f32 = 21.0;
pub const CARET_W: f32 = 2.0;
pub const CHEVRON_W: f32 = 16.0;

/// Rebuild the dock snapshot for `pane`. Fail-closed to Hidden.
pub fn derive_input_dock(pane: &PaneModel, palette_open: bool) -> InputDockSnapshot {
    let hidden = |source: DockSourceSpan, cursor: DockCursor, prompt_kind: Option<PromptKind>| {
        InputDockSnapshot {
            state: InputDockState::Hidden,
            pane_id: pane.id,
            source,
            cells: Arc::from([]),
            styles: Arc::from([]),
            combining: Arc::from([]),
            cursor,
            footer: footer_from_pwd(pane.core.pwd()),
            prompt_kind,
            generation: 0,
        }
    };

    let snapshot = pane.core.terminal_snapshot();
    let semantic = pane.core.semantic_state();
    let cursor = snapshot.cursor;
    let fallback_span = DockSourceSpan {
        row: cursor.row,
        start_col: 0,
        end_col: cursor.col.max(1),
    };
    let fallback_cursor = DockCursor {
        source_col: cursor.col,
        visible: false,
        wrap_pending: cursor.wrap_pending,
    };

    if palette_open {
        return hidden(fallback_span, fallback_cursor, semantic.prompt_kind);
    }
    if pane
        .latest_frame
        .as_ref()
        .is_some_and(|frame| frame.viewport.alternate_screen)
        || pane.core.has_mode(TerminalMode::AltScreen)
    {
        return hidden(fallback_span, fallback_cursor, semantic.prompt_kind);
    }
    if pane
        .latest_frame
        .as_ref()
        .is_some_and(|frame| frame.viewport.scroll_offset != 0)
        || pane.viewport.offset() != 0
    {
        return hidden(fallback_span, fallback_cursor, semantic.prompt_kind);
    }
    if !pane.ever_seen_osc133() {
        return hidden(fallback_span, fallback_cursor, semantic.prompt_kind);
    }
    if semantic.prompt_kind == Some(PromptKind::Right) {
        return hidden(fallback_span, fallback_cursor, semantic.prompt_kind);
    }
    if pane.core.has_mode(TerminalMode::MouseReportClick)
        || pane.core.has_mode(TerminalMode::MouseDrag)
        || pane.core.has_mode(TerminalMode::MouseMotion)
    {
        return hidden(fallback_span, fallback_cursor, semantic.prompt_kind);
    }
    if !semantic.cursor_is_at_prompt() {
        return hidden(fallback_span, fallback_cursor, semantic.prompt_kind);
    }
    if semantic
        .input_start_row
        .is_some_and(|start_row| start_row != cursor.row)
        || cursor.wrap_pending
    {
        return hidden(fallback_span, fallback_cursor, semantic.prompt_kind);
    }

    let source = project_source_span(semantic, cursor.col, cursor.row, cursor.wrap_pending);
    let projection = extract_span_cells(&snapshot, source);
    InputDockSnapshot {
        state: InputDockState::ShellInputActive,
        pane_id: pane.id,
        source,
        cells: projection.cells,
        styles: projection.styles,
        combining: projection.combining,
        cursor: DockCursor {
            source_col: cursor.col,
            visible: true,
            wrap_pending: cursor.wrap_pending,
        },
        footer: footer_from_pwd(pane.core.pwd()),
        prompt_kind: semantic.prompt_kind,
        generation: pane
            .latest_frame
            .as_ref()
            .map(|frame| frame.sequence)
            .unwrap_or(0),
    }
}

/// Fish/A-only: whole cursor row from col 0. zsh after B: `input_start_col`.
pub fn project_source_span(
    semantic: &SemanticPromptState,
    cursor_col: u16,
    cursor_row: u16,
    wrap_pending: bool,
) -> DockSourceSpan {
    let row = semantic.input_start_row.unwrap_or(cursor_row);
    let start_col = semantic.input_start_col.unwrap_or(0);
    let mut end_col = cursor_col.max(start_col.saturating_add(1));
    if wrap_pending {
        end_col = end_col.saturating_add(1);
    }
    DockSourceSpan {
        row,
        start_col,
        end_col,
    }
}

/// Copy `[start_col, end_col)` of `span.row` from the snapshot. Wide pairs
/// are never split: a start that lands on `WIDE_SPACER` steps back one col.
pub fn extract_span_cells(
    snapshot: &NormalizedSnapshot,
    span: DockSourceSpan,
) -> DockSpanProjection {
    let cols = usize::from(snapshot.size.cols);
    let rows = usize::from(snapshot.size.rows);
    let row = usize::from(span.row);
    if cols == 0 || row >= rows {
        return DockSpanProjection {
            cells: Arc::from([]),
            styles: Arc::from(snapshot.styles.as_slice()),
            combining: Arc::from([]),
        };
    }
    let row_start = row * cols;
    let mut start = usize::from(span.start_col).min(cols);
    let end = usize::from(span.end_col)
        .min(cols)
        .max(start.saturating_add(1).min(cols));
    if start < cols {
        let cell = snapshot.cells[row_start + start];
        if cell.flags & Cell::WIDE_SPACER != 0 && start > 0 {
            start -= 1;
        }
    }
    let slice = if start < end {
        snapshot.cells[row_start + start..row_start + end].to_vec()
    } else {
        Vec::new()
    };
    let combining: Vec<CombiningMarks> = snapshot
        .combining_marks
        .iter()
        .filter_map(|mark| {
            let index = mark.cell_index as usize;
            if index >= row_start + start && index < row_start + end {
                Some(CombiningMarks {
                    cell_index: (index - (row_start + start)) as u32,
                    codepoints: mark.codepoints.clone(),
                })
            } else {
                None
            }
        })
        .collect();
    DockSpanProjection {
        cells: Arc::from(slice),
        styles: Arc::from(snapshot.styles.as_slice()),
        combining: Arc::from(combining),
    }
}

/// One-row synthetic frame for `TerminalElement::with_shared`. Cursor is
/// hidden so the element does not paint a PTY block cursor; chrome paints
/// the 2px caret. Not published as `pane.latest_frame`.
pub fn synthetic_dock_frame(snap: &InputDockSnapshot) -> FrameDelta {
    let cols = snap.source.col_count().max(1);
    let mut cells = snap.cells.to_vec();
    cells.resize(usize::from(cols), Cell::default());
    let mut runs = Vec::new();
    batch_runs(&cells, &mut runs);
    let mut frame = FrameDelta::empty(GridSize::new(cols, 1));
    frame.sequence = snap.generation;
    frame.damage = DamageKind::Full;
    frame.rows = vec![RowDelta {
        row: 0,
        generation: snap.generation,
        cells,
        runs,
    }];
    frame.cursor = CursorState {
        row: 0,
        col: snap.cursor.source_col.saturating_sub(snap.source.start_col),
        visible: false,
        blinking: false,
        wrap_pending: snap.cursor.wrap_pending,
        ..CursorState::default()
    };
    frame.styles = snap.styles.to_vec();
    frame.viewport = TerminalViewport {
        scroll_offset: 0,
        history_rows: 0,
        alternate_screen: false,
    };
    frame
}

/// Window-bottom dock geometry. `None` when Hidden so callers omit children.
pub fn layout_input_dock(
    window_viewport: (f32, f32),
    pane_origin: PointF,
    pane_content: (f32, f32),
    metrics: CellMetrics,
    snap: &InputDockSnapshot,
    focused: bool,
) -> Option<InputDockLayout> {
    let _ = focused;
    if snap.state != InputDockState::ShellInputActive {
        return None;
    }
    let cell_h = metrics.height;
    let mask = DockBounds {
        x: pane_origin.x,
        y: pane_origin.y + f32::from(snap.source.row) * cell_h,
        width: pane_content.0,
        height: cell_h,
    };
    let footer = DockBounds {
        x: 0.0,
        y: window_viewport.1 - FOOTER_H,
        width: window_viewport.0,
        height: FOOTER_H,
    };
    let dock = DockBounds {
        x: 0.0,
        y: window_viewport.1 - FOOTER_H - DOCK_H,
        width: window_viewport.0,
        height: DOCK_H,
    };
    let separator = DockBounds {
        x: 0.0,
        y: dock.y - SEPARATOR_H,
        width: window_viewport.0,
        height: SEPARATOR_H,
    };
    let chevron = DockBounds {
        x: dock.x + CHEVRON_X_INSET,
        y: dock.y + (DOCK_H - cell_h).max(0.0) * 0.5,
        width: CHEVRON_W,
        height: cell_h,
    };
    let cell_origin = PointF {
        x: dock.x + CHEVRON_X_INSET + CHEVRON_GAP,
        y: dock.y + PAD_Y,
    };
    Some(InputDockLayout {
        mask,
        separator,
        dock,
        footer,
        chevron,
        cell_origin,
        map: DockCoordinateMap {
            pane_id: snap.pane_id,
            metrics,
            pane_origin,
            source_row: snap.source.row,
            source_start_col: snap.source.start_col,
            projected_cols: snap.source.col_count(),
            cell_origin,
        },
    })
}

pub fn hit_test_dock(layout: &InputDockLayout, window_pt: PointF) -> DockHit {
    if layout.footer.contains(window_pt.x, window_pt.y) {
        return DockHit::Footer;
    }
    if layout.separator.contains(window_pt.x, window_pt.y) {
        return DockHit::Separator;
    }
    if layout.chevron.contains(window_pt.x, window_pt.y) {
        return DockHit::Chevron;
    }
    if layout.dock.contains(window_pt.x, window_pt.y) {
        let local_x = window_pt.x - layout.cell_origin.x;
        if local_x < 0.0 {
            return DockHit::Chevron;
        }
        let width = layout.map.metrics.width.max(1.0);
        let col_f = local_x / width;
        let col = col_f
            .floor()
            .clamp(0.0, f32::from(layout.map.projected_cols.saturating_sub(1)))
            as u16;
        let frac_x = (col_f - f32::from(col)).clamp(0.0, 1.0);
        return DockHit::Cells { col, frac_x };
    }
    DockHit::Miss
}

/// Remap a dock cell hit into pane-content local coordinates.
pub fn remap_pointer(map: &DockCoordinateMap, hit: DockHit) -> Option<(f32, f32)> {
    match hit {
        DockHit::Cells { col, frac_x } => {
            let x = (f32::from(map.source_start_col + col) + frac_x) * map.metrics.width;
            let y = f32::from(map.source_row) * map.metrics.height + map.metrics.height * 0.5;
            Some((x, y))
        }
        DockHit::Miss | DockHit::Separator | DockHit::Footer | DockHit::Chevron => None,
    }
}

fn footer_from_pwd(pwd: Option<&str>) -> DockFooter {
    let label = pwd
        .and_then(osc7_basename)
        .unwrap_or_else(|| "~".to_string());
    DockFooter {
        label: Arc::from(label),
    }
}

fn osc7_basename(pwd: &str) -> Option<String> {
    let path = pwd
        .strip_prefix("file://")
        .map(|rest| {
            if let Some(slash) = rest.find('/') {
                &rest[slash..]
            } else {
                rest
            }
        })
        .unwrap_or(pwd);
    if path.is_empty() || path == "/" {
        return Some("/".to_string());
    }
    path.rsplit('/')
        .find(|part| !part.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use mr_crabs_protocols::shell::{SemanticContent, SemanticPromptState};
    use mr_crabs_terminal::{CursorSnapshot, GridSize};

    fn empty_snapshot(cols: u16, rows: u16, col: u16, row: u16) -> NormalizedSnapshot {
        let n = usize::from(cols) * usize::from(rows);
        NormalizedSnapshot {
            size: GridSize { cols, rows },
            cursor: CursorSnapshot {
                row,
                col,
                wrap_pending: false,
            },
            cells: vec![Cell::default(); n],
            styles: vec![Style::default()],
            combining_marks: Vec::new(),
            hyperlinks: Vec::new(),
            modes: Vec::new(),
        }
    }

    #[test]
    fn hidden_without_osc133() {
        let pane = PaneModel::detached(PaneId::new(1), GridSize::new(80, 24)).expect("pane");
        let snap = derive_input_dock(&pane, false);
        assert_eq!(snap.state, InputDockState::Hidden);
        assert!(!pane.ever_seen_osc133());
    }

    #[test]
    fn active_after_133_a_b() {
        let mut pane = PaneModel::detached(PaneId::new(1), GridSize::new(80, 24)).expect("pane");
        pane.feed_test_output(b"\x1b]133;A\x07$ \x1b]133;B\x07ls")
            .expect("feed");
        assert!(pane.ever_seen_osc133());
        let snap = derive_input_dock(&pane, false);
        assert_eq!(snap.state, InputDockState::ShellInputActive);
        assert_eq!(
            snap.source.start_col,
            pane.core.semantic_state().input_start_col.unwrap_or(0)
        );
        let snapshot = pane.core.terminal_snapshot();
        let projection = extract_span_cells(&snapshot, snap.source);
        assert_eq!(&*snap.cells, &*projection.cells);
    }

    #[test]
    fn fish_a_without_b() {
        let mut pane = PaneModel::detached(PaneId::new(1), GridSize::new(80, 24)).expect("pane");
        pane.feed_test_output(b"\x1b]133;A\x07").expect("feed");
        let snap = derive_input_dock(&pane, false);
        assert_eq!(snap.state, InputDockState::ShellInputActive);
        assert_eq!(snap.source.start_col, 0);
    }

    #[test]
    fn hidden_on_133_c_and_d() {
        let mut pane = PaneModel::detached(PaneId::new(1), GridSize::new(80, 24)).expect("pane");
        pane.feed_test_output(b"\x1b]133;A\x07\x1b]133;B\x07ls\x1b]133;C\x07")
            .expect("feed");
        assert_eq!(
            derive_input_dock(&pane, false).state,
            InputDockState::Hidden
        );

        let mut pane = PaneModel::detached(PaneId::new(2), GridSize::new(80, 24)).expect("pane");
        pane.feed_test_output(b"\x1b]133;A\x07\x1b]133;B\x07ls\x1b]133;D;0\x07")
            .expect("feed");
        assert_eq!(
            derive_input_dock(&pane, false).state,
            InputDockState::Hidden
        );
        assert_eq!(
            pane.core.semantic_state().row,
            mr_crabs_protocols::shell::RowSemantic::None
        );
    }

    #[test]
    fn hidden_on_1049_even_if_prompt() {
        let mut pane = PaneModel::detached(PaneId::new(1), GridSize::new(80, 24)).expect("pane");
        pane.feed_test_output(b"\x1b]133;A\x07\x1b]133;B\x07\x1b[?1049h")
            .expect("feed");
        let snap = derive_input_dock(&pane, false);
        assert_eq!(snap.state, InputDockState::Hidden);
        assert_eq!(pane.last_size.rows, 24);
        assert_eq!(pane.session.last_size().rows, 24);
    }

    #[test]
    fn right_prompt_does_not_activate() {
        let mut pane = PaneModel::detached(PaneId::new(1), GridSize::new(80, 24)).expect("pane");
        pane.feed_test_output(b"\x1b]133;P;k=r\x07").expect("feed");
        assert_eq!(
            derive_input_dock(&pane, false).state,
            InputDockState::Hidden
        );
    }

    #[test]
    fn hidden_when_palette_open() {
        let mut pane = PaneModel::detached(PaneId::new(1), GridSize::new(80, 24)).expect("pane");
        pane.feed_test_output(b"\x1b]133;A\x07").expect("feed");
        assert_eq!(derive_input_dock(&pane, true).state, InputDockState::Hidden);
    }

    #[test]
    fn remap_pointer_cells_to_source_row() {
        let metrics = CellMetrics::new(8.0, 16.0).expect("metrics");
        let map = DockCoordinateMap {
            pane_id: PaneId::new(1),
            metrics,
            pane_origin: PointF { x: 0.0, y: 0.0 },
            source_row: 5,
            source_start_col: 2,
            projected_cols: 10,
            cell_origin: PointF { x: 47.0, y: 400.0 },
        };
        let mapped = remap_pointer(
            &map,
            DockHit::Cells {
                col: 3,
                frac_x: 0.25,
            },
        )
        .expect("cells");
        assert!((mapped.0 - (2.0 + 3.0 + 0.25) * 8.0).abs() < 0.01);
        assert!((mapped.1 - (5.0 * 16.0 + 8.0)).abs() < 0.01);
    }

    #[test]
    fn footer_hit_does_not_select() {
        let metrics = CellMetrics::new(8.0, 16.0).expect("metrics");
        let snap = InputDockSnapshot {
            state: InputDockState::ShellInputActive,
            pane_id: PaneId::new(1),
            source: DockSourceSpan {
                row: 0,
                start_col: 0,
                end_col: 4,
            },
            cells: Arc::from([Cell::default(); 4]),
            styles: Arc::from([Style::default()]),
            combining: Arc::from([]),
            cursor: DockCursor {
                source_col: 1,
                visible: true,
                wrap_pending: false,
            },
            footer: DockFooter {
                label: Arc::from("~"),
            },
            prompt_kind: None,
            generation: 0,
        };
        let layout = layout_input_dock(
            (800.0, 600.0),
            PointF { x: 0.0, y: 0.0 },
            (800.0, 600.0),
            metrics,
            &snap,
            true,
        )
        .expect("layout");
        let hit = hit_test_dock(
            &layout,
            PointF {
                x: 24.0,
                y: 600.0 - 10.0,
            },
        );
        assert_eq!(hit, DockHit::Footer);
        assert_eq!(remap_pointer(&layout.map, hit), None);
    }

    #[test]
    fn synthetic_frame_is_one_row_with_hidden_cursor() {
        let snap = InputDockSnapshot {
            state: InputDockState::ShellInputActive,
            pane_id: PaneId::new(1),
            source: DockSourceSpan {
                row: 3,
                start_col: 0,
                end_col: 4,
            },
            cells: Arc::from(vec![Cell::default(); 4]),
            styles: Arc::from([Style::default()]),
            combining: Arc::from([]),
            cursor: DockCursor {
                source_col: 2,
                visible: true,
                wrap_pending: false,
            },
            footer: DockFooter {
                label: Arc::from("~"),
            },
            prompt_kind: None,
            generation: 9,
        };
        let frame = synthetic_dock_frame(&snap);
        assert_eq!(frame.size.rows, 1);
        assert_eq!(frame.size.cols, 4);
        assert_eq!(frame.rows.len(), 1);
        assert!(!frame.cursor.visible);
        assert!(!frame.viewport.alternate_screen);
        assert_eq!(frame.damage, DamageKind::Full);
    }

    #[test]
    fn project_source_span_uses_b_col() {
        let mut semantic = SemanticPromptState::new();
        semantic.content = SemanticContent::Input;
        semantic.input_start_col = Some(4);
        semantic.input_start_row = Some(2);
        let span = project_source_span(&semantic, 7, 2, false);
        assert_eq!(
            span,
            DockSourceSpan {
                row: 2,
                start_col: 4,
                end_col: 7,
            }
        );
    }

    #[test]
    fn text_right_of_moved_cursor_stays_in_snapshot_and_synthetic_frame() {
        let mut pane = PaneModel::detached(PaneId::new(1), GridSize::new(80, 24)).expect("pane");
        pane.feed_test_output(b"\x1b]133;A\x07$ \x1b]133;B\x07hello")
            .expect("feed");
        pane.feed_test_output(b"\x1b[3D")
            .expect("move cursor left into hello");

        let snap = derive_input_dock(&pane, false);
        assert_eq!(snap.state, InputDockState::ShellInputActive);
        let text: String = snap
            .cells
            .iter()
            .filter_map(|cell| char::from_u32(cell.content).filter(|ch| *ch != '\0'))
            .collect();
        assert!(
            text.contains("hello"),
            "text to the right of the moved cursor must remain in InputDockSnapshot, got {text:?} span={:?} cursor={:?}",
            snap.source,
            snap.cursor
        );
        assert!(
            snap.source.end_col > snap.cursor.source_col,
            "span must extend past the moved cursor so trailing text remains, span={:?} cursor={:?}",
            snap.source,
            snap.cursor
        );

        let frame = synthetic_dock_frame(&snap);
        let frame_text: String = frame.rows[0]
            .cells
            .iter()
            .filter_map(|cell| char::from_u32(cell.content).filter(|ch| *ch != '\0'))
            .collect();
        assert!(
            frame_text.contains("hello"),
            "synthetic frame must keep text to the right of the moved cursor, got {frame_text:?}"
        );
    }

    #[test]
    fn osc133_prompt_redraw_does_not_hide_current_prompt_on_stale_input_start() {
        let mut pane = PaneModel::detached(PaneId::new(1), GridSize::new(80, 24)).expect("pane");
        pane.feed_test_output(b"\x1b]133;A\x07$ \x1b]133;B\x07old")
            .expect("first prompt");
        pane.feed_test_output(b"\r\n\x1b]133;P;k=i\x07(reverse-i-search)`': \x1b]133;B\x07")
            .expect("ctrl-r shaped redraw");

        let semantic = pane.core.semantic_state();
        let snapshot = pane.core.terminal_snapshot();
        let snap = derive_input_dock(&pane, false);
        assert_eq!(
            snap.state,
            InputDockState::ShellInputActive,
            "stale input_start coordinates must not hide the current prompt dock; input_start_row={:?} cursor_row={} content={:?} row={:?}",
            semantic.input_start_row,
            snapshot.cursor.row,
            semantic.content,
            semantic.row
        );
    }


    #[test]
    fn extract_span_never_splits_wide_pair() {
        let mut snapshot = empty_snapshot(8, 1, 3, 0);
        snapshot.cells[2].flags |= Cell::WIDE;
        snapshot.cells[3].flags |= Cell::WIDE_SPACER;
        let projection = extract_span_cells(
            &snapshot,
            DockSourceSpan {
                row: 0,
                start_col: 3,
                end_col: 5,
            },
        );
        assert_eq!(projection.cells[0].flags & Cell::WIDE, Cell::WIDE);
        assert_eq!(
            projection.cells[1].flags & Cell::WIDE_SPACER,
            Cell::WIDE_SPACER
        );
    }

    #[test]
    fn no_command_buffer_field() {
        let snap = InputDockSnapshot {
            state: InputDockState::Hidden,
            pane_id: PaneId::new(1),
            source: DockSourceSpan {
                row: 0,
                start_col: 0,
                end_col: 1,
            },
            cells: Arc::from([]),
            styles: Arc::from([]),
            combining: Arc::from([]),
            cursor: DockCursor {
                source_col: 0,
                visible: false,
                wrap_pending: false,
            },
            footer: DockFooter {
                label: Arc::from("~"),
            },
            prompt_kind: None,
            generation: 0,
        };
        let _ = snap.cells;
        let _ = snap.state;
    }

    #[test]
    fn keys_still_hit_pty() {
        use std::sync::mpsc::sync_channel;
        let size = GridSize::new(80, 24);
        let mut pane = PaneModel::detached(PaneId::new(1), size).expect("pane");
        let (reader_tx, reader_rx) = sync_channel::<Vec<u8>>(8);
        let (writer_tx, writer_rx) = sync_channel::<Vec<u8>>(8);
        pane.session = crate::model::pane::PaneSession::from_receivers_with_writer(
            size,
            Some(reader_rx),
            None,
            Some(writer_tx),
        );
        pane.feed_test_output(b"\x1b]133;A\x07\x1b]133;B\x07")
            .expect("feed");
        assert_eq!(
            derive_input_dock(&pane, false).state,
            InputDockState::ShellInputActive
        );
        pane.session.write(b"a").expect("write");
        let written = writer_rx.try_recv().expect("pty writer received key");
        assert_eq!(written, b"a");
        let before = derive_input_dock(&pane, false).cells.clone();
        reader_tx.send(b"a".to_vec()).expect("echo");
        pane.pump(4);
        let after = derive_input_dock(&pane, false);
        assert_eq!(after.state, InputDockState::ShellInputActive);
        assert_ne!(
            &*after.cells, &*before,
            "cells change only after reader pump, not from a host buffer"
        );
    }
}

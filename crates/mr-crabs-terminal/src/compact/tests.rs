//! Focused CompactEngine contracts: wrap, history identity, region scroll,
//! wide overwrite, combining, alt screen, resize/reflow, modes, damage,
//! snapshot/restore, and chunk-invariant vte feed.

use std::sync::Arc;

use vte::ansi::{
    Handler, Mode, NamedMode, NamedPrivateMode, PrivateMode, Processor, StdSyncHandler,
};

use super::flags;
use super::{CompactEngine, CompactPage, CompactRow, RowExtras};
use crate::{
    Cell, CombiningMarks, CursorSnapshot, DamageKind, GridSize, HistoryRead, SnapshotHyperlink,
    Style, TerminalError, TerminalMode,
};

fn size(cols: u16, rows: u16) -> GridSize {
    GridSize::new(cols, rows)
}

fn engine(cols: u16, rows: u16) -> CompactEngine {
    CompactEngine::new(size(cols, rows)).expect("nonzero grid")
}

fn feed(engine: &mut CompactEngine, bytes: &[u8]) {
    let mut processor: Processor<StdSyncHandler> = Processor::new();
    processor.advance(engine, bytes);
}

fn feed_chunked(engine: &mut CompactEngine, bytes: &[u8], chunk: usize) {
    let mut processor: Processor<StdSyncHandler> = Processor::new();
    let chunk = chunk.max(1);
    for piece in bytes.chunks(chunk) {
        processor.advance(engine, piece);
    }
}

fn cell_ch(cell: &Cell) -> char {
    char::from_u32(cell.content).unwrap_or('\u{FFFD}')
}

fn row_text(row: &[Cell]) -> String {
    row.iter().map(cell_ch).collect()
}

fn visible_text(engine: &CompactEngine) -> Vec<String> {
    engine
        .visible_rows()
        .into_iter()
        .map(|row| row_text(&row))
        .collect()
}

fn history_text(engine: &mut CompactEngine, index: usize) -> Option<String> {
    let mut out = Vec::new();
    if engine.read_history_line(index, &mut out) {
        Some(row_text(&out))
    } else {
        None
    }
}

#[test]
fn new_rejects_zero_dimensions() {
    assert_eq!(
        CompactEngine::new(size(0, 24)).err(),
        Some(TerminalError::ZeroColumns)
    );
    assert_eq!(
        CompactEngine::new(size(80, 0)).err(),
        Some(TerminalError::ZeroRows)
    );
    assert_eq!(
        CompactEngine::new_with_history(size(0, 1), 8).err(),
        Some(TerminalError::ZeroColumns)
    );
}

#[test]
fn pending_wrap_holds_then_advances_on_next_glyph() {
    let mut term = engine(4, 3);
    assert!(term.has_mode(TerminalMode::LineWrap));
    for ch in ['A', 'B', 'C', 'D'] {
        term.input(ch);
    }
    let cursor = term.cursor();
    assert_eq!(cursor.row, 0);
    assert_eq!(cursor.col, 3);
    assert!(cursor.wrap_pending);
    assert_eq!(&visible_text(&term)[0], "ABCD");

    term.input('E');
    let cursor = term.cursor();
    assert!(!cursor.wrap_pending);
    assert_eq!(cursor.row, 1);
    assert_eq!(cursor.col, 1);
    let rows = term.visible_rows();
    assert_eq!(cell_ch(&rows[0][3]), 'D');
    assert_ne!(rows[0][3].flags & flags::WRAPLINE, 0);
    assert_eq!(cell_ch(&rows[1][0]), 'E');
}

#[test]
fn full_screen_scroll_moves_row_descriptor_identity() {
    let mut term = engine(8, 3);
    feed(&mut term, b"AAA\r\nBBB\r\nCCC");
    assert_eq!(term.history_len(), 0);
    assert_eq!(term.history_line_cols(0), None);

    feed(&mut term, b"\n");
    assert_eq!(term.history_len(), 1);
    assert_eq!(term.history_line_cols(0), Some(8));
    assert_eq!(history_text(&mut term, 0).as_deref(), Some("AAA     "));

    let pages = term.history_pages();
    assert_eq!(pages.len(), 1);
    assert_eq!(pages[0].cols, 8);
    assert_eq!(pages[0].len(), 1);
    assert!(!pages[0].is_empty());
    let page_cells = Arc::clone(&pages[0].rows[0].cells);

    let scrolled = term.take_scrolled_rows();
    assert_eq!(scrolled.len(), 1);
    assert!(Arc::ptr_eq(&page_cells, &scrolled[0].cells));
    assert_eq!(scrolled[0].cols, 8);
    assert_eq!(scrolled[0].occupancy, 3);
    assert!(!scrolled[0].wrapped);
    assert_eq!(term.history_len(), 0);
    assert!(term.take_scrolled_rows().is_empty());
}

#[test]
fn region_scroll_leaves_outside_rows_and_skips_history() {
    let mut term = engine(4, 4);
    feed(&mut term, b"AAAA\r\nBBBB\r\nCCCC\r\nDDDD");
    term.set_scrolling_region(2, Some(3));
    assert_eq!(
        term.cursor(),
        CursorSnapshot {
            row: 0,
            col: 0,
            wrap_pending: false,
        }
    );

    term.scroll_up(1);
    let rows = visible_text(&term);
    assert_eq!(rows[0], "AAAA");
    assert_eq!(rows[1], "CCCC");
    assert_eq!(rows[2], "    ");
    assert_eq!(rows[3], "DDDD");
    assert_eq!(term.history_len(), 0);
}

#[test]
fn wide_overwrite_clears_spacer() {
    let mut term = engine(6, 2);
    term.input('中');
    let rows = term.visible_rows();
    assert_eq!(cell_ch(&rows[0][0]), '中');
    assert_ne!(rows[0][0].flags & flags::WIDE_CHAR, 0);
    assert_ne!(rows[0][1].flags & flags::WIDE_CHAR_SPACER, 0);
    assert_eq!(term.cursor().col, 2);

    term.goto(0, 0);
    term.input('A');
    let rows = term.visible_rows();
    assert_eq!(cell_ch(&rows[0][0]), 'A');
    assert_eq!(rows[0][0].flags & flags::WIDE_BITS, 0);
    assert_eq!(rows[0][1].flags & flags::WIDE_CHAR_SPACER, 0);
}

#[test]
fn combining_mark_attaches_to_previous_cell() {
    let mut term = engine(8, 2);
    term.input('e');
    term.input('\u{0301}');
    let rows = term.visible_rows();
    assert_eq!(cell_ch(&rows[0][0]), 'e');
    assert_ne!(rows[0][0].flags & flags::COMBINING, 0);
    assert_eq!(term.cursor().col, 1);

    let snap = term.snapshot();
    assert_eq!(
        snap.combining_marks,
        vec![CombiningMarks {
            cell_index: 0,
            codepoints: vec![u32::from('\u{0301}')],
        }]
    );
}

#[test]
fn alt_screen_preserves_primary_and_does_not_ingest_history() {
    let mut term = engine(8, 3);
    feed(&mut term, b"PRIMARY\r\nLINE");
    assert!(!term.has_mode(TerminalMode::AltScreen));
    let primary = visible_text(&term);
    let history_before = term.history_len();

    term.set_private_mode(PrivateMode::Named(
        NamedPrivateMode::SwapScreenAndSetRestoreCursor,
    ));
    assert!(term.has_mode(TerminalMode::AltScreen));
    assert!(
        visible_text(&term)
            .iter()
            .all(|row| row.chars().all(|c| c == ' '))
    );

    feed(&mut term, b"ALT\n\n\n\n");
    assert_eq!(term.history_len(), history_before);

    term.unset_private_mode(PrivateMode::Named(
        NamedPrivateMode::SwapScreenAndSetRestoreCursor,
    ));
    assert!(!term.has_mode(TerminalMode::AltScreen));
    assert_eq!(visible_text(&term), primary);
}

#[test]
fn resize_reflow_joins_wrapped_rows_and_shorter_height_pages_history() {
    let mut term = engine(4, 3);
    for ch in "ABCDEFGH".chars() {
        term.input(ch);
    }
    assert_eq!(visible_text(&term)[0], "ABCD");
    assert_eq!(visible_text(&term)[1], "EFGH");
    assert_ne!(term.visible_rows()[0][3].flags & flags::WRAPLINE, 0);

    term.resize(size(8, 3)).expect("widen");
    let widened = visible_text(&term);
    assert!(
        widened.iter().any(|row| row.starts_with("ABCDEFGH")),
        "wrapped stream must reflow into the new width: {widened:?}"
    );

    let mut tall = engine(4, 4);
    feed(&mut tall, b"W0\r\nW1\r\nW2\r\nW3");
    tall.resize(size(4, 2)).expect("shorten");
    assert_eq!(tall.size(), size(4, 2));
    assert_eq!(tall.history_len(), 2);
    assert_eq!(history_text(&mut tall, 0).as_deref(), Some("W0  "));
    assert_eq!(history_text(&mut tall, 1).as_deref(), Some("W1  "));
    assert_eq!(visible_text(&tall)[0].trim_end(), "W2");
    assert_eq!(visible_text(&tall)[1].trim_end(), "W3");

    assert_eq!(
        tall.resize(size(0, 2)).err(),
        Some(TerminalError::ZeroColumns)
    );
    assert_eq!(tall.resize(size(4, 0)).err(), Some(TerminalError::ZeroRows));
    tall.resize(size(4, 2)).expect("identical size is a no-op");
    assert_eq!(tall.size(), size(4, 2));
}

#[test]
fn taller_primary_grid_restores_history_and_shifts_live_and_saved_cursor() {
    let mut term = engine(4, 3);
    feed(&mut term, b"W0\r\nW1\r\nW2\r\nW3\r\nW4");
    assert_eq!(term.history_len(), 2);
    assert_eq!(term.cursor().row, 2);
    feed(&mut term, b"\x1b7");

    term.resize(size(4, 6)).expect("grow rows");

    assert_eq!(term.history_len(), 0);
    assert_eq!(
        visible_text(&term)
            .iter()
            .map(|row| row.trim_end())
            .collect::<Vec<_>>(),
        vec!["W0", "W1", "W2", "W3", "W4", ""]
    );
    assert_eq!(term.cursor().row, 4, "live cursor follows restored history");
    feed(&mut term, b"\x1b8");
    assert_eq!(
        term.cursor().row,
        4,
        "saved cursor follows restored history"
    );
}

#[test]
fn modes_set_report_and_restore() {
    let mut term = engine(8, 4);
    assert!(term.has_mode(TerminalMode::ShowCursor));
    assert!(term.has_mode(TerminalMode::LineWrap));
    assert!(term.has_mode(TerminalMode::AlternateScroll));
    assert!(term.has_mode(TerminalMode::UrgencyHints));
    assert!(!term.has_mode(TerminalMode::Insert));
    assert!(!term.has_mode(TerminalMode::AltScreen));

    term.set_mode(Mode::Named(NamedMode::Insert));
    term.set_private_mode(PrivateMode::Named(NamedPrivateMode::BracketedPaste));
    term.set_private_mode(PrivateMode::Named(NamedPrivateMode::CursorKeys));
    assert!(term.has_mode(TerminalMode::Insert));
    assert!(term.has_mode(TerminalMode::BracketedPaste));
    assert!(term.has_mode(TerminalMode::AppCursor));
    assert!(term.modes().contains(&TerminalMode::Insert));

    term.unset_mode(Mode::Named(NamedMode::Insert));
    term.unset_private_mode(PrivateMode::Named(NamedPrivateMode::LineWrap));
    assert!(!term.has_mode(TerminalMode::Insert));
    assert!(!term.has_mode(TerminalMode::LineWrap));

    let saved = vec![
        TerminalMode::ShowCursor,
        TerminalMode::BracketedPaste,
        TerminalMode::AppCursor,
        TerminalMode::AlternateScroll,
        TerminalMode::UrgencyHints,
    ];
    term.restore_modes(&saved);
    assert!(term.has_mode(TerminalMode::BracketedPaste));
    assert!(term.has_mode(TerminalMode::AppCursor));
    assert!(!term.has_mode(TerminalMode::LineWrap));
    assert!(!term.has_mode(TerminalMode::AltScreen));
}

#[test]
fn damage_tracks_partial_then_full_then_clears() {
    let mut term = engine(8, 3);
    assert_eq!(term.take_damage(), DamageKind::Full);
    assert_eq!(term.take_damage(), DamageKind::Clean);

    term.input('Z');
    assert_eq!(term.take_damage(), DamageKind::Partial);
    assert_eq!(term.take_damage(), DamageKind::Clean);

    feed(&mut term, b"\r\n\n\n");
    assert_eq!(term.take_damage(), DamageKind::Full);
    assert_eq!(term.take_damage(), DamageKind::Clean);
}

#[test]
fn snapshot_restore_round_trips_grid_cursor_modes_and_side_tables() {
    let mut term = engine(4, 2);
    term.input('e');
    term.input('\u{0301}');
    term.input('X');
    term.set_private_mode(PrivateMode::Named(NamedPrivateMode::BracketedPaste));
    let snap = term.snapshot();
    assert_eq!(snap.size, size(4, 2));
    assert_eq!(snap.cursor.col, 2);
    assert!(!snap.combining_marks.is_empty());
    assert!(snap.modes.contains(&TerminalMode::BracketedPaste));

    let mut restored = CompactEngine::new(snap.size).expect("restore target");
    restored
        .restore_visible_grid(
            &snap.cells,
            &snap.styles,
            &snap.combining_marks,
            &snap.hyperlinks,
        )
        .expect("grid");
    restored.restore_cursor(snap.cursor).expect("cursor");
    restored.restore_modes(&snap.modes);

    let again = restored.snapshot();
    assert_eq!(again.cells, snap.cells);
    assert_eq!(again.cursor, snap.cursor);
    assert_eq!(again.combining_marks, snap.combining_marks);
    assert!(restored.has_mode(TerminalMode::BracketedPaste));
    assert_eq!(
        restored.restore_cursor(CursorSnapshot {
            row: 99,
            col: 0,
            wrap_pending: false,
        }),
        Err(TerminalError::RestoreSizeMismatch)
    );

    let mut linked = engine(3, 1);
    let cells = vec![
        Cell {
            content: u32::from('A'),
            style: 0,
            flags: 0,
        },
        Cell::default(),
        Cell::default(),
    ];
    linked
        .restore_visible_grid(
            &cells,
            &[Style::default()],
            &[],
            &[SnapshotHyperlink {
                cell_index: 0,
                id: Some("id1".into()),
                uri: "https://example.test".into(),
            }],
        )
        .expect("link grid");
    let link = linked.hyperlink_at(0, 0).expect("restored hyperlink");
    assert_eq!(link.id.as_deref(), Some("id1"));
    assert_eq!(link.uri, "https://example.test");
    assert!(linked.hyperlink_at(0, 1).is_none());
}

#[test]
fn arbitrary_feed_chunking_is_snapshot_invariant() {
    let bytes = b"Hello\r\n\x1b[31mRed\x1b[0m\r\n\x1b[2;3HXY\x1b[6n\x1b[?1049hALT\x1b[?1049l";
    let grid = size(12, 4);

    let mut whole = CompactEngine::new(grid).expect("whole");
    feed(&mut whole, bytes);
    let expected = whole.snapshot();
    let replies = whole.take_replies();
    assert!(!replies.is_empty());

    for chunk in [1usize, 2, 3, 5, 7, bytes.len()] {
        let mut split = CompactEngine::new(grid).expect("split");
        feed_chunked(&mut split, bytes, chunk);
        assert_eq!(
            split.snapshot(),
            expected,
            "chunk size {chunk} must match a single advance"
        );
        assert_eq!(split.take_replies(), replies);
    }
}

#[test]
fn history_read_trait_and_manual_push_clear() {
    let mut term = engine(4, 2);
    let cells = [
        Cell {
            content: u32::from('H'),
            style: 0,
            flags: 0,
        },
        Cell {
            content: u32::from('i'),
            style: 0,
            flags: 0,
        },
    ];
    term.push_history_line(4, &cells);
    assert_eq!(HistoryRead::history_len(&term), 1);
    assert_eq!(HistoryRead::history_line_cols(&term, 0), Some(4));
    let mut out = Vec::new();
    assert!(HistoryRead::read_history_line(&mut term, 0, &mut out));
    assert_eq!(cell_ch(&out[0]), 'H');
    assert_eq!(cell_ch(&out[1]), 'i');
    assert_eq!(out.len(), 4);
    term.clear_history();
    assert_eq!(term.history_len(), 0);
}

#[test]
fn compact_row_page_and_title_surface() {
    let row = CompactRow::new(
        vec![
            Cell {
                content: u32::from('Z'),
                style: 0,
                flags: 0,
            },
            Cell::default(),
        ],
        true,
    )
    .with_generation(9);
    assert_eq!(row.cols, 2);
    assert_eq!(row.occupancy, 1);
    assert!(row.wrapped);
    assert_eq!(row.generation, 9);
    assert!(!row.is_visually_empty());

    let blank = CompactRow::blank(3);
    assert_eq!(blank.cols, 3);
    assert_eq!(blank.occupancy, 0);
    assert!(blank.is_visually_empty());

    let extras = RowExtras::default();
    assert!(extras.is_empty());

    let page = CompactPage::new(vec![row], 2, 4);
    assert_eq!(page.len(), 1);
    assert_eq!(page.generation, 4);
    assert!(!page.is_empty());

    let mut term = engine(8, 2);
    assert!(term.title().is_none());
    term.set_window_title(Some("mr-crabs".into()));
    assert_eq!(term.title(), Some("mr-crabs"));
    term.set_default_cursor_blink(true);
    let _ = term.cursor_style();
}

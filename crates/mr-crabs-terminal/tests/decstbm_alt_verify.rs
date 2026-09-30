//! DECSTBM + 1049 read-only verifier (new file only, no source edits).
//! Proves via public `mr_crabs_terminal::Terminal` API + raw escape bytes:
//! - DECSTBM `\x1b[2;5r` sets region, scroll confines, `\x1b[r` resets full
//! - origin mode (DECOM `\x1b[?6h/l`) CUP respects region
//! - 1049 enter clears alt / preserves primary, exit restores (AltWriter guard)

use mr_crabs_terminal::{GridSize, Terminal, TerminalMode};

fn term(cols: u16, rows: u16) -> Terminal {
    Terminal::new(GridSize::new(cols, rows)).expect("nonzero grid")
}

fn cell_ch(content: u32) -> char {
    char::from_u32(content).unwrap_or('\u{FFFD}')
}

fn row_text(row: &[mr_crabs_terminal::Cell]) -> String {
    row.iter().map(|c| cell_ch(c.content)).collect()
}

fn visible_text(term: &mut Terminal) -> Vec<String> {
    term.visible_rows().iter().map(|r| row_text(r)).collect()
}

fn snap_row_text(term: &Terminal, row: usize) -> String {
    let snap = term.snapshot();
    let cols = usize::from(snap.size.cols);
    let start = row * cols;
    snap.cells[start..start + cols]
        .iter()
        .map(|c| cell_ch(c.content))
        .collect()
}

// Byte seq under test: `\x1b[2;5r` (DECSTBM top=2 bottom=5, 1-indexed).
// Then scroll confined: rows outside 1..=4 (0-indexed 0 and 5) must not move.
#[test]
fn decstbm_sets_region_and_confines_scroll() {
    let mut t = term(10, 6);
    // Fill 6 distinct rows. `\r\n` = CR LF.
    t.feed(b"R0\r\nR1\r\nR2\r\nR3\r\nR4\r\nR5")
        .expect("terminal feed");
    assert!(visible_text(&mut t)[0].starts_with("R0"));
    assert!(visible_text(&mut t)[5].starts_with("R5"));

    // DECSTBM set region rows 2..5 (1-indexed) => 0-indexed 1..5 (end-exclusive).
    t.feed(b"\x1b[2;5r").expect("terminal feed");
    // DECSTBM homes cursor to (0,0) per CompactEngine::set_scrolling_region -> goto(0,0).
    assert_eq!((t.snapshot().cursor.row, t.snapshot().cursor.col), (0, 0));

    // Park cursor on region bottom (row 5 1-indexed = 0-indexed 4) then LF.
    // `\x1b[5;1H` = CUP row=5 col=1.
    t.feed(b"\x1b[5;1H").expect("terminal feed");
    assert_eq!((t.snapshot().cursor.row, t.snapshot().cursor.col), (4, 0));
    // `\n` = LF triggers linefeed -> scroll_up_relative(region.start=1, 1).
    t.feed(b"\n").expect("terminal feed");

    let rows = visible_text(&mut t);
    // Outside rows pinned.
    assert!(rows[0].starts_with("R0"), "row0 must stay R0, got {:?}", rows[0]);
    assert!(rows[5].starts_with("R5"), "row5 must stay R5, got {:?}", rows[5]);
    // Inside region scrolled up by one: R1 evicted, R2..R4 shift, blank at 4.
    assert!(rows[1].starts_with("R2"), "row1 must be R2, got {:?}", rows[1]);
    assert!(rows[2].starts_with("R3"), "row2 must be R3, got {:?}", rows[2]);
    assert!(rows[3].starts_with("R4"), "row3 must be R4, got {:?}", rows[3]);
    assert!(rows[4].trim().is_empty(), "row4 must be blank, got {:?}", rows[4]);
    // Region scroll must not ingest history (origin != 0).
    assert_eq!(t.history_len(), 0, "region scroll must skip history");
}

// Byte seq under test: `\x1b[r` (DECSTBM reset => full 1..rows).
#[test]
fn decstbm_reset_restores_full_scroll() {
    let mut t = term(10, 6);
    t.feed(b"R0\r\nR1\r\nR2\r\nR3\r\nR4\r\nR5")
        .expect("terminal feed");
    // Narrow first so reset has observable effect.
    t.feed(b"\x1b[2;5r").expect("terminal feed");
    // Reset: `\x1b[r`.
    t.feed(b"\x1b[r").expect("terminal feed");

    // After reset, bottom LF must scroll the FULL grid (origin 0).
    // CUP to last row (6;1) then LF.
    t.feed(b"\x1b[6;1H").expect("terminal feed");
    assert_eq!((t.snapshot().cursor.row, t.snapshot().cursor.col), (5, 0));
    t.feed(b"\n").expect("terminal feed");
    let rows = visible_text(&mut t);
    // Full scroll: R0 evicted to history, R1..R5 shift, blank at bottom.
    assert!(rows[0].starts_with("R1"), "row0 must be R1 after full scroll, got {:?}", rows[0]);
    assert!(rows[4].starts_with("R5"), "row4 must be R5 after full scroll, got {:?}", rows[4]);
    assert!(rows[5].trim().is_empty(), "row5 must be blank, got {:?}", rows[5]);
    // Snapshot parity helper covers same grid via snapshot cells.
    assert!(snap_row_text(&t, 0).starts_with("R1"));
}

// Origin mode (DECOM): `\x1b[?6h` on, `\x1b[?6l` off.
// With region 2;5, CUP `\x1b[1;1H` must land on region top (row 1 0-indexed),
// not physical row 0.
#[test]
fn origin_mode_goto_respects_region() {
    let mut t = term(10, 6);
    // Set region 2;5.
    t.feed(b"\x1b[2;5r").expect("terminal feed");

    // Origin off: CUP 1;1 => physical (0,0).
    t.feed(b"\x1b[1;1H").expect("terminal feed");
    assert_eq!((t.snapshot().cursor.row, t.snapshot().cursor.col), (0, 0));

    // Origin on: `\x1b[?6h`.
    t.feed(b"\x1b[?6h").expect("terminal feed");
    assert!(t.has_mode(TerminalMode::Origin));
    // CUP 1;1 => region top => (1,0).
    t.feed(b"\x1b[1;1H").expect("terminal feed");
    assert_eq!(
        (t.snapshot().cursor.row, t.snapshot().cursor.col),
        (1, 0),
        "origin CUP 1;1 must hit region top"
    );
    // CUP 2;1 => (2,0).
    t.feed(b"\x1b[2;1H").expect("terminal feed");
    assert_eq!((t.snapshot().cursor.row, t.snapshot().cursor.col), (2, 0));
    // CUP past bottom clamps to region bottom (row 4 0-indexed).
    // `\x1b[10;1H` with origin on.
    t.feed(b"\x1b[10;1H").expect("terminal feed");
    assert_eq!(
        (t.snapshot().cursor.row, t.snapshot().cursor.col),
        (4, 0),
        "origin CUP must clamp to region bottom"
    );

    // Origin off: `\x1b[?6l`, CUP 1;1 => (0,0) again.
    t.feed(b"\x1b[?6l").expect("terminal feed");
    assert!(!t.has_mode(TerminalMode::Origin));
    t.feed(b"\x1b[1;1H").expect("terminal feed");
    assert_eq!((t.snapshot().cursor.row, t.snapshot().cursor.col), (0, 0));
}

// 1049 regression guard for AltWriter.
// Bytes: enter `\x1b[?1049h`, exit `\x1b[?1049l`.
#[test]
fn alt_1049_enter_clears_alt_preserves_primary_exit_restores() {
    let mut t = term(10, 4);
    // Primary content + known cursor (0,4).
    t.feed(b"\x1b[1;1H").expect("terminal feed");
    t.feed(b"PRIM").expect("terminal feed");
    assert!(!t.has_mode(TerminalMode::AltScreen));
    let before = t.snapshot();
    assert_eq!((before.cursor.row, before.cursor.col), (0, 4));
    let primary_row0 = snap_row_text(&t, 0);
    assert!(primary_row0.starts_with("PRIM"));

    // Enter alt: `\x1b[?1049h`.
    t.feed(b"\x1b[?1049h").expect("terminal feed");
    assert!(t.has_mode(TerminalMode::AltScreen));
    // Alt must be cleared (all blank).
    for (i, row) in visible_text(&mut t).iter().enumerate() {
        assert!(row.trim().is_empty(), "alt row {i} must be blank, got {row:?}");
    }
    // Cursor carried into alt (swap_alt preserves position).
    assert_eq!((t.snapshot().cursor.row, t.snapshot().cursor.col), (0, 4));

    // Write in alt away from home.
    t.feed(b"\x1b[2;2H").expect("terminal feed");
    t.feed(b"ALT").expect("terminal feed");
    let mid = t.snapshot();
    assert_eq!((mid.cursor.row, mid.cursor.col), (1, 4));
    assert!(snap_row_text(&t, 1).contains("ALT"));

    // Exit alt: `\x1b[?1049l`.
    t.feed(b"\x1b[?1049l").expect("terminal feed");
    assert!(!t.has_mode(TerminalMode::AltScreen));
    // Primary restored incl. cursor.
    let restored = t.snapshot();
    assert_eq!((restored.cursor.row, restored.cursor.col), (0, 4));
    assert!(snap_row_text(&t, 0).starts_with("PRIM"));
    // Alt content must not leak into primary.
    for (i, row) in visible_text(&mut t).iter().enumerate() {
        assert!(!row.contains("ALT"), "primary row {i} leaked ALT: {row:?}");
    }
}

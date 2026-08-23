//! Deterministic headless OMP-like integration fixtures.
//! No PTY, no GPUI, no wall-clock — only public `mr_crabs-terminal` APIs.

use mr_crabs_terminal::{
    CursorShape, DamageKind, FramePool, GridSize, NamedColorValue, NormalizedColor, Terminal,
    frame_pool_default,
};

fn new_term() -> (Terminal, FramePool, GridSize) {
    let size = GridSize::new(10, 4);
    (Terminal::new(size).unwrap(), frame_pool_default(), size)
}

#[test]
fn sequence_and_damage_through_framedelta() {
    let (mut term, mut pool, size) = new_term();

    assert_eq!(term.next_sequence(), 0);
    let f0 = term.build_frame_delta(&mut pool);
    assert_eq!(f0.sequence, 0);
    assert_eq!(f0.damage, DamageKind::Full);
    assert_eq!(f0.rows.len(), size.rows as usize);
    assert!(f0.cursor.visible);
    assert_eq!(f0.cursor.shape, CursorShape::Block);
    pool.release(f0);

    // Idle rebuild increments sequence and yields Partial (cursor row touched).
    let seq = term.next_sequence();
    let f1 = term.build_frame_delta(&mut pool);
    assert_eq!(f1.sequence, seq);
    assert_eq!(f1.damage, DamageKind::Partial);
    assert_eq!(f1.rows.len(), 1);
    assert_eq!(f1.rows[0].row, f1.cursor.row);
    let prev_gen = f1.rows[0].generation;
    pool.release(f1);

    let f2 = term.build_frame_delta(&mut pool);
    assert_eq!(f2.sequence, seq + 1);
    assert_eq!(f2.damage, DamageKind::Partial);
    assert!(f2.rows[0].generation > prev_gen);
    pool.release(f2);
}

#[test]
fn sgr_red_and_wide_emoji_widths() {
    let (mut term, _pool, size) = new_term();
    term.feed(b"Hi").expect("terminal feed");
    term.feed(b"\x1b[31mR\x1b[0m").expect("terminal feed");
    term.feed("界".as_bytes()).expect("terminal feed");
    term.feed("🎉".as_bytes()).expect("terminal feed");

    let snap = term.snapshot();
    assert_eq!(snap.cursor.col, 7);
    assert!(!snap.cursor.wrap_pending);
    assert!(snap.styles.len() >= 2);

    let cols = size.cols as usize;
    let row0 = &snap.cells[0..cols];

    let r_style = row0[2].style as usize;
    let r_fg = &snap.styles[r_style].foreground;
    assert_eq!(*r_fg, NormalizedColor::Named(NamedColorValue::Red));
    // Cells after the SGR reset (界, 🎉 and spacers) must be default style.
    assert_eq!(row0[3].style, 0, "界 must be reset style");
    assert_eq!(row0[5].style, 0, "🎉 must be reset style");

    // Wide invariants.
    assert_eq!(row0[0].content, u32::from('H'));
    assert_eq!(row0[1].content, u32::from('i'));
    assert_eq!(row0[2].content, u32::from('R'));
    assert_eq!(row0[3].content, u32::from('界'));
    assert!(row0[3].flags & mr_crabs_terminal::Cell::WIDE != 0);
    assert!(row0[4].flags & mr_crabs_terminal::Cell::WIDE_SPACER != 0);
    assert_eq!(row0[5].content, u32::from('🎉'));
    assert!(row0[6].flags & mr_crabs_terminal::Cell::WIDE_SPACER != 0);
}

#[test]
fn exact_fill_wrap_pending_el_right_noop_and_cr_clear() {
    let (mut term, _pool, size) = new_term();

    // Fill exactly 10 cols -> wrap_pending without wrap.
    term.feed(b"\r").expect("terminal feed");
    term.feed(b"XXXXXXXXXX").expect("terminal feed");
    let snap = term.snapshot();
    assert_eq!(snap.cursor.row, 0);
    assert_eq!(snap.cursor.col, 9);
    assert!(snap.cursor.wrap_pending);
    let cols = size.cols as usize;
    assert!(
        snap.cells[0..cols]
            .iter()
            .all(|c| c.content == u32::from('X'))
    );

    // EL Right with wrap_pending is a no-op.
    term.feed(b"\x1b[K").expect("terminal feed");
    let snap2 = term.snapshot();
    assert!(snap2.cursor.wrap_pending);
    assert!(
        snap2.cells[0..cols]
            .iter()
            .all(|c| c.content == u32::from('X'))
    );

    // CR clears wrap_pending; EL Right from col 0 then clears the row.
    term.feed(b"\r").expect("terminal feed");
    assert!(!term.snapshot().cursor.wrap_pending);
    term.feed(b"\x1b[K").expect("terminal feed");
    let snap3 = term.snapshot();
    assert_eq!(snap3.cursor.col, 0);
    for (i, cell) in snap3.cells[0..cols].iter().enumerate() {
        assert_eq!(cell.content, u32::from(' '), "cell {i} not erased");
        assert_eq!(cell.style, 0, "erased cell {i} not default style");
    }
}

#[test]
fn printable_wrap_after_pending() {
    let (mut term, _pool, size) = new_term();
    term.feed(b"XXXXXXXXXX").expect("terminal feed"); // wrap_pending on row 0
    assert!(term.snapshot().cursor.wrap_pending);
    term.feed(b"Y").expect("terminal feed");
    let snap = term.snapshot();
    assert_eq!(snap.cursor.row, 1);
    assert_eq!(snap.cursor.col, 1);
    assert!(!snap.cursor.wrap_pending);
    let cols = size.cols as usize;
    assert!(
        snap.cells[0..cols]
            .iter()
            .all(|c| c.content == u32::from('X'))
    );
    assert_eq!(snap.cells[cols].content, u32::from('Y'));
    assert_eq!(snap.cells[cols + 1].content, u32::from(' '));
}

#[test]
fn cursor_visibility_shape_blink_through_frames() {
    let (mut term, mut pool, _size) = new_term();
    term.feed(b"\x1b[2;3H").expect("terminal feed");
    let pos = term.snapshot();
    assert_eq!((pos.cursor.row, pos.cursor.col), (1, 2));

    term.feed(b"\x1b[?25l").expect("terminal feed");
    let f = term.build_frame_delta(&mut pool);
    assert!(!f.cursor.visible);
    assert_eq!((f.cursor.row, f.cursor.col), (1, 2));
    pool.release(f);

    term.feed(b"\x1b[?25h").expect("terminal feed");
    let f = term.build_frame_delta(&mut pool);
    assert!(f.cursor.visible);
    assert_eq!(f.cursor.shape, CursorShape::Block);
    pool.release(f);

    term.feed(b"\x1b[2 q").expect("terminal feed");
    let f = term.build_frame_delta(&mut pool);
    assert_eq!(f.cursor.shape, CursorShape::Block);
    assert!(!f.cursor.blinking);
    pool.release(f);

    term.feed(b"\x1b[4 q").expect("terminal feed");
    let f = term.build_frame_delta(&mut pool);
    assert_eq!(f.cursor.shape, CursorShape::Underline);
    assert!(!f.cursor.blinking);
    pool.release(f);

    term.feed(b"\x1b[5 q").expect("terminal feed");
    let f = term.build_frame_delta(&mut pool);
    assert_eq!(f.cursor.shape, CursorShape::Bar);
    assert!(f.cursor.blinking);
    pool.release(f);

    term.feed(b"\x1b[0 q").expect("terminal feed");
    let f = term.build_frame_delta(&mut pool);
    assert_eq!(f.cursor.shape, CursorShape::Block);
    assert!(!f.cursor.blinking);
    pool.release(f);
}

#[test]
fn alt_screen_flag_and_primary_restoration() {
    let (mut term, mut pool, size) = new_term();
    term.feed(b"\x1b[1;1H").expect("terminal feed");
    term.feed(b"PRIM").expect("terminal feed");
    let before = term.snapshot();
    assert_eq!((before.cursor.row, before.cursor.col), (0, 4));

    term.feed(b"\x1b[?1049h").expect("terminal feed");
    let f = term.build_frame_delta(&mut pool);
    assert!(f.viewport.alternate_screen);
    assert_eq!((f.cursor.row, f.cursor.col), (0, 4));
    pool.release(f);

    term.feed(b"\x1b[2;2H").expect("terminal feed");
    term.feed(b"ALT").expect("terminal feed");
    let mid = term.snapshot();
    assert_eq!((mid.cursor.row, mid.cursor.col), (1, 4));

    term.feed(b"\x1b[?1049l").expect("terminal feed");
    let f = term.build_frame_delta(&mut pool);
    assert!(!f.viewport.alternate_screen);
    pool.release(f);

    let restored = term.snapshot();
    assert_eq!((restored.cursor.row, restored.cursor.col), (0, 4));
    let row0 = &restored.cells[0..size.cols as usize];
    assert_eq!(row0[0].content, u32::from('P'));
    assert_eq!(row0[1].content, u32::from('R'));
    assert_eq!(row0[2].content, u32::from('I'));
    assert_eq!(row0[3].content, u32::from('M'));
}

#[test]
fn visible_rows_snapshot_parity() {
    let (mut term, _pool, size) = new_term();
    term.feed(b"hello").expect("terminal feed");
    term.feed(b"\x1b[31mR\x1b[0m").expect("terminal feed");
    let snap = term.snapshot();
    let rows = term.visible_rows();
    assert_eq!(rows.len(), size.rows as usize);
    let cols = size.cols as usize;
    for (r, row) in rows.iter().enumerate() {
        let expect = &snap.cells[r * cols..(r + 1) * cols];
        assert_eq!(row.as_slice(), expect, "row {r} mismatch");
    }
}

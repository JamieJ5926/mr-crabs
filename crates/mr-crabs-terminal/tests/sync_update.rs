//! Headless tests for synchronized-update batching (`?2026`), plus 1047
//! alt isolation and 1048 cursor save/restore.
//!
//! Public `Terminal` surface only. Engine internals stay with SyncImpl.

use mr_crabs_terminal::{
    DamageKind, FrameDelta, FramePool, GridSize, NormalizedSnapshot, Terminal, TerminalMode,
    frame_pool_default,
};

const BSU: &[u8] = b"\x1b[?2026h";
const ESU: &[u8] = b"\x1b[?2026l";
const DECRQM_2026: &[u8] = b"\x1b[?2026$p";
const DECRQM_1049: &[u8] = b"\x1b[?1049$p";
const ALT_1047H: &[u8] = b"\x1b[?1047h";
const ALT_1047L: &[u8] = b"\x1b[?1047l";
const CURSOR_1048H: &[u8] = b"\x1b[?1048h";
const CURSOR_1048L: &[u8] = b"\x1b[?1048l";
const ALT_1049H: &[u8] = b"\x1b[?1049h";
const ALT_1049L: &[u8] = b"\x1b[?1049l";

fn new_term(cols: u16, rows: u16) -> (Terminal, FramePool, GridSize) {
    let size = GridSize::new(cols, rows);
    (
        Terminal::new(size).expect("terminal"),
        frame_pool_default(),
        size,
    )
}

fn drain_initial_frame(term: &mut Terminal, pool: &mut FramePool) {
    let frame = term.build_frame_delta(pool);
    pool.release(frame);
}

fn row_text(snap: &NormalizedSnapshot, row: usize) -> String {
    let cols = usize::from(snap.size.cols);
    let start = row * cols;
    snap.cells[start..start + cols]
        .iter()
        .map(|cell| char::from_u32(cell.content).unwrap_or(' '))
        .collect()
}

fn visible_text(snap: &NormalizedSnapshot) -> Vec<String> {
    (0..usize::from(snap.size.rows))
        .map(|row| row_text(snap, row))
        .collect()
}

fn is_paint(frame: &FrameDelta) -> bool {
    frame.damage != DamageKind::Clean && !frame.rows.is_empty()
}

fn paint_count(frames: &[FrameDelta]) -> usize {
    frames.iter().filter(|frame| is_paint(frame)).count()
}

fn full_frame_body(size: GridSize) -> Vec<u8> {
    let cells = usize::from(size.cols) * usize::from(size.rows);
    (0..cells)
        .map(|i| b'A' + (i % 26) as u8)
        .collect()
}

#[test]
fn decrqm_2026_reports_set_inside_bsu_block() {
    let (mut term, mut pool, _size) = new_term(10, 4);
    drain_initial_frame(&mut term, &mut pool);

    term.feed(ALT_1049H).expect("1049h");
    assert!(
        term.has_mode(TerminalMode::AltScreen),
        "calibrate ?1049: alt must be set after 1049h"
    );
    term.feed(DECRQM_1049).expect("decrqm 1049");
    assert!(
        term.has_mode(TerminalMode::AltScreen),
        "calibrate ?1049: DECRQM must not drop alt"
    );
    term.feed(ALT_1049L).expect("1049l");
    assert!(!term.has_mode(TerminalMode::AltScreen));

    assert!(
        !term.is_sync_update(),
        "idle terminal must report sync-update reset"
    );
    term.feed(DECRQM_2026).expect("decrqm 2026 idle");
    assert!(
        !term.is_sync_update(),
        "DECRQM 2026 idle must stay reset"
    );

    term.feed(BSU).expect("bsu");
    assert!(
        term.is_sync_update(),
        "DECRQM 2026 must report set (active) inside the BSU block"
    );
    term.feed(DECRQM_2026).expect("decrqm 2026 active");
    assert!(
        term.is_sync_update(),
        "DECRQM query inside the block must leave sync active"
    );

    term.feed(ESU).expect("esu");
    assert!(
        !term.is_sync_update(),
        "ESU must report sync-update reset"
    );
}

#[test]
fn wrapped_full_frame_emits_one_coalesced_paint() {
    let (mut term, mut pool, size) = new_term(8, 4);
    drain_initial_frame(&mut term, &mut pool);

    let body = full_frame_body(size);
    let mut mid_frames = Vec::new();

    term.feed(BSU).expect("bsu");
    assert!(term.is_sync_update());
    mid_frames.push(term.build_frame_delta(&mut pool));

    for chunk in body.chunks(usize::from(size.cols)) {
        term.feed(chunk).expect("sync body chunk");
        mid_frames.push(term.build_frame_delta(&mut pool));
    }

    assert!(
        mid_frames.iter().all(|frame| !is_paint(frame)),
        "builds inside ?2026 must be Clean with no row paint, got damages {:?}",
        mid_frames
            .iter()
            .map(|frame| frame.damage)
            .collect::<Vec<_>>()
    );
    assert_eq!(
        paint_count(&mid_frames),
        0,
        "no paint may escape the BSU/ESU window"
    );

    let during = term.snapshot();
    assert!(
        visible_text(&during)
            .iter()
            .all(|row| row.chars().all(|ch| ch == ' ')),
        "VTE must hold the full-frame body until ESU, got {during_text:?}",
        during_text = visible_text(&during)
    );

    term.feed(ESU).expect("esu");
    assert!(!term.is_sync_update());
    let coalesced = term.build_frame_delta(&mut pool);
    assert!(
        is_paint(&coalesced),
        "the first build after ESU must be the one coalesced paint, damage={:?}",
        coalesced.damage
    );
    assert_eq!(
        paint_count(&mid_frames) + usize::from(is_paint(&coalesced)),
        1,
        "wrapped full-frame must emit exactly one paint"
    );

    let after = term.snapshot();
    assert_eq!(row_text(&after, 0).as_bytes(), &body[..usize::from(size.cols)]);
    let last = usize::from(size.rows) - 1;
    let start = last * usize::from(size.cols);
    assert_eq!(
        row_text(&after, last).as_bytes(),
        &body[start..start + usize::from(size.cols)]
    );

    for frame in mid_frames {
        pool.release(frame);
    }
    pool.release(coalesced);
}

#[test]
fn unwrapped_output_matches_wrapped_sync_block() {
    let size = GridSize::new(8, 4);
    let body = full_frame_body(size);

    let (mut wrapped, mut wrap_pool, _) = new_term(size.cols, size.rows);
    drain_initial_frame(&mut wrapped, &mut wrap_pool);
    wrapped.feed(BSU).expect("bsu");
    wrapped.feed(&body).expect("wrapped body");
    wrapped.feed(ESU).expect("esu");
    let wrap_frame = wrapped.build_frame_delta(&mut wrap_pool);
    let wrap_snap = wrapped.snapshot();
    wrap_pool.release(wrap_frame);

    let (mut plain, mut plain_pool, _) = new_term(size.cols, size.rows);
    drain_initial_frame(&mut plain, &mut plain_pool);
    plain.feed(&body).expect("plain body");
    let plain_frame = plain.build_frame_delta(&mut plain_pool);
    let plain_snap = plain.snapshot();
    plain_pool.release(plain_frame);

    assert_eq!(wrap_snap.cells, plain_snap.cells);
    assert_eq!(wrap_snap.cursor, plain_snap.cursor);
    assert_eq!(wrap_snap.size, plain_snap.size);
    assert_eq!(visible_text(&wrap_snap), visible_text(&plain_snap));
}

#[test]
fn mode_1047_isolates_alt_content_from_primary() {
    let (mut term, mut pool, size) = new_term(8, 3);
    drain_initial_frame(&mut term, &mut pool);

    term.feed(b"\x1b[1;1HPRIM").expect("primary text");
    let primary = term.snapshot();
    assert_eq!(&row_text(&primary, 0)[..4], "PRIM");
    assert!(!term.has_mode(TerminalMode::AltScreen));
    let primary_cursor = primary.cursor;

    term.feed(ALT_1047H).expect("1047h");
    assert!(
        term.has_mode(TerminalMode::AltScreen),
        "1047h must enter the alternate buffer"
    );
    let entered = term.snapshot();
    assert!(
        visible_text(&entered)
            .iter()
            .all(|row| row.chars().all(|ch| ch == ' ')),
        "1047h must present a blank alt buffer, got {:?}",
        visible_text(&entered)
    );

    term.feed(b"\x1b[1;1HALT!").expect("alt text");
    let alt = term.snapshot();
    assert_eq!(&row_text(&alt, 0)[..4], "ALT!");
    assert_ne!(row_text(&alt, 0), row_text(&primary, 0));

    term.feed(ALT_1047L).expect("1047l");
    assert!(
        !term.has_mode(TerminalMode::AltScreen),
        "1047l must leave the alternate buffer"
    );
    let restored = term.snapshot();
    assert_eq!(
        &row_text(&restored, 0)[..4],
        "PRIM",
        "1047 must isolate alt writes from primary"
    );
    assert_eq!(
        restored.cells[..usize::from(size.cols)],
        primary.cells[..usize::from(size.cols)]
    );
    assert_eq!(
        restored.cursor, primary_cursor,
        "1047 restores the primary buffer, including its cursor; 1048 is same-screen save/restore"
    );
}

#[test]
fn mode_1048_saves_and_restores_cursor() {
    let (mut term, mut pool, _size) = new_term(10, 4);
    drain_initial_frame(&mut term, &mut pool);

    term.feed(b"\x1b[2;3H").expect("goto 2;3");
    let saved = term.snapshot().cursor;
    assert_eq!((saved.row, saved.col), (1, 2));

    term.feed(CURSOR_1048H).expect("1048h");
    assert_eq!(
        term.snapshot().cursor, saved,
        "1048h is save-only and must not move the cursor"
    );
    assert!(
        !term.has_mode(TerminalMode::AltScreen),
        "1048 must not enter alt"
    );

    term.feed(b"\x1b[4;8H").expect("goto 4;8");
    let moved = term.snapshot().cursor;
    assert_eq!((moved.row, moved.col), (3, 7));
    assert_ne!(moved, saved);

    term.feed(CURSOR_1048L).expect("1048l");
    let restored = term.snapshot().cursor;
    assert_eq!(
        restored, saved,
        "1048l must restore the cursor saved by 1048h"
    );
    assert!(
        !term.has_mode(TerminalMode::AltScreen),
        "1048l must not touch alt"
    );
}

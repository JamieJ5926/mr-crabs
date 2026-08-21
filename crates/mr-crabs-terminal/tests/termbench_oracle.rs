//! Termbench sampled oracle + compaction acceptance (Wave H).
//!
//! Generator contract is pinned to cmuratori/termbench 82afbc69256b4e22de:
//! inclusive X=0..=80, Y=0..=24 (2025 cells/frame), CUP per row as
//! `ESC[(Y+1);1H`, FGPerChar `ESC[38;2;R;G;Bm` with
//! `R=(F&255) G=(F+Y&255) B=(F+Y+X&255)`, FGBGPerChar background
//! `ESC[48;2;(F+Y+X&255);(F+Y&255);(F&255)m` then same FG SGR, char
//! `'a'+((F+X+Y)%25)`, frames Small=512 Normal=8192.
//!
//! Small 512-frame FG/FGBG payloads must compact without overflow and retain
//! sampled final-frame FG/BG semantics identical to the short-prefix oracle.

use mr_crabs_terminal::{GridSize, NamedColorValue, NormalizedColor, Terminal};

const WIDTH_INCLUSIVE: u32 = 80;
const HEIGHT_INCLUSIVE: u32 = 24;
const COLS: usize = 81;
const ROWS: usize = 25;
const CELLS_PER_FRAME: usize = 2025;
const SMALL_FRAMES: usize = 512;
const NORMAL_FRAMES: usize = 8192;

#[inline]
fn expected_fg_rgb(frame: u32, y: u32, x: u32) -> [u8; 3] {
    [
        (frame & 255) as u8,
        ((frame + y) & 255) as u8,
        ((frame + y + x) & 255) as u8,
    ]
}

#[inline]
fn expected_bg_rgb(frame: u32, y: u32, x: u32) -> [u8; 3] {
    [
        ((frame + y + x) & 255) as u8,
        ((frame + y) & 255) as u8,
        (frame & 255) as u8,
    ]
}

#[inline]
fn expected_char(frame: u32, y: u32, x: u32) -> u8 {
    b'a' + (((frame + x + y) % 25) as u8)
}

#[inline]
fn append_decimal(out: &mut Vec<u8>, v: u32) {
    if v == 0 {
        out.push(b'0');
        return;
    }
    let mut buf = [0u8; 10];
    let mut len = 0usize;
    let mut x = v;
    while x > 0 {
        buf[len] = b'0' + (x % 10) as u8;
        x /= 10;
        len += 1;
    }
    for i in (0..len).rev() {
        out.push(buf[i]);
    }
}

#[inline]
fn append_color(out: &mut Vec<u8>, fg: bool, r: u8, g: u8, b: u8) {
    if fg {
        out.extend_from_slice(b"\x1b[38;2;");
    } else {
        out.extend_from_slice(b"\x1b[48;2;");
    }
    append_decimal(out, r as u32);
    out.push(b';');
    append_decimal(out, g as u32);
    out.push(b';');
    append_decimal(out, b as u32);
    out.push(b'm');
}

#[inline]
fn append_cup(out: &mut Vec<u8>, x: u32, y: u32) {
    out.extend_from_slice(b"\x1b[");
    append_decimal(out, y);
    out.push(b';');
    append_decimal(out, x);
    out.push(b'H');
}

fn append_frame(out: &mut Vec<u8>, fgbg: bool, frame: u32) {
    for y in 0..=HEIGHT_INCLUSIVE {
        append_cup(out, 1, 1 + y);
        for x in 0..=WIDTH_INCLUSIVE {
            if fgbg {
                let [br, bg, bb] = expected_bg_rgb(frame, y, x);
                let [fr, fg, fb] = expected_fg_rgb(frame, y, x);
                append_color(out, false, br, bg, bb);
                append_color(out, true, fr, fg, fb);
            } else {
                let [fr, fg, fb] = expected_fg_rgb(frame, y, x);
                append_color(out, true, fr, fg, fb);
            }
            out.push(expected_char(frame, y, x));
        }
    }
}

fn frame_payload(fgbg: bool, frame: u32) -> Vec<u8> {
    let mut out = Vec::with_capacity(80 * 1024);
    append_frame(&mut out, fgbg, frame);
    out
}

fn term_81x25() -> Terminal {
    Terminal::new(GridSize::new(81, 25)).unwrap()
}

fn style_at(
    snap: &mr_crabs_terminal::NormalizedSnapshot,
    row: u16,
    col: u16,
) -> mr_crabs_terminal::Style {
    let cols = usize::from(snap.size.cols);
    let idx = usize::from(row) * cols + usize::from(col);
    let cell = &snap.cells[idx];
    snap.styles[usize::from(cell.style)].clone()
}

/// FGPerChar Small (512 frames) compacts without overflow and sampled
/// final-frame FG/BG matches the pinned generator.
#[test]
fn termbench_fg_small_compacts_without_overflow() {
    let mut term = term_81x25();
    let mut cfg = term.scrollback_config();
    cfg.max_lines = 0;
    term.set_scrollback_config(cfg);
    for f in 0..SMALL_FRAMES as u32 {
        term.feed(&frame_payload(false, f))
            .expect("terminal feed must succeed without overflow after compaction");
    }
    let last = (SMALL_FRAMES - 1) as u32;
    let snap = term.snapshot();
    for &(y, x) in &[(0u16, 0u16), (0, 80), (12, 40), (24, 80), (24, 0)] {
        let cols = usize::from(snap.size.cols);
        let idx = usize::from(y) * cols + usize::from(x);
        let cell = &snap.cells[idx];
        assert!(
            !cell.is_default(),
            "sampled cell should be non-default at {y},{x}"
        );
        assert_eq!(
            cell.content as u8,
            expected_char(last, y as u32, x as u32),
            "char mismatch at {y},{x}"
        );
        let resolved = style_at(&snap, y, x);
        let exp_fg = expected_fg_rgb(last, y as u32, x as u32);
        assert_eq!(
            resolved.foreground,
            NormalizedColor::Rgb(exp_fg),
            "FG mismatch at {y},{x} frame {last}"
        );
        assert_eq!(
            resolved.background,
            NormalizedColor::Named(NamedColorValue::Background),
            "BG should remain default for FGPerChar at {y},{x}"
        );
    }
}

/// FGBGPerChar Small (512 frames) compacts without overflow and sampled
/// final-frame FG/BG matches the pinned generator.
#[test]
fn termbench_fgbg_small_compacts_without_overflow() {
    let mut term = term_81x25();
    let mut cfg = term.scrollback_config();
    cfg.max_lines = 0;
    term.set_scrollback_config(cfg);
    for f in 0..SMALL_FRAMES as u32 {
        term.feed(&frame_payload(true, f))
            .expect("terminal feed must succeed without overflow after compaction");
    }
    let last = (SMALL_FRAMES - 1) as u32;
    let snap = term.snapshot();
    for &(y, x) in &[(0u16, 0u16), (12, 40), (24, 80)] {
        let cols = usize::from(snap.size.cols);
        let idx = usize::from(y) * cols + usize::from(x);
        let cell = &snap.cells[idx];
        assert!(
            !cell.is_default(),
            "FGBG cell should be non-default at {y},{x}"
        );
        assert_eq!(cell.content as u8, expected_char(last, y as u32, x as u32));
        let resolved = style_at(&snap, y, x);
        assert_eq!(
            resolved.foreground,
            NormalizedColor::Rgb(expected_fg_rgb(last, y as u32, x as u32)),
            "FGBG FG mismatch at {y},{x}"
        );
        assert_eq!(
            resolved.background,
            NormalizedColor::Rgb(expected_bg_rgb(last, y as u32, x as u32)),
            "FGBG BG mismatch at {y},{x}"
        );
    }
}

/// Short prefix (8 frames) is overflow-safe and sampled semantics hold.
/// Resolves Style values (foreground/background) via snapshot — not only chars.
#[test]
fn termbench_sampled_oracle_short_prefix() {
    let mut term = term_81x25();
    let mut cfg = term.scrollback_config();
    cfg.max_lines = 0;
    term.set_scrollback_config(cfg);
    let frames = 8u32;
    for f in 0..frames {
        term.feed(&frame_payload(false, f)).expect("terminal feed");
    }
    let last = frames - 1;
    let snap = term.snapshot();
    for &(y, x) in &[(0u16, 0u16), (0, 80), (12, 40), (24, 80), (24, 0)] {
        let cols = usize::from(snap.size.cols);
        let idx = usize::from(y) * cols + usize::from(x);
        let cell = &snap.cells[idx];
        assert!(
            !cell.is_default(),
            "sampled cell should be non-default at {y},{x}"
        );
        assert_eq!(
            cell.content as u8,
            expected_char(last, y as u32, x as u32),
            "char mismatch at {y},{x}"
        );
        let resolved = style_at(&snap, y, x);
        let exp_fg = expected_fg_rgb(last, y as u32, x as u32);
        assert_eq!(
            resolved.foreground,
            NormalizedColor::Rgb(exp_fg),
            "FG mismatch at {y},{x} frame {last}"
        );
        // FGPerChar keeps default background.
        assert_eq!(
            resolved.background,
            NormalizedColor::Named(NamedColorValue::Background),
            "BG should remain default for FGPerChar at {y},{x}"
        );
    }

    // FGBG short prefix: both FG and BG are per-char Rgb.
    let mut term2 = term_81x25();
    let mut cfg2 = term2.scrollback_config();
    cfg2.max_lines = 0;
    term2.set_scrollback_config(cfg2);
    for f in 0..frames {
        term2.feed(&frame_payload(true, f)).expect("terminal feed");
    }
    let snap2 = term2.snapshot();
    for &(y, x) in &[(0u16, 0u16), (12, 40), (24, 80)] {
        let cols = usize::from(snap2.size.cols);
        let idx = usize::from(y) * cols + usize::from(x);
        let cell = &snap2.cells[idx];
        assert!(
            !cell.is_default(),
            "FGBG cell should be non-default at {y},{x}"
        );
        assert_eq!(cell.content as u8, expected_char(last, y as u32, x as u32));
        let resolved = style_at(&snap2, y, x);
        assert_eq!(
            resolved.foreground,
            NormalizedColor::Rgb(expected_fg_rgb(last, y as u32, x as u32)),
            "FGBG FG mismatch at {y},{x}"
        );
        assert_eq!(
            resolved.background,
            NormalizedColor::Rgb(expected_bg_rgb(last, y as u32, x as u32)),
            "FGBG BG mismatch at {y},{x}"
        );
    }
}

#[test]
fn termbench_cells_per_frame_and_counts_are_pinned() {
    assert_eq!(CELLS_PER_FRAME, 2025);
    assert_eq!((WIDTH_INCLUSIVE + 1) * (HEIGHT_INCLUSIVE + 1), 2025);
    assert_eq!(COLS * ROWS, 2025);
    assert_eq!(COLS, 81);
    assert_eq!(ROWS, 25);
    assert_eq!(SMALL_FRAMES, 512);
    assert_eq!(NORMAL_FRAMES, 8192);
    // Inclusive ranges: X=0..=80 (81 cols), Y=0..=24 (25 rows).
    assert_eq!(
        (0..=WIDTH_INCLUSIVE).count() * (0..=HEIGHT_INCLUSIVE).count(),
        CELLS_PER_FRAME
    );
}

#[test]
fn termbench_alternate_screen_round_trips_short_payload() {
    let mut term = term_81x25();
    // Seed primary with a known cell so we can verify restoration.
    term.feed(b"\x1b[38;2;255;0;0m\x1b[1;1HX")
        .expect("terminal feed");
    let primary_snap = term.snapshot();
    let primary_cell = primary_snap.cells[0];
    let primary_style = primary_snap.styles[usize::from(primary_cell.style)].clone();

    term.feed(b"\x1b[?1049h").expect("terminal feed");
    assert!(
        term.snapshot()
            .modes
            .contains(&mr_crabs_terminal::TerminalMode::AltScreen)
    );
    for f in 0..4u32 {
        term.feed(&frame_payload(false, f)).expect("terminal feed");
    }
    // Verify alt payload is visible while in alt screen.
    let alt_snap = term.snapshot();
    let expected = expected_char(3, 0, 0);
    let alt_cell = &alt_snap.cells[0];
    assert_eq!(
        alt_cell.content as u8, expected,
        "alt screen char should reflect last alt frame"
    );
    let alt_style = &alt_snap.styles[usize::from(alt_cell.style)];
    assert_eq!(
        alt_style.foreground,
        NormalizedColor::Rgb(expected_fg_rgb(3, 0, 0))
    );

    term.feed(b"\x1b[?1049l").expect("terminal feed");
    assert!(
        !term
            .snapshot()
            .modes
            .contains(&mr_crabs_terminal::TerminalMode::AltScreen)
    );
    let restored = term.snapshot();
    assert_eq!(restored.size, GridSize::new(81, 25));
    // Primary content must be restored exactly.
    let restored_cell = &restored.cells[0];
    assert_eq!(
        restored_cell.content, primary_cell.content,
        "primary cell content should round-trip through alt screen"
    );
    let restored_style = &restored.styles[usize::from(restored_cell.style)];
    assert_eq!(
        *restored_style, primary_style,
        "primary style should round-trip through alt screen"
    );
}

#[test]
fn termbench_saved_pen_and_reset_is_stable() {
    let mut term = term_81x25();
    // Set a distinct pen, save it, then perturb with termbench payload.
    term.feed(b"\x1b[38;2;10;20;30m").expect("terminal feed");
    term.feed(b"\x1b7").expect("terminal feed"); // DECSC save (cursor + pen)
    for f in 0..4u32 {
        term.feed(&frame_payload(false, f)).expect("terminal feed");
    }
    term.feed(b"\x1b8").expect("terminal feed"); // DECRC restore
    // After restore, next cells should use the saved pen (10,20,30).
    term.feed(b"\x1b[1;2H").expect("terminal feed"); // move to known col to avoid overwrite of termbench fill
    term.feed(b"Q").expect("terminal feed");
    let snap = term.snapshot();
    let q_cell = &snap.cells[1]; // row 0 col 1
    assert_eq!(q_cell.content as u8, b'Q');
    let q_style = &snap.styles[usize::from(q_cell.style)];
    assert_eq!(
        q_style.foreground,
        NormalizedColor::Rgb([10, 20, 30]),
        "saved pen FG should restore after termbench churn"
    );
    // Explicit reset must clear to default.
    term.feed(b"\x1b[0m").expect("terminal feed");
    term.feed(b"\x1b[1;3H").expect("terminal feed");
    term.feed(b"R").expect("terminal feed");
    let snap2 = term.snapshot();
    let r_cell = &snap2.cells[2];
    assert_eq!(r_cell.content as u8, b'R');
    let r_style = &snap2.styles[usize::from(r_cell.style)];
    assert_eq!(
        r_style.foreground,
        NormalizedColor::Named(NamedColorValue::Foreground)
    );
    assert_eq!(
        r_style.background,
        NormalizedColor::Named(NamedColorValue::Background)
    );
}

#[test]
fn termbench_hot_history_drain_is_overflow_safe_for_short_payload() {
    let mut term = term_81x25();
    let mut cfg = term.scrollback_config();
    cfg.max_lines = 10_000;
    term.set_scrollback_config(cfg);
    for f in 0..8u32 {
        term.feed(&frame_payload(false, f)).expect("terminal feed");
    }
    // Drain without re-enqueue: hot pages remain bounded.
    term.drain_compression();
    assert!(
        term.history_len() < 100_000,
        "history must remain bounded after short payload"
    );
    // Visible grid must remain exact after drain.
    let snap = term.snapshot();
    let last = 7u32;
    for &(y, x) in &[(0u16, 0u16), (12, 40), (24, 80)] {
        let cols = usize::from(snap.size.cols);
        let idx = usize::from(y) * cols + usize::from(x);
        let cell = &snap.cells[idx];
        assert_eq!(cell.content as u8, expected_char(last, y as u32, x as u32));
        let style = &snap.styles[usize::from(cell.style)];
        assert_eq!(
            style.foreground,
            NormalizedColor::Rgb(expected_fg_rgb(last, y as u32, x as u32))
        );
    }
    // History reads must not corrupt or panic (termbench itself does not scroll,
    // so history may be empty — the check is that reads are safe).
    let mut out = Vec::new();
    if term.history_len() > 0 {
        assert!(term.read_history_line(0, &mut out));
        assert!(!out.is_empty());
    }
}

#[test]
fn termbench_cold_history_round_trips_short_payload() {
    let mut term = term_81x25();
    let mut cfg = term.scrollback_config();
    cfg.max_lines = 10_000;
    term.set_scrollback_config(cfg);
    for f in 0..8u32 {
        term.feed(&frame_payload(false, f)).expect("terminal feed");
    }
    term.drain_compression();
    // Force cold compression and full restore: styles must survive the cycle.
    term.force_compress_all();
    term.force_restore_all();
    let snap = term.snapshot();
    assert_eq!(snap.size, GridSize::new(81, 25));
    let last = 7u32;
    for &(y, x) in &[(0u16, 0u16), (12, 40)] {
        let cols = usize::from(snap.size.cols);
        let idx = usize::from(y) * cols + usize::from(x);
        let cell = &snap.cells[idx];
        assert_eq!(
            cell.content as u8,
            expected_char(last, y as u32, x as u32),
            "char should survive cold round-trip at {y},{x}"
        );
        let style = &snap.styles[usize::from(cell.style)];
        assert_eq!(
            style.foreground,
            NormalizedColor::Rgb(expected_fg_rgb(last, y as u32, x as u32)),
            "FG should survive cold round-trip at {y},{x}"
        );
    }
}

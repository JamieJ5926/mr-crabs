//! Paste/focus verify: public-API proofs for focus reports, bracketed paste,
//! non-bracketed CR, sanitize strip list, and cursor app/normal paths.
//!
//! Reproduce-first source (READ-ONLY, HEAD 6bc20c0,
//! crates/mr-crabs-input/src/lib.rs):
//! - encode_focus 985-994, sanitize_paste 997-1012, encode_paste 1014-1031,
//!   encode_cursor 522-528 via NumpadUp trail 419.
//! Protocol refs (terminal-owned, READ-ONLY):
//! - mr-crabs-terminal/src/lib.rs:845-853/873-881: FocusInOut => ESC[?1004h/l,
//!   BracketedPaste => ESC[?2004h/l.
//! DECSCUSR disposition: terminal-owned, NO input-crate bytes. Rendered by
//! terminal feed `ESC[<n> q` + `build_frame_delta` (mr-crabs-terminal/src/lib.rs
//! tests 2242-2267, tests/omp_like_headless.rs:157-179). This file asserts only
//! input-side public API (encode_key/encode_focus/encode_paste) and marks
//! DECSCUSR render out-of-scope; see `decscusr_is_terminal_owned` below.

use mr_crabs_input::{Key, KeyEvent, KeyboardMode, encode_focus, encode_key, encode_paste, sanitize_paste};

#[test]
fn focus_in_report_is_esc_bracket_cap_i() {
    let mut out = Vec::new();
    encode_focus(true, true, &mut out);
    assert_eq!(out, b"\x1b[I", "focus-in enabled must be ESC[I (1b 5b 49)");
}

#[test]
fn focus_out_report_is_esc_bracket_cap_o() {
    let mut out = Vec::new();
    encode_focus(false, true, &mut out);
    assert_eq!(out, b"\x1b[O", "focus-out enabled must be ESC[O (1b 5b 4f)");
}

#[test]
fn focus_disabled_emits_no_bytes() {
    let mut out = Vec::new();
    encode_focus(true, false, &mut out);
    assert!(out.is_empty(), "focus-in disabled must emit nothing");
    encode_focus(false, false, &mut out);
    assert!(out.is_empty(), "focus-out disabled must emit nothing");
}

#[test]
fn bracketed_paste_wraps_with_200_201() {
    let mut out = Vec::new();
    encode_paste("hi", true, &mut out);
    assert_eq!(
        out,
        b"\x1b[200~hi\x1b[201~",
        "bracketed paste must wrap: ESC[200~ hi ESC[201~"
    );
}

#[test]
fn bracketed_paste_preserves_newline() {
    let mut out = Vec::new();
    encode_paste("hi\nthere", true, &mut out);
    assert_eq!(
        out,
        b"\x1b[200~hi\nthere\x1b[201~",
        "bracketed paste must preserve LF (0a) as-is"
    );
}

#[test]
fn non_bracketed_paste_maps_newline_to_cr() {
    let mut out = Vec::new();
    encode_paste("hi\nthere", false, &mut out);
    assert_eq!(
        out,
        b"hi\rthere",
        "non-bracketed paste must map LF (0a) to CR (0d)"
    );
}

#[test]
fn sanitize_strips_control_bytes_to_space() {
    // Exact STRIP list from sanitize_paste (lib.rs:998-1001).
    const STRIP: &[u8] = &[
        0x00, 0x08, 0x05, 0x04, 0x1b, 0x7f, 0x03, 0x1c, 0x15, 0x1a, 0x11, 0x13, 0x17, 0x16, 0x12,
        0x0f,
    ];
    for &b in STRIP {
        // Every STRIP byte is a single valid ASCII byte, so from_utf8 always
        // succeeds; no fallback needed (NUL included: valid UTF-8).
        let s = String::from_utf8(vec![b'a', b, b'c'])
            .unwrap_or_else(|_| panic!("STRIP byte 0x{b:02x} must be valid UTF-8"));
        let got = sanitize_paste(&s);
        assert_eq!(got, "a c", "byte 0x{b:02x} must sanitize to space");
    }
}

#[test]
fn sanitize_keeps_newline_and_unicode() {
    assert_eq!(sanitize_paste("a\nb"), "a\nb", "LF must survive sanitize");
    assert_eq!(sanitize_paste("aéb"), "aéb", "unicode must survive sanitize");
    assert_eq!(
        sanitize_paste("a\x00b\x1bc"),
        "a b c",
        "NUL + ESC must become spaces"
    );
}

#[test]
fn cursor_normal_mode_is_esc_bracket_via_public_api() {
    // Private encode_cursor(application=false, b'A') reached via public
    // encode_key NumpadUp trail (lib.rs:419). No lib edits.
    let mode = KeyboardMode::default();
    assert!(
        !mode.cursor_key_application,
        "default mode must be normal cursor"
    );
    let mut out = Vec::new();
    encode_key(&KeyEvent::new(Key::NumpadUp), &mode, &mut out);
    assert_eq!(out, b"\x1b[A", "normal NumpadUp must be ESC[A (1b 5b 41)");
}

#[test]
fn cursor_application_mode_is_esc_o_via_public_api() {
    let mut mode = KeyboardMode::default();
    mode.cursor_key_application = true;
    let mut out = Vec::new();
    encode_key(&KeyEvent::new(Key::NumpadUp), &mode, &mut out);
    assert_eq!(
        out,
        b"\x1bOA",
        "application NumpadUp must be ESC O A (1b 4f 41)"
    );
}

#[test]
fn decscusr_is_terminal_owned_no_input_bytes() {
    // DECSCUSR (`ESC[<n> q`, e.g. `ESC[4 q` steady underline) is rendered by
    // the terminal engine (`term.feed` + `build_frame_delta`), NOT by the
    // input crate. Evidence (READ-ONLY):
    // - mr-crabs-terminal/src/lib.rs:2242-2267 (feed ESC[4 q / ESC[5 q /
    //   ESC[2 q, assert CursorShape via frame.cursor)
    // - mr-crabs-terminal/tests/omp_like_headless.rs:157-179 (same via
    //   public headless API)
    // The input crate exposes no DECSCUSR encoder; grep for ` q` / DECSCUSR
    // in mr-crabs-input/src/lib.rs finds nothing. This test pins that the
    // input-side cursor path (encode_key NumpadUp) emits cursor *keys*
    // (ESC[A / ESC O A), never cursor *style* (` q`) bytes.
    let mut out = Vec::new();
    encode_key(
        &KeyEvent::new(Key::NumpadUp),
        &KeyboardMode::default(),
        &mut out,
    );
    assert!(
        !out.ends_with(b" q") && !out.contains(&b'q'),
        "input cursor-key bytes must never contain DECSCUSR ` q`: got {out:?}"
    );
}

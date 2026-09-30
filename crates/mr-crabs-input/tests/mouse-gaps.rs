//! Mouse gaps: selector table for `MouseMode::from_modes` / `from_modes_with`
//! plus SGR / Urxvt / SGR-pixels wire-byte proofs.
//!
//! Reproduce-first source (READ-ONLY, HEAD 6bc20c0,
//! crates/mr-crabs-input/src/lib.rs:792-981 pre-fix):
//! - `from_modes` returned ONLY X10/Utf8/Sgr, NEVER Urxvt/SgrPixels, yet
//!   `encode_mouse` had live Urxvt + SgrPixels branches (dead variants).
//! - `TerminalMode` carries no 1015/1016 bits (mr-crabs-terminal lib.rs:353-377);
//!   protocol.rs:1258-1281 maps only 1000/1002/1003/1004/1005/1006/2004.
//! - Fix: `MouseModeOverlay { urxvt_mouse (DEC 1015), sgr_pixels (DEC 1016) }`
//!   + `from_modes_with`; `from_modes` delegates with default overlay
//!   (backward-compat). Precedence: pixels > urxvt > Sgr(1006) > Utf8(1005) > X10.
//! - SGR press col10,row5 (0-indexed) -> ESC[<0;11;6M
//!   (1b 5b 3c 30 3b 31 31 3b 36 4d); release -> ESC[<0;11;6m (... 6d).

use mr_crabs_input::{
    Modifiers, MouseAction, MouseButton, MouseEncoding, MouseEvent, MouseMode,
    MouseModeOverlay, MouseReportMode, encode_mouse,
};
use mr_crabs_terminal::TerminalMode;

fn press(col: u16, row: u16) -> MouseEvent {
    MouseEvent {
        button: Some(MouseButton::Left),
        action: MouseAction::Press,
        mods: Modifiers::NONE,
        col,
        row,
        x_px: 0,
        y_px: 0,
    }
}

#[test]
fn from_modes_selector_table_report_and_encoding() {
    // No modes -> None / X10.
    assert_eq!(
        MouseMode::from_modes(&[]),
        MouseMode {
            report: MouseReportMode::None,
            encoding: MouseEncoding::X10,
        }
    );
    // 1000 alone -> Normal / X10.
    assert_eq!(
        MouseMode::from_modes(&[TerminalMode::MouseReportClick]),
        MouseMode {
            report: MouseReportMode::Normal,
            encoding: MouseEncoding::X10,
        }
    );
    // 1002 x 1005 -> Button / Utf8.
    assert_eq!(
        MouseMode::from_modes(&[TerminalMode::MouseDrag, TerminalMode::Utf8Mouse]),
        MouseMode {
            report: MouseReportMode::Button,
            encoding: MouseEncoding::Utf8,
        }
    );
    // 1003 x 1006 (+1004/2004 ignored for mouse) -> Any / Sgr.
    assert_eq!(
        MouseMode::from_modes(&[
            TerminalMode::MouseMotion,
            TerminalMode::SgrMouse,
            TerminalMode::FocusInOut,
            TerminalMode::BracketedPaste,
        ]),
        MouseMode {
            report: MouseReportMode::Any,
            encoding: MouseEncoding::Sgr,
        }
    );
    // 1006 beats 1005.
    assert_eq!(
        MouseMode::from_modes(&[TerminalMode::SgrMouse, TerminalMode::Utf8Mouse]).encoding,
        MouseEncoding::Sgr,
    );
    // Report precedence: 1003 > 1002 > 1000.
    assert_eq!(
        MouseMode::from_modes(&[
            TerminalMode::MouseReportClick,
            TerminalMode::MouseDrag,
            TerminalMode::MouseMotion,
        ])
        .report,
        MouseReportMode::Any,
    );
}

#[test]
fn from_modes_is_backward_compat_with_default_overlay() {
    let cases: &[&[TerminalMode]] = &[
        &[],
        &[TerminalMode::MouseReportClick],
        &[TerminalMode::MouseDrag, TerminalMode::Utf8Mouse],
        &[TerminalMode::MouseMotion, TerminalMode::SgrMouse],
        &[TerminalMode::SgrMouse, TerminalMode::Utf8Mouse],
    ];
    for modes in cases {
        assert_eq!(
            MouseMode::from_modes(modes),
            MouseMode::from_modes_with(modes, MouseModeOverlay::default()),
            "from_modes must equal from_modes_with(default) for {modes:?}",
        );
    }
    // Default overlay selects nothing: SgrMouse still wins, no 1015/1016 leak.
    assert_eq!(
        MouseMode::from_modes(&[TerminalMode::SgrMouse]).encoding,
        MouseEncoding::Sgr,
    );
}

#[test]
fn overlay_makes_urxvt_and_sgr_pixels_reachable() {
    let modes = &[TerminalMode::MouseReportClick, TerminalMode::SgrMouse];
    // DEC 1015 opt-in beats SGR.
    assert_eq!(
        MouseMode::from_modes_with(
            modes,
            MouseModeOverlay {
                urxvt_mouse: true,
                ..MouseModeOverlay::default()
            },
        )
        .encoding,
        MouseEncoding::Urxvt,
    );
    // DEC 1016 opt-in beats both urxvt and SGR.
    assert_eq!(
        MouseMode::from_modes_with(
            modes,
            MouseModeOverlay {
                urxvt_mouse: true,
                sgr_pixels: true,
            },
        )
        .encoding,
        MouseEncoding::SgrPixels,
    );
    // Overlay alone (no TerminalMode bits) still selects the encoding.
    assert_eq!(
        MouseMode::from_modes_with(
            &[],
            MouseModeOverlay {
                sgr_pixels: true,
                ..MouseModeOverlay::default()
            },
        )
        .encoding,
        MouseEncoding::SgrPixels,
    );
}

#[test]
fn sgr_encode_press_bytes() {
    let mode = MouseMode {
        report: MouseReportMode::Any,
        encoding: MouseEncoding::Sgr,
    };
    let mut out = Vec::new();
    encode_mouse(&press(10, 5), &mode, &mut out);
    assert_eq!(out, b"\x1b[<0;11;6M", "SGR press col10,row5 must be ESC[<0;11;6M");
}

#[test]
fn sgr_encode_release_uses_lowercase_m() {
    let mode = MouseMode {
        report: MouseReportMode::Any,
        encoding: MouseEncoding::Sgr,
    };
    let mut out = Vec::new();
    encode_mouse(
        &MouseEvent {
            action: MouseAction::Release,
            ..press(10, 5)
        },
        &mode,
        &mut out,
    );
    assert_eq!(
        out,
        b"\x1b[<0;11;6m",
        "SGR release col10,row5 must be ESC[<0;11;6m"
    );
}

#[test]
fn urxvt_encode_press_bytes() {
    let mode = MouseMode {
        report: MouseReportMode::Any,
        encoding: MouseEncoding::Urxvt,
    };
    let mut out = Vec::new();
    encode_mouse(&press(10, 5), &mode, &mut out);
    assert_eq!(
        out,
        b"\x1b[32;11;6M",
        "Urxvt press col10,row5 code0 must be ESC[32;11;6M"
    );
}

#[test]
fn sgr_pixels_encode_uses_pixel_coords() {
    let mode = MouseMode {
        report: MouseReportMode::Any,
        encoding: MouseEncoding::SgrPixels,
    };
    let mut out = Vec::new();
    encode_mouse(
        &MouseEvent {
            x_px: 123,
            y_px: 45,
            col: 10,
            row: 5,
            ..press(10, 5)
        },
        &mode,
        &mut out,
    );
    assert_eq!(
        out, b"\x1b[<0;123;45M",
        "SGR-pixels must encode x_px/y_px, not col/row"
    );
    out.clear();
    encode_mouse(
        &MouseEvent {
            action: MouseAction::Release,
            x_px: 123,
            y_px: 45,
            col: 10,
            row: 5,
            ..press(10, 5)
        },
        &mode,
        &mut out,
    );
    assert_eq!(out, b"\x1b[<0;123;45m", "SGR-pixels release uses lowercase m");
}

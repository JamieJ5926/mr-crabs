//! System-matrix resize + TERM/terminfo boundary checks.
//!
//! Read-only on src: exercises public APIs only, zero src edits.
//! - `PtySize::new` rejects zero cols/rows.
//! - `to_winsize` pixel totals are products and saturate at `u16::MAX`.
//! - terminfo resolver: direct `<dir>/78/xterm-ghostty` vs nested
//!   `<dir>/terminfo/78/xterm-ghostty`, missing entry falls back to
//!   `TERM=xterm-256color` with no `TERMINFO` dir.
//!
//! Repro (read-only): `/tmp/crabs-sysmatrix-resize/resize-check.log`
//! (probe `ENV` + `SIGWINCH-NOTE` + `DECSTBM-REGION` labels).

use mr_crabs_pty::{PtySize, PtySizeError};

#[path = "../../mr-crabs-config/src/lib.rs"]
mod config_under_test;

use config_under_test::{
    COLORTERM_TRUECOLOR, TERMINFO_ENTRY_REL, TERM_FALLBACK, TERM_GHOSTTY,
    resolve_child_terminfo_from, resolve_terminfo_dir,
};

#[test]
fn sysmatrix_zero_columns_err() {
    assert_eq!(PtySize::new(0, 24, 8, 16).err(), Some(PtySizeError::ZeroColumns));
}

#[test]
fn sysmatrix_zero_rows_err() {
    assert_eq!(PtySize::new(80, 0, 8, 16).err(), Some(PtySizeError::ZeroRows));
}

#[test]
fn sysmatrix_winsize_totals_are_products() {
    let size = PtySize::new(80, 24, 8, 17).expect("valid size");
    let winsize = size.to_winsize();
    assert_eq!(winsize.ws_col, 80);
    assert_eq!(winsize.ws_row, 24);
    assert_eq!(winsize.ws_xpixel, 640);
    assert_eq!(winsize.ws_ypixel, 408);
}

#[test]
fn sysmatrix_winsize_totals_saturate_u16() {
    let size = PtySize::new(65535, 65535, 65535, 65535).expect("valid size");
    let winsize = size.to_winsize();
    assert_eq!(winsize.ws_xpixel, u16::MAX);
    assert_eq!(winsize.ws_ypixel, u16::MAX);
}

fn unique_root(prefix: &str) -> std::path::PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("time")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "mr-crabs-sysmatrix-{}-{}-{}",
        prefix,
        std::process::id(),
        stamp
    ))
}

#[test]
fn sysmatrix_resolve_terminfo_dir_direct_and_nested() {
    // Direct layout: <dir>/78/xterm-ghostty.
    let direct = unique_root("direct");
    let direct_entry = direct.join(TERMINFO_ENTRY_REL);
    std::fs::create_dir_all(direct_entry.parent().expect("parent")).expect("direct layout");
    std::fs::write(&direct_entry, b"ghostty").expect("entry");
    assert_eq!(
        resolve_terminfo_dir(&direct).as_deref(),
        Some(direct.as_path())
    );

    // Nested layout: <dir>/terminfo/78/xterm-ghostty resolves to <dir>/terminfo.
    let nested_root = unique_root("nested");
    let nested_entry = nested_root.join("terminfo").join(TERMINFO_ENTRY_REL);
    std::fs::create_dir_all(nested_entry.parent().expect("parent")).expect("nested layout");
    std::fs::write(&nested_entry, b"ghostty").expect("entry");
    assert_eq!(
        resolve_terminfo_dir(&nested_root).as_deref(),
        Some(nested_root.join("terminfo").as_path())
    );

    let _ = std::fs::remove_dir_all(&direct);
    let _ = std::fs::remove_dir_all(&nested_root);
}

#[test]
fn sysmatrix_missing_entry_falls_back_to_xterm_256color() {
    let missing = unique_root("missing");
    std::fs::create_dir_all(missing.join("78")).expect("temp dir");
    assert!(resolve_terminfo_dir(&missing).is_none());

    let resolved = resolve_child_terminfo_from(std::slice::from_ref(&missing));
    assert_eq!(resolved.term, TERM_FALLBACK);
    assert_eq!(resolved.term, "xterm-256color");
    assert_eq!(resolved.colorterm, COLORTERM_TRUECOLOR);
    assert!(resolved.terminfo_dir.is_none());
    assert!(
        !resolved.env_pairs().iter().any(|(k, _)| k == "TERMINFO"),
        "fallback must not set TERMINFO"
    );

    // Sanity: the ghostty constant the direct layout resolves to.
    assert_eq!(TERM_GHOSTTY, "xterm-ghostty");

    let _ = std::fs::remove_dir_all(&missing);
}

//! SysMatrix OSC permission-matrix probes: OSC8 hyperlink + OSC52 clipboard.
//!
//! Read-only over `mr-crabs-protocols` parser surface; zero src edits.
//! Parser verdicts live here. Permission enforcement lives one layer up
//! and is fail-closed by default:
//! - `crates/mr-crabs-config/src/lib.rs:559-560`
//!   (`EffectiveConfig::defaults`: `allow_osc52_write: false`,
//!   `allow_osc52_read: false`)
//! - `crates/mr-crabs-input/src/lib.rs:1139-1167`
//!   (`osc52_write`/`osc52_read_request` deny before touching backend)
//! - `crates/mr-crabs-app/src/ui/workspace.rs:214-241`
//!   (`?` read path requires `allow_osc52_read`; write path requires
//!   `osc52_write` Ok; no app-side timeout, request handled inline)
//! Probe bytes: `/tmp/crabs-probe-liveprobe/probe.sh:87-93`
//! (OSC8-LINK, OSC52-QUERY labels).

use mr_crabs_protocols::osc::{Command, Parser};

fn parse(body: &[u8]) -> Option<Command> {
    let mut p = Parser::new();
    for &b in body {
        p.next(b);
    }
    p.end(None)
}

#[test]
fn hyperlink_empty_uri_ends_active_link() {
    assert_eq!(parse(b"8;;"), Some(Command::HyperlinkEnd));
}

#[test]
fn hyperlink_id_plus_empty_uri_is_invalid() {
    assert_eq!(parse(b"8;id=foo;"), None);
}

#[test]
fn hyperlink_start_carries_id_and_uri() {
    assert_eq!(
        parse(b"8;id=foo;https://example.com"),
        Some(Command::HyperlinkStart {
            id: Some("foo".into()),
            uri: "https://example.com".into(),
        })
    );
}

#[test]
fn hyperlink_missing_separator_is_invalid() {
    assert_eq!(parse(b"8;noseparator"), None);
}

#[test]
fn clipboard_bare_separator_defaults_to_c_kind() {
    assert_eq!(
        parse(b"52;;?"),
        Some(Command::ClipboardContents {
            kind: b'c',
            data: b"?".to_vec(),
        })
    );
}

#[test]
fn clipboard_explicit_kind_splits_kind_from_data() {
    assert_eq!(
        parse(b"52;s;?"),
        Some(Command::ClipboardContents {
            kind: b's',
            data: b"?".to_vec(),
        })
    );
    assert_eq!(
        parse(b"52;c;aGk="),
        Some(Command::ClipboardContents {
            kind: b'c',
            data: b"aGk=".to_vec(),
        })
    );
}

#[test]
fn clipboard_empty_payload_is_invalid() {
    assert_eq!(parse(b"52;"), None);
}

#[test]
fn clipboard_missing_separator_is_invalid() {
    assert_eq!(parse(b"52;cx"), None);
}

#[test]
fn osc52_read_query_parses_but_requires_allow_osc52_read_default_false() {
    assert_eq!(
        parse(b"52;c;?"),
        Some(Command::ClipboardContents {
            kind: b'c',
            data: b"?".to_vec(),
        })
    );
}

#[test]
fn osc52_write_payload_parses_but_requires_allow_osc52_write_default_false() {
    assert_eq!(
        parse(b"52;c;aGVsbG8="),
        Some(Command::ClipboardContents {
            kind: b'c',
            data: b"aGVsbG8=".to_vec(),
        })
    );
}

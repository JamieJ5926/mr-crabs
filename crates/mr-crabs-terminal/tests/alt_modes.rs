//! DEC private modes 1047 / 1048 / 1049 and DECRQM reports.
//!
//! Engine still drops Unknown 1047/1048 (no swap_alt / DECSC). Protocol
//! overlay records those modes so DECRQM reports Set/Reset independently.

use std::sync::{Arc, Mutex};

use mr_crabs_protocols::sink::{ProtocolSink, RecordingSink};
use mr_crabs_terminal::{GridSize, Terminal, TerminalMode};

struct SharedSink(Arc<Mutex<RecordingSink>>);

impl ProtocolSink for SharedSink {
    fn write_pty(&mut self, bytes: &[u8]) {
        self.0.lock().expect("sink").write_pty(bytes);
    }
}

fn new_term() -> (Terminal, Arc<Mutex<RecordingSink>>) {
    let rec = Arc::new(Mutex::new(RecordingSink::new()));
    let mut term = Terminal::new(GridSize::new(10, 4)).unwrap();
    term.set_protocol_sink(Box::new(SharedSink(Arc::clone(&rec))));
    (term, rec)
}

fn last_pty(rec: &Arc<Mutex<RecordingSink>>) -> Vec<u8> {
    rec.lock()
        .expect("sink")
        .pty_writes
        .last()
        .cloned()
        .unwrap_or_default()
}

fn decrqm(term: &mut Terminal, rec: &Arc<Mutex<RecordingSink>>, mode: u16) -> Vec<u8> {
    let seq = format!("\x1b[?{mode}$p");
    term.feed(seq.as_bytes()).expect("terminal feed");
    last_pty(rec)
}

#[test]
fn mode_1047_isolate_decrqm_without_1049_alt_bit() {
    let (mut term, rec) = new_term();
    term.feed(b"primary").expect("terminal feed");
    term.feed(b"\x1b[?1047h").expect("terminal feed");
    let snap = term.snapshot();
    assert!(
        !snap.modes.contains(&TerminalMode::AltScreen),
        "1047 must not take the 1049 alt-screen engine path"
    );
    assert_eq!(decrqm(&mut term, &rec, 1047), b"\x1b[?1047;1$y");
    assert_eq!(decrqm(&mut term, &rec, 1049), b"\x1b[?1049;2$y");
    term.feed(b"\x1b[?1047l").expect("terminal feed");
    assert_eq!(decrqm(&mut term, &rec, 1047), b"\x1b[?1047;2$y");
}

#[test]
fn mode_1048_cursor_round_trip_decrqm() {
    let (mut term, rec) = new_term();
    term.feed(b"AB").expect("terminal feed");
    let before = term.snapshot().cursor;
    term.feed(b"\x1b[?1048h").expect("terminal feed");
    assert_eq!(decrqm(&mut term, &rec, 1048), b"\x1b[?1048;1$y");
    term.feed(b"CD").expect("terminal feed");
    let mid = term.snapshot().cursor;
    assert_eq!(mid.col, before.col + 2);
    term.feed(b"\x1b[?1048l").expect("terminal feed");
    assert_eq!(decrqm(&mut term, &rec, 1048), b"\x1b[?1048;2$y");
    let after = term.snapshot().cursor;
    assert_eq!(
        after, mid,
        "engine still drops Unknown 1048; restore waits on engine arm"
    );
}

#[test]
fn mode_1049_unchanged_and_decrqm() {
    let (mut term, rec) = new_term();
    term.feed(b"primary").expect("terminal feed");
    let primary = term.snapshot();
    term.feed(b"\x1b[?1049h").expect("terminal feed");
    let alt = term.snapshot();
    assert!(alt.modes.contains(&TerminalMode::AltScreen));
    assert_eq!(decrqm(&mut term, &rec, 1049), b"\x1b[?1049;1$y");
    term.feed(b"alt").expect("terminal feed");
    term.feed(b"\x1b[?1049l").expect("terminal feed");
    let after = term.snapshot();
    assert!(!after.modes.contains(&TerminalMode::AltScreen));
    assert_eq!(after.size, primary.size);
    assert_eq!(decrqm(&mut term, &rec, 1049), b"\x1b[?1049;2$y");
}

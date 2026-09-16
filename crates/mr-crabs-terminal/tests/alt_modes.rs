//! DEC private modes 1047 / 1048 / 1049 / 2026 and DECRQM reports.
//!
//! 1047 routes to the alt-screen engine path and 1048 saves/restores the
//! cursor through the protocol overlay; DECRQM reports Set/Reset from the
//! overlay flags. 2026 answers from vte's sync-window signal: Reset idle
//! and after the window closes, with the in-window query held until ESU.

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
fn mode_1047_alt_screen_path_decrqm() {
    let (mut term, rec) = new_term();
    term.feed(b"primary").expect("terminal feed");
    term.feed(b"\x1b[?1047h").expect("terminal feed");
    let snap = term.snapshot();
    assert!(
        snap.modes.contains(&TerminalMode::AltScreen),
        "1047 takes the alt-screen engine path"
    );
    assert_eq!(decrqm(&mut term, &rec, 1047), b"\x1b[?1047;1$y");
    assert_eq!(decrqm(&mut term, &rec, 1049), b"\x1b[?1049;1$y");
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
    assert_eq!(after, before, "1048l restores the cursor saved by 1048h");
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

#[test]
fn mode_2026_decrqm_reset_idle_and_after_window() {
    let (mut term, rec) = new_term();
    assert_eq!(decrqm(&mut term, &rec, 2026), b"\x1b[?2026;2$y");
    let writes = rec.lock().expect("sink").pty_writes.len();
    term.feed(b"\x1b[?2026h").expect("terminal feed");
    term.feed(b"\x1b[?2026$p").expect("terminal feed");
    assert_eq!(
        rec.lock().expect("sink").pty_writes.len(),
        writes,
        "vte holds the in-window query until ESU"
    );
    term.feed(b"\x1b[?2026l").expect("terminal feed");
    assert_eq!(last_pty(&rec), b"\x1b[?2026;2$y");
    assert_eq!(decrqm(&mut term, &rec, 2026), b"\x1b[?2026;2$y");
}

fn decrqss_sgr(term: &mut Terminal, rec: &Arc<Mutex<RecordingSink>>) -> Vec<u8> {
    term.feed(b"\x1bP$q").expect("terminal feed");
    term.feed(b"m\x1b\\").expect("terminal feed");
    last_pty(rec)
}

#[test]
fn sgr58_rgb_reaches_decrqss() {
    let (mut term, rec) = new_term();
    term.feed(b"\x1b[58:2::255:0:128m").expect("terminal feed");
    assert_eq!(decrqss_sgr(&mut term, &rec), b"\x1bP1$r0;58:2::255:0:128m\x1b\\");
}

#[test]
fn sgr58_indexed_reaches_decrqss_and_59_clears() {
    let (mut term, rec) = new_term();
    term.feed(b"\x1b[58:5:196m").expect("terminal feed");
    assert_eq!(decrqss_sgr(&mut term, &rec), b"\x1bP1$r0;58:5:196m\x1b\\");
    term.feed(b"\x1b[59m").expect("terminal feed");
    assert_eq!(decrqss_sgr(&mut term, &rec), b"\x1bP1$r0m\x1b\\");
}

#[test]
fn mixed_plain_escape_window_holds_until_esu() {
    use mr_crabs_terminal::GridSize as GS;
    let size = GS::new(10, 4);
    let mut term = Terminal::new(size).unwrap();
    term.feed(b"\x1b[?2026hhello\x1b[31mworld").expect("terminal feed");
    let snap = term.snapshot();
    let row0: String = snap.cells[..10]
        .iter()
        .map(|c| char::from_u32(c.content).unwrap_or(' '))
        .collect();
    assert_eq!(row0, "          ", "mid-window bytes must not paint");
    term.feed(b"\x1b[?2026l").expect("terminal feed");
    let snap = term.snapshot();
    let row0: String = snap.cells[..10]
        .iter()
        .map(|c| char::from_u32(c.content).unwrap_or(' '))
        .collect();
    assert!(row0.starts_with("helloworld"), "ESU replays the held bytes");
}

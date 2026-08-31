use std::fs::OpenOptions;
use std::io::{self, Read, Write};
use std::os::fd::AsRawFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crate::animation_control::{
    AnimationControl, AnimationReply, AnimationSelection, AnimationSnapshot, ReplyScanner,
    SCANNER_MAX_KEY_INPUT, TextAnimationChoice, selection_control, snapshot_control,
};

const HOST_TIMEOUT: Duration = Duration::from_secs(2);
const ESC_GRACE_MS: i32 = 25;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MenuItem {
    TextNone,
    TextStreaming,
    TextTypewriter,
    CursorTrail,
}

impl MenuItem {
    pub const ALL: [Self; 4] = [
        Self::TextNone,
        Self::TextStreaming,
        Self::TextTypewriter,
        Self::CursorTrail,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::TextNone => "Text animation: none",
            Self::TextStreaming => "Text animation: streaming",
            Self::TextTypewriter => "Text animation: typewriter",
            Self::CursorTrail => "Cursor trail",
        }
    }

    pub const fn index(self) -> usize {
        match self {
            Self::TextNone => 0,
            Self::TextStreaming => 1,
            Self::TextTypewriter => 2,
            Self::CursorTrail => 3,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Key {
    Up,
    Down,
    Left,
    Right,
    Space,
    Enter,
    Replay,
    PlayAll,
    Save,
    Quit,
    Escape,
    CtrlC,
    Unknown(u8),
}

/// Decode one complete key from the front of `bytes`.
///
/// Incomplete CSI/SS3 sequences return `(None, 0)` so callers can retain their
/// bytes. A lone ESC is intentionally a complete Escape key.
pub fn decode_key(bytes: &[u8]) -> (Option<Key>, usize) {
    let Some(&first) = bytes.first() else {
        return (None, 0);
    };
    match first {
        0x03 => (Some(Key::CtrlC), 1),
        b' ' => (Some(Key::Space), 1),
        b'\r' | b'\n' => (Some(Key::Enter), 1),
        b'r' | b'R' => (Some(Key::Replay), 1),
        b'a' | b'A' => (Some(Key::PlayAll), 1),
        b's' | b'S' => (Some(Key::Unknown(first)), 1),
        b'q' | b'Q' => (Some(Key::Quit), 1),
        0x1b => {
            if bytes.len() == 1 {
                return (Some(Key::Escape), 1);
            }
            if bytes[1] != b'[' && bytes[1] != b'O' {
                return (Some(Key::Escape), 1);
            }
            if bytes.len() < 3 {
                return (None, 0);
            }
            let key = match bytes[2] {
                b'A' => Key::Up,
                b'B' => Key::Down,
                b'C' => Key::Right,
                b'D' => Key::Left,
                _ => return (Some(Key::Unknown(bytes[2])), 3),
            };
            (Some(key), 3)
        }
        other => (Some(Key::Unknown(other)), 1),
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DemoStep {
    Write(Vec<u8>),
    Sleep(Duration),
}

pub fn demo_script(selection: AnimationSelection) -> Vec<DemoStep> {
    let text = selection.text.as_str();
    let trail = if selection.cursor_trail { "on" } else { "off" };
    vec![
        DemoStep::Write(
            format!("\r\x1b[2KPreview — text={text}, cursor-trail={trail}\r\n").into_bytes(),
        ),
        DemoStep::Sleep(Duration::from_millis(70)),
        DemoStep::Write(b"\r\x1b[2KAnimation demo complete\r\n".to_vec()),
        DemoStep::Sleep(Duration::from_millis(70)),
    ]
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TuiAction {
    Redraw,
    Apply(AnimationSelection),
    Demo(DemoStep),
    Restore(AnimationSnapshot),
    Save(AnimationSelection),
    Quit(AnimationSnapshot),
}

pub fn replay_sequence(selection: AnimationSelection, entry: AnimationSnapshot) -> Vec<TuiAction> {
    let mut actions = vec![TuiAction::Apply(selection)];
    actions.extend(demo_script(selection).into_iter().map(TuiAction::Demo));
    actions.push(TuiAction::Restore(entry));
    actions
}

pub fn play_all_sequence(entry: AnimationSnapshot) -> Vec<TuiAction> {
    let selections = [
        AnimationSelection {
            text: TextAnimationChoice::Streaming,
            cursor_trail: entry.selection.cursor_trail,
        },
        AnimationSelection {
            text: TextAnimationChoice::Typewriter,
            cursor_trail: entry.selection.cursor_trail,
        },
        AnimationSelection {
            text: entry.selection.text,
            cursor_trail: true,
        },
    ];
    let mut actions = Vec::new();
    for selection in selections {
        actions.extend(replay_sequence(selection, entry));
    }
    actions
}

fn highlighted_selection(item: MenuItem, current: AnimationSelection) -> AnimationSelection {
    match item {
        MenuItem::TextNone => AnimationSelection {
            text: TextAnimationChoice::None,
            cursor_trail: current.cursor_trail,
        },
        MenuItem::TextStreaming => AnimationSelection {
            text: TextAnimationChoice::Streaming,
            cursor_trail: current.cursor_trail,
        },
        MenuItem::TextTypewriter => AnimationSelection {
            text: TextAnimationChoice::Typewriter,
            cursor_trail: current.cursor_trail,
        },
        MenuItem::CursorTrail => AnimationSelection {
            text: current.text,
            cursor_trail: true,
        },
    }
}

#[derive(Clone, Debug)]
pub struct AnimationTui {
    pub selection: AnimationSelection,
    pub entry: AnimationSnapshot,
    pub cursor: usize,
    pub status: String,
    pub menu: bool,
}

impl AnimationTui {
    pub fn new(entry: AnimationSnapshot) -> Self {
        Self::new_with_menu(entry, false)
    }

    pub fn new_with_menu(entry: AnimationSnapshot, menu: bool) -> Self {
        let cursor = if menu {
            0
        } else {
            match entry.selection.text {
                TextAnimationChoice::None => MenuItem::TextNone.index(),
                TextAnimationChoice::Streaming => MenuItem::TextStreaming.index(),
                TextAnimationChoice::Typewriter => MenuItem::TextTypewriter.index(),
            }
        };
        Self {
            selection: entry.selection,
            entry,
            cursor,
            status: String::new(),
            menu,
        }
    }

    pub fn highlighted(&self) -> MenuItem {
        MenuItem::ALL[self.cursor]
    }

    pub fn move_cursor(&mut self, delta: isize) {
        let len = MenuItem::ALL.len() as isize;
        self.cursor = (self.cursor as isize + delta).rem_euclid(len) as usize;
    }

    pub fn toggle(&mut self) {
        match self.highlighted() {
            MenuItem::TextNone => self.selection.text = TextAnimationChoice::None,
            MenuItem::TextStreaming => self.selection.text = TextAnimationChoice::Streaming,
            MenuItem::TextTypewriter => self.selection.text = TextAnimationChoice::Typewriter,
            MenuItem::CursorTrail => self.selection.cursor_trail = !self.selection.cursor_trail,
        }
    }

    pub fn dispatch(&mut self, key: Key) -> Vec<TuiAction> {
        match key {
            Key::Up | Key::Left => {
                self.move_cursor(-1);
                vec![TuiAction::Redraw]
            }
            Key::Down | Key::Right => {
                self.move_cursor(1);
                vec![TuiAction::Redraw]
            }
            Key::Space => {
                self.toggle();
                vec![TuiAction::Redraw]
            }
            Key::Replay => replay_sequence(
                highlighted_selection(self.highlighted(), self.selection),
                self.entry,
            ),
            Key::PlayAll => play_all_sequence(self.entry),
            Key::Enter | Key::Save => {
                self.status = "Save unavailable until a trusted host channel exists".into();
                vec![TuiAction::Redraw]
            }
            Key::Quit | Key::Escape | Key::CtrlC => vec![TuiAction::Quit(self.entry)],
            Key::Unknown(_) => vec![],
        }
    }
}

pub fn render(state: &AnimationTui) -> Vec<u8> {
    let mut out = String::from("\x1b[2J\x1b[H\x1b[?25l");
    if state.menu {
        out.push_str("Mr Crabs animation menu\r\n\r\n");
    } else {
        out.push_str("Mr Crabs animation\r\n\r\n");
    }
    for (index, item) in MenuItem::ALL.iter().copied().enumerate() {
        let checked = match item {
            MenuItem::TextNone => state.selection.text == TextAnimationChoice::None,
            MenuItem::TextStreaming => state.selection.text == TextAnimationChoice::Streaming,
            MenuItem::TextTypewriter => state.selection.text == TextAnimationChoice::Typewriter,
            MenuItem::CursorTrail => state.selection.cursor_trail,
        };
        let marker = if checked { 'x' } else { ' ' };
        let pointer = if index == state.cursor { '>' } else { ' ' };
        out.push_str(&format!("{pointer} [{marker}] {}\r\n", item.label()));
    }
    out.push_str("\r\n  space select   r replay   a play all   q quit\r\n");
    if !state.status.is_empty() {
        out.push_str(&format!("\r\n{}\r\n", state.status));
    }
    out.into_bytes()
}

struct Cleanup {
    fd: i32,
    original: libc::termios,
    io: Mutex<std::fs::File>,
    restored: AtomicBool,
}

impl Cleanup {
    fn new(file: std::fs::File, original: libc::termios) -> Self {
        Self {
            fd: file.as_raw_fd(),
            original,
            io: Mutex::new(file),
            restored: AtomicBool::new(false),
        }
    }

    fn restore(&self) {
        if self.restored.swap(true, Ordering::AcqRel) {
            return;
        }
        if let Ok(mut file) = self.io.lock() {
            let _ = file.write_all(b"\x1b[?25h\x1b[?1049l");
            let _ = file.flush();
        }
        // SAFETY: fd and original were obtained from this still-live File.
        unsafe {
            let _ = libc::tcsetattr(self.fd, libc::TCSANOW, &self.original);
        }
    }
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        self.restore();
    }
}

type PanicHook = Box<dyn Fn(&std::panic::PanicHookInfo<'_>) + Send + Sync + 'static>;

struct HookState {
    previous: Mutex<Option<PanicHook>>,
}

/// Raw `/dev/tty` session. Drop and the installed panic hook both restore the
/// terminal, and the atomic guard makes either path idempotent.
pub struct RawTty {
    cleanup: Arc<Cleanup>,
    hook_state: Arc<HookState>,
}

impl RawTty {
    pub fn open() -> io::Result<Self> {
        let file = OpenOptions::new().read(true).write(true).open("/dev/tty")?;
        let fd = file.as_raw_fd();
        let mut original = std::mem::MaybeUninit::<libc::termios>::uninit();
        // SAFETY: original points to writable termios storage and fd is valid.
        if unsafe { libc::tcgetattr(fd, original.as_mut_ptr()) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let original = unsafe { original.assume_init() };
        let mut raw = original;
        // SAFETY: raw is initialized and owned by this function.
        unsafe {
            libc::cfmakeraw(&mut raw);
        }
        // SAFETY: fd remains valid while file is held by Cleanup.
        if unsafe { libc::tcsetattr(fd, libc::TCSANOW, &raw) } != 0 {
            return Err(io::Error::last_os_error());
        }
        let cleanup = Arc::new(Cleanup::new(file, original));
        {
            let mut file = cleanup.io.lock().expect("new tty mutex");
            file.write_all(b"\x1b[?1049h\x1b[?25l")?;
            file.flush()?;
        }
        let hook_state = Arc::new(HookState {
            previous: Mutex::new(Some(std::panic::take_hook())),
        });
        let hook_cleanup = Arc::clone(&cleanup);
        let hook_state_for_panic = Arc::clone(&hook_state);
        std::panic::set_hook(Box::new(move |info| {
            hook_cleanup.restore();
            if let Some(previous) = hook_state_for_panic
                .previous
                .lock()
                .expect("panic hook mutex")
                .take()
            {
                previous(info);
            }
        }));
        Ok(Self {
            cleanup,
            hook_state,
        })
    }

    pub fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        let mut file = self.cleanup.io.lock().expect("tty mutex");
        file.write_all(bytes)
    }

    pub fn flush(&mut self) -> io::Result<()> {
        self.cleanup.io.lock().expect("tty mutex").flush()
    }

    fn read_some(&mut self, timeout_ms: i32) -> io::Result<Vec<u8>> {
        let mut file = self.cleanup.io.lock().expect("tty mutex");
        let fd = file.as_raw_fd();
        let mut pollfd = libc::pollfd {
            fd,
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: pollfd is valid for one element.
        let ready = unsafe { libc::poll(&mut pollfd, 1, timeout_ms) };
        if ready == 0 {
            return Ok(Vec::new());
        }
        if ready < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut buf = [0u8; 256];
        match file.read(&mut buf) {
            Ok(0) => Err(io::Error::new(io::ErrorKind::UnexpectedEof, "tty closed")),
            Ok(n) => Ok(buf[..n].to_vec()),
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => Ok(Vec::new()),
            Err(error) => Err(error),
        }
    }
}

impl Drop for RawTty {
    fn drop(&mut self) {
        self.cleanup.restore();
        if let Some(previous) = self
            .hook_state
            .previous
            .lock()
            .expect("panic hook mutex")
            .take()
        {
            let _ = std::panic::take_hook();
            std::panic::set_hook(previous);
        }
    }
}

struct HostAdapter {
    tty: RawTty,
    scanner: ReplyScanner,
    pending_keys: Vec<u8>,
}

impl HostAdapter {
    fn open() -> io::Result<Self> {
        Ok(Self {
            tty: RawTty::open()?,
            scanner: ReplyScanner::new(),
            pending_keys: Vec::new(),
        })
    }

    fn write_all(&mut self, bytes: &[u8]) -> io::Result<()> {
        self.tty.write_all(bytes)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.tty.flush()
    }

    fn send(&mut self, control: &AnimationControl) -> io::Result<()> {
        self.tty.write_all(&control.encode())?;
        self.tty.flush()
    }

    fn ingest(&mut self, timeout_ms: i32) -> io::Result<()> {
        let bytes = self.tty.read_some(timeout_ms)?;
        if !bytes.is_empty() {
            self.scanner.push(&bytes);
            if self.scanner.overflowed() {
                return Err(io::Error::new(
                    io::ErrorKind::InvalidData,
                    "animation host reply exceeded scanner limits",
                ));
            }
            append_pending_keys(&mut self.pending_keys, self.scanner.drain_key_input())?;
        }
        Ok(())
    }

    fn wait_reply(&mut self, timeout: Duration) -> io::Result<AnimationReply> {
        let deadline = Instant::now() + timeout;
        loop {
            let replies = self.scanner.drain_replies();
            if let Some(reply) = replies.into_iter().next() {
                return Ok(reply);
            }
            let now = Instant::now();
            if now >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "animation host reply timed out; terminal restored",
                ));
            }
            let remaining = deadline.saturating_duration_since(now);
            let timeout_ms = remaining.as_millis().min(i32::MAX as u128) as i32;
            self.ingest(timeout_ms)?;
        }
    }

    fn query_snapshot(&mut self) -> io::Result<AnimationSnapshot> {
        self.send(&AnimationControl::Query {
            terminator: crate::animation_control::OscTerminator::Bell,
        })?;
        match self.wait_reply(HOST_TIMEOUT)? {
            AnimationReply::Snapshot(snapshot) => Ok(snapshot),
            AnimationReply::Saved(_) => Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "animation host query returned a save reply",
            )),
        }
    }

    fn read_key(&mut self) -> io::Result<Key> {
        loop {
            match decode_key(&self.pending_keys) {
                (Some(key), n) => {
                    self.pending_keys.drain(..n);
                    let _ = self.scanner.drain_replies();
                    return Ok(key);
                }
                (None, 0) if self.pending_keys.is_empty() => {
                    self.ingest(-1)?;
                }
                (None, 0) => {
                    self.ingest(ESC_GRACE_MS)?;
                    if decode_key(&self.pending_keys) == (None, 0) {
                        if self.pending_keys.first() == Some(&0x1b) {
                            self.pending_keys.remove(0);
                            let _ = self.scanner.drain_replies();
                            return Ok(Key::Escape);
                        }
                        self.ingest(-1)?;
                    }
                }
                _ => unreachable!("decode_key consumed count is 0 or a complete key"),
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AnimationExit {
    Quit,
    Saved,
}

/// Run the animation TUI on `/dev/tty`. `menu` only changes the initial
/// highlight and title; Enter/s do not save over PTY.
pub fn run_animation_tui(menu: bool) -> io::Result<AnimationExit> {
    let mut host = HostAdapter::open()?;
    let entry = match host.query_snapshot() {
        Ok(entry) => entry,
        Err(error) => {
            return Err(io::Error::new(
                error.kind(),
                format!("animation host query failed: {error}"),
            ));
        }
    };
    let mut state = AnimationTui::new_with_menu(entry, menu);
    loop {
        host.write_all(&render(&state))?;
        host.flush()?;
        for action in state.dispatch(host.read_key()?) {
            match action {
                TuiAction::Redraw => {}
                TuiAction::Apply(selection) => host.send(&selection_control(selection))?,
                TuiAction::Demo(DemoStep::Write(bytes)) => host.write_all(&bytes)?,
                TuiAction::Demo(DemoStep::Sleep(duration)) => thread::sleep(duration),
                TuiAction::Restore(snapshot) => host.send(&snapshot_control(snapshot))?,
                TuiAction::Save(_) => {}
                TuiAction::Quit(snapshot) => {
                    host.send(&snapshot_control(snapshot))?;
                    return Ok(AnimationExit::Quit);
                }
            }
        }
    }
}

fn append_pending_keys(pending: &mut Vec<u8>, keys: Vec<u8>) -> io::Result<()> {
    if keys.len() > SCANNER_MAX_KEY_INPUT.saturating_sub(pending.len()) {
        pending.clear();
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "animation host pending keys exceeded scanner limits",
        ));
    }
    pending.extend_from_slice(&keys);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::animation_control::OverlaySource;

    fn entry() -> AnimationSnapshot {
        AnimationSnapshot {
            selection: AnimationSelection {
                text: TextAnimationChoice::Streaming,
                cursor_trail: false,
            },
            text_source: OverlaySource::Override,
            trail_source: OverlaySource::Global,
            save_available: false,
        }
    }

    #[test]
    fn text_choices_are_mutually_exclusive() {
        let mut tui = AnimationTui::new(entry());
        tui.cursor = MenuItem::TextTypewriter.index();
        tui.toggle();
        assert_eq!(tui.selection.text, TextAnimationChoice::Typewriter);
        assert!(!tui.selection.cursor_trail);
        tui.cursor = MenuItem::TextNone.index();
        tui.toggle();
        assert_eq!(tui.selection.text, TextAnimationChoice::None);
    }

    #[test]
    fn trail_is_independent_of_text() {
        let mut tui = AnimationTui::new(entry());
        tui.cursor = MenuItem::CursorTrail.index();
        tui.toggle();
        assert_eq!(tui.selection.text, TextAnimationChoice::Streaming);
        assert!(tui.selection.cursor_trail);
    }

    #[test]
    fn navigation_wraps() {
        let mut tui = AnimationTui::new(entry());
        tui.move_cursor(-1);
        assert_eq!(tui.highlighted(), MenuItem::TextNone);
        tui.cursor = MenuItem::TextNone.index();
        tui.move_cursor(-1);
        assert_eq!(tui.highlighted(), MenuItem::CursorTrail);
        tui.move_cursor(1);
        assert_eq!(tui.highlighted(), MenuItem::TextNone);
    }

    #[test]
    fn menu_flag_only_changes_initial_highlight_and_title() {
        let menu = AnimationTui::new_with_menu(entry(), true);
        assert_eq!(menu.highlighted(), MenuItem::TextNone);
        let rendered = String::from_utf8(render(&menu)).expect("utf8");
        assert!(rendered.contains("Mr Crabs animation menu"));

        let bare = AnimationTui::new_with_menu(entry(), false);
        assert_eq!(bare.highlighted(), MenuItem::TextStreaming);
        let rendered = String::from_utf8(render(&bare)).expect("utf8");
        assert!(rendered.contains("Mr Crabs animation"));
        assert!(!rendered.contains("animation menu"));
    }

    #[test]
    fn decodes_csi_ss3_and_lone_escape() {
        assert_eq!(decode_key(b"\x1b[A"), (Some(Key::Up), 3));
        assert_eq!(decode_key(b"\x1bOB"), (Some(Key::Down), 3));
        assert_eq!(decode_key(b"\x1b"), (Some(Key::Escape), 1));
        assert_eq!(decode_key(b"\x1b["), (None, 0));
        assert_eq!(decode_key(b"\x1bq"), (Some(Key::Escape), 1));
    }

    #[test]
    fn decodes_controls() {
        assert_eq!(decode_key(b" ").0, Some(Key::Space));
        assert_eq!(decode_key(b"r").0, Some(Key::Replay));
        assert_eq!(decode_key(b"a").0, Some(Key::PlayAll));
        assert_eq!(decode_key(b"\n").0, Some(Key::Enter));
        assert_eq!(decode_key(b"s").0, Some(Key::Unknown(b's')));
        assert_eq!(decode_key(b"q").0, Some(Key::Quit));
        assert_eq!(decode_key(b"\x03").0, Some(Key::CtrlC));
    }

    #[test]
    fn save_and_quit_actions_are_distinct() {
        let mut tui = AnimationTui::new(entry());
        assert_eq!(tui.dispatch(Key::Save), vec![TuiAction::Redraw]);
        assert_eq!(tui.dispatch(Key::Enter), vec![TuiAction::Redraw]);
        assert_eq!(tui.dispatch(Key::Quit), vec![TuiAction::Quit(tui.entry)]);
        assert!(tui.status.contains("unavailable"));
    }

    #[test]
    fn replay_uses_highlighted_item() {
        let mut tui = AnimationTui::new(entry());
        tui.cursor = MenuItem::TextTypewriter.index();
        let actions = tui.dispatch(Key::Replay);
        assert_eq!(
            actions.first(),
            Some(&TuiAction::Apply(AnimationSelection {
                text: TextAnimationChoice::Typewriter,
                cursor_trail: false,
            }))
        );
        assert_eq!(actions.last(), Some(&TuiAction::Restore(entry())));
    }

    #[test]
    fn unavailable_save_is_rendered_as_notice() {
        let mut tui = AnimationTui::new(entry());
        assert!(!tui.entry.save_available);
        assert_eq!(tui.dispatch(Key::Save), vec![TuiAction::Redraw]);
        assert_eq!(tui.dispatch(Key::Enter), vec![TuiAction::Redraw]);
        assert!(tui.status.contains("unavailable"));
    }

    #[test]
    fn render_contains_checkboxes_and_help() {
        let bytes = render(&AnimationTui::new(entry()));
        let rendered = String::from_utf8(bytes).expect("utf8 render");
        assert!(rendered.contains("Mr Crabs animation"));
        assert!(rendered.contains("[ ] Text animation: none"));
        assert!(rendered.contains("[x] Text animation: streaming"));
        assert!(rendered.contains("space select"));
        assert!(rendered.contains("r replay"));
        assert!(!rendered.contains("enter/s save"));
        assert!(rendered.contains("q quit"));
    }

    #[test]
    fn replay_applies_demo_and_restores() {
        let actions = replay_sequence(AnimationSelection::none(), entry());
        assert_eq!(
            actions.first(),
            Some(&TuiAction::Apply(AnimationSelection::none()))
        );
        assert!(
            actions
                .iter()
                .any(|action| matches!(action, TuiAction::Demo(_)))
        );
        assert_eq!(actions.last(), Some(&TuiAction::Restore(entry())));
    }

    #[test]
    fn play_all_has_three_apply_restore_pairs() {
        let actions = play_all_sequence(entry());
        assert_eq!(
            actions
                .iter()
                .filter(|a| matches!(a, TuiAction::Apply(_)))
                .count(),
            3
        );
        assert_eq!(
            actions
                .iter()
                .filter(|a| matches!(a, TuiAction::Restore(_)))
                .count(),
            3
        );
    }

    #[test]
    fn snapshot_restore_uses_inherit_for_global_keys() {
        let control = snapshot_control(entry());
        match control {
            AnimationControl::State { text, trail } => {
                assert_eq!(text.as_str(), "streaming");
                assert_eq!(trail.as_str(), "inherit");
            }
            other => panic!("expected state control, got {other:?}"),
        }
    }

    fn dummy_termios() -> libc::termios {
        // SAFETY: zeros are only passed to ignored tcsetattr on a regular file.
        unsafe { std::mem::zeroed() }
    }

    fn temp_tty_path(label: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "mr-crabs-tty-{label}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("clock")
                .as_nanos()
        ))
    }

    fn open_temp_tty(path: &std::path::Path) -> std::fs::File {
        std::fs::write(path, b"").expect("temp tty");
        OpenOptions::new()
            .read(true)
            .write(true)
            .open(path)
            .expect("open temp tty")
    }

    #[test]
    fn cleanup_drop_restores_after_setup_failure() {
        let path = temp_tty_path("setup-fail");
        let file = open_temp_tty(&path);
        let result: io::Result<()> = (|| {
            let _cleanup = super::Cleanup::new(file, dummy_termios());
            Err(io::Error::new(
                io::ErrorKind::Other,
                "alt-screen setup failed",
            ))
        })();
        assert!(result.is_err());
        assert_eq!(
            std::fs::read(&path).expect("read restored tty"),
            b"\x1b[?25h\x1b[?1049l"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn cleanup_restore_is_idempotent() {
        let path = temp_tty_path("idempotent");
        let file = open_temp_tty(&path);
        let cleanup = super::Cleanup::new(file, dummy_termios());
        cleanup.restore();
        cleanup.restore();
        drop(cleanup);
        assert_eq!(
            std::fs::read(&path).expect("read restored tty"),
            b"\x1b[?25h\x1b[?1049l"
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn pending_keys_reject_aggregate_over_cap() {
        let mut pending = vec![b'a'; SCANNER_MAX_KEY_INPUT - 4];
        append_pending_keys(&mut pending, vec![b'b'; 4]).expect("fits");
        assert_eq!(pending.len(), SCANNER_MAX_KEY_INPUT);
        let err = append_pending_keys(&mut pending, vec![b'c']).expect_err("overflow");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(pending.is_empty());
    }

    #[test]
    fn pending_keys_stay_capped_across_ingest_cycles() {
        let mut pending = Vec::new();
        for _ in 0..8 {
            append_pending_keys(&mut pending, vec![b'k'; 512]).expect("cycle");
        }
        assert_eq!(pending.len(), SCANNER_MAX_KEY_INPUT);
        let err = append_pending_keys(&mut pending, vec![b'z']).expect_err("closed");
        assert_eq!(err.kind(), io::ErrorKind::InvalidData);
        assert!(pending.is_empty());
    }
}

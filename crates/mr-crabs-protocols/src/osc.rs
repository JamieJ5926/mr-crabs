//! OSC (Operating System Command) parsing, ported from Ghostty
//! `src/terminal/osc.zig`.
//!
//! The parser is a bounded incremental state machine. It accepts one byte at
//! a time ([`Parser::next`]) and produces a typed, owned [`Command`] when the
//! sequence ends ([`Parser::end`]). The state machine mirrors the Ghostty
//! prefix states exactly, so every OSC prefix Ghostty recognizes is
//! recognized here, unknown prefixes transition to `invalid`, and oversized
//! captures transition to `invalid` without growing further.
//!
//! Two capture modes exist, matching Ghostty:
//!
//! * fixed: a stack buffer of [`Parser::MAX_BUF`] bytes (2048);
//! * allocating: a caller-supplied heap buffer bounded by
//!   [`Parser::max_allocating_bytes`] (default [`Parser::MAX_ALLOCATING_BUF`],
//!   8 MiB), used only by commands that legitimately carry large payloads
//!   (OSC 52 clipboard, 66 text sizing, 72 drag-and-drop, 5522 kitty
//!   clipboard).
//!
//! In both modes, exceeding the bound marks the sequence invalid and discards
//! the remainder with no further allocation. One byte is always reserved for
//! the implicit NUL terminator Ghostty writes before parsing payloads, so a
//! fixed capture holds at most `MAX_BUF - 1` payload bytes.

use crate::Terminator;
use crate::color::{self, ColorOperation, ColorRequest};
use crate::semantic_prompt::SemanticPrompt;

pub mod parsers;

/// Maximum size of a "normal" OSC (Ghostty `Parser.MAX_BUF`).
pub const MAX_BUF: usize = 2048;

/// Maximum size of an allocating OSC capture (Ghostty `MAX_ALLOCATING_BUF`).
pub const MAX_ALLOCATING_BUF: usize = 8 * 1024 * 1024;

/// A parsed OSC command. Every variant is owned and bounded.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Command {
    /// OSC 0/2: change the window title.
    ChangeWindowTitle(String),
    /// OSC 1: change the window icon (parsed, ignored by Ghostty).
    ChangeWindowIcon(String),
    /// OSC 133: semantic prompt.
    SemanticPrompt(SemanticPrompt),
    /// OSC 52: clipboard contents.
    ClipboardContents { kind: u8, data: Vec<u8> },
    /// OSC 7: report the current working directory (file URL).
    ReportPwd(String),
    /// OSC 22: set the mouse shape.
    MouseShape(String),
    /// OSC 4/5/10-19/104/105/110-119: color operations.
    ColorOperation {
        op: ColorOperation,
        requests: Vec<ColorRequest>,
        terminator: Terminator,
    },
    /// OSC 21: kitty color protocol requests (set/reset/query items).
    KittyColor {
        requests: Vec<color::KittyColorRequest>,
    },
    /// OSC 9 (iTerm2 form) or OSC 777 (`notify` extension): desktop
    /// notification.
    ShowDesktopNotification { title: String, body: String },
    /// OSC 8: start a hyperlink.
    HyperlinkStart { id: Option<String>, uri: String },
    /// OSC 8: end the active hyperlink.
    HyperlinkEnd,
    /// OSC 9;1: ConEmu sleep.
    ConemuSleep { duration_ms: u16 },
    /// OSC 9;2: ConEmu GUI message box.
    ConemuShowMessageBox(String),
    /// OSC 9;3: ConEmu change tab title.
    ConemuChangeTabTitle(Option<String>),
    /// OSC 9;4: ConEmu progress report.
    ConemuProgressReport {
        state: ProgressState,
        progress: Option<u8>,
    },
    /// OSC 9;5: ConEmu wait for input.
    ConemuWaitInput,
    /// OSC 9;6: ConEmu GUI macro.
    ConemuGuimacro(String),
    /// OSC 9;7: ConEmu run process.
    ConemuRunProcess(String),
    /// OSC 9;8: ConEmu output environment variable.
    ConemuOutputEnvironmentVariable(String),
    /// OSC 9;10: ConEmu xterm keyboard/output emulation.
    ConemuXtermEmulation {
        keyboard: Option<bool>,
        output: Option<bool>,
    },
    /// OSC 9;11: ConEmu comment.
    ConemuComment(String),
    /// OSC 66: kitty text sizing.
    KittyTextSizing(parsers::KittyTextSizing),
    /// OSC 72: kitty drag-and-drop protocol.
    KittyDnd(parsers::KittyDnd),
    /// OSC 5522: kitty clipboard protocol.
    KittyClipboard(parsers::KittyClipboard),
    /// OSC 1337: iTerm2 extension.
    Iterm2(parsers::Iterm2),
    /// OSC 3008: hierarchical context signal.
    ContextSignal(parsers::ContextSignal),
    /// OSC 9;12: ConEmu mark prompt start.
    MarkPromptStart,
}

/// ConEmu progress report states (Ghostty `ProgressReport.State`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProgressState {
    Remove,
    Set,
    Error,
    Indeterminate,
    Pause,
}

/// Internal parser state; mirrors Ghostty `Parser.State` prefixes.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum State {
    Start,
    Invalid,
    N0,
    N1,
    N2,
    N3,
    N4,
    N5,
    N6,
    N7,
    N8,
    N9,
    N30,
    N300,
    N3008,
    N10,
    N11,
    N12,
    N13,
    N14,
    N15,
    N16,
    N17,
    N18,
    N19,
    N21,
    N22,
    N52,
    N55,
    N66,
    N72,
    N77,
    N104,
    N110,
    N111,
    N112,
    N113,
    N114,
    N115,
    N116,
    N117,
    N118,
    N119,
    N133,
    N552,
    N777,
    N1337,
    N5522,
}

/// The bounded OSC payload capture.
enum Capture {
    Fixed {
        /// Bytes accumulated (excluding the reserved terminator slot).
        len: usize,
    },
    Allocating {
        data: Vec<u8>,
        max_bytes: usize,
    },
}

impl Capture {
    fn write_byte(&mut self, byte: u8, fixed: &mut [u8; MAX_BUF]) -> Result<(), Overflow> {
        match self {
            Self::Fixed { len } => {
                // Reserve one byte for the NUL terminator Ghostty writes
                // before payload parsing.
                if *len >= MAX_BUF - 1 {
                    return Err(Overflow);
                }
                fixed[*len] = byte;
                *len += 1;
                Ok(())
            }
            Self::Allocating { data, max_bytes } => {
                if data.len() >= *max_bytes {
                    return Err(Overflow);
                }
                data.push(byte);
                Ok(())
            }
        }
    }
}

struct Overflow;

/// The bounded incremental OSC parser.
pub struct Parser {
    /// Maximum bytes retained by an allocating capture (configurable so
    /// tests and embedders can choose a smaller policy).
    pub max_allocating_bytes: usize,
    state: State,
    buffer: Box<[u8; MAX_BUF]>,
    capture: Option<Capture>,
}

impl Parser {
    pub fn new() -> Self {
        Self {
            max_allocating_bytes: MAX_ALLOCATING_BUF,
            state: State::Start,
            buffer: Box::new([0; MAX_BUF]),
            capture: None,
        }
    }

    /// Reset the parser to its initial state, dropping any capture.
    pub fn reset(&mut self) {
        self.state = State::Start;
        self.capture = None;
    }

    pub fn state(&self) -> State {
        self.state
    }

    /// Consume one byte and advance the parser state.
    ///
    /// Once invalid, all further input is discarded (Ghostty `next`).
    pub fn next(&mut self, c: u8) {
        if self.state == State::Invalid {
            return;
        }

        // Active capture: accumulate and skip the state machine.
        if let Some(cap) = &mut self.capture {
            if cap.write_byte(c, &mut self.buffer).is_err() {
                self.state = State::Invalid;
            }
            return;
        }

        use State::*;
        self.state = match self.state {
            Start => match c {
                b'0' => N0,
                b'1' => N1,
                b'2' => N2,
                b'3' => N3,
                b'4' => N4,
                b'5' => N5,
                b'6' => N6,
                b'7' => N7,
                b'8' => N8,
                b'9' => N9,
                _ => Invalid,
            },
            N3 => match c {
                b'0' => N30,
                _ => Invalid,
            },
            N30 => match c {
                b'0' => N300,
                _ => Invalid,
            },
            N300 => match c {
                b'8' => N3008,
                _ => Invalid,
            },
            N3008 => match c {
                b';' => self.begin_capture(false),
                _ => Invalid,
            },
            N1 => match c {
                b';' => self.begin_capture(false),
                b'0' => N10,
                b'1' => N11,
                b'2' => N12,
                b'3' => N13,
                b'4' => N14,
                b'5' => N15,
                b'6' => N16,
                b'7' => N17,
                b'8' => N18,
                b'9' => N19,
                _ => Invalid,
            },
            N10 => match c {
                b';' => self.begin_capture(false),
                b'4' => N104,
                _ => Invalid,
            },
            N104 => match c {
                b';' => self.begin_capture(false),
                _ => Invalid,
            },
            N11 => match c {
                b';' => self.begin_capture(false),
                b'0' => N110,
                b'1' => N111,
                b'2' => N112,
                b'3' => N113,
                b'4' => N114,
                b'5' => N115,
                b'6' => N116,
                b'7' => N117,
                b'8' => N118,
                b'9' => N119,
                _ => Invalid,
            },
            N4 | N12 | N14 | N15 | N16 | N17 | N18 | N19 | N21 | N110 | N111 | N112 | N113
            | N114 | N115 | N116 | N117 | N118 | N119 => match c {
                b';' => self.begin_capture(false),
                _ => Invalid,
            },
            N13 => match c {
                b';' => self.begin_capture(false),
                b'3' => N133,
                _ => Invalid,
            },
            N2 => match c {
                b';' => self.begin_capture(false),
                b'1' => N21,
                b'2' => N22,
                _ => Invalid,
            },
            N5 => match c {
                b';' => self.begin_capture(false),
                b'2' => N52,
                b'5' => N55,
                _ => Invalid,
            },
            N6 => match c {
                b'6' => N66,
                _ => Invalid,
            },
            N52 | N66 => match c {
                b';' => self.begin_capture(true),
                _ => Invalid,
            },
            N55 => match c {
                b'2' => N552,
                _ => Invalid,
            },
            N7 => match c {
                b';' => self.begin_capture(false),
                b'2' => N72,
                b'7' => N77,
                _ => Invalid,
            },
            N72 => match c {
                b';' => self.begin_capture(true),
                _ => Invalid,
            },
            N77 => match c {
                b'7' => N777,
                _ => Invalid,
            },
            N133 => match c {
                b';' => self.begin_capture(false),
                b'7' => N1337,
                _ => Invalid,
            },
            N552 => match c {
                b'2' => N5522,
                _ => Invalid,
            },
            N1337 => match c {
                b';' => self.begin_capture(false),
                _ => Invalid,
            },
            N5522 => match c {
                b';' => self.begin_capture(true),
                _ => Invalid,
            },
            N0 | N22 | N777 | N8 | N9 => match c {
                b';' => self.begin_capture(false),
                _ => Invalid,
            },
            Invalid => Invalid,
        };
    }

    /// Begin capturing trailing data in the given mode. The separator byte
    /// itself is consumed by the state machine; payload bytes that follow
    /// are captured (Ghostty `captureTrailing`). The prefix state is
    /// preserved (Ghostty keeps the state while the capture short-circuits
    /// `next`), so `end` can dispatch on it.
    fn begin_capture(&mut self, allocating: bool) -> State {
        let state = self.state;
        self.capture = Some(if allocating {
            Capture::Allocating {
                data: Vec::with_capacity(MAX_BUF.min(self.max_allocating_bytes)),
                max_bytes: self.max_allocating_bytes,
            }
        } else {
            Capture::Fixed { len: 0 }
        });
        state
    }

    /// End the sequence and return the parsed command, if any. The optional
    /// terminator byte determines the response terminator for commands that
    /// demand a response.
    pub fn end(&mut self, terminator_ch: Option<u8>) -> Option<Command> {
        let terminator = Terminator::init(terminator_ch);
        let command = match self.state {
            State::Start | State::Invalid => None,
            State::N0 | State::N2 => parsers::change_window_title(self),
            State::N1 => parsers::change_window_icon(self),
            State::N4
            | State::N5
            | State::N10
            | State::N11
            | State::N12
            | State::N13
            | State::N14
            | State::N15
            | State::N16
            | State::N17
            | State::N18
            | State::N19
            | State::N104
            | State::N110
            | State::N111
            | State::N112
            | State::N113
            | State::N114
            | State::N115
            | State::N116
            | State::N117
            | State::N118
            | State::N119 => parsers::color(self, terminator),
            State::N7 => parsers::report_pwd(self),
            State::N8 => parsers::hyperlink(self),
            State::N9 => parsers::osc9(self),
            State::N21 => parsers::kitty_color(self),
            State::N22 => parsers::mouse_shape(self),
            State::N52 => parsers::clipboard(self),
            State::N55
            | State::N3
            | State::N30
            | State::N300
            | State::N6
            | State::N77
            | State::N552 => None,
            State::N3008 => parsers::context_signal(self),
            State::N66 => parsers::kitty_text_sizing(self),
            State::N72 => parsers::kitty_dnd(self),
            State::N133 => parsers::semantic_prompt(self),
            State::N777 => parsers::rxvt_extension(self),
            State::N1337 => parsers::iterm2(self),
            State::N5522 => parsers::kitty_clipboard(self),
        };
        if command.is_none() {
            // Ghostty leaves invalid/malformed states invalid so the
            // remainder of the sequence is discarded.
            if self.state != State::Invalid {
                self.state = State::Invalid;
            }
        }
        command
    }

    /// Borrow the current capture payload (used by the parsers).
    fn payload(&self) -> Option<&[u8]> {
        match &self.capture {
            Some(Capture::Fixed { len }) => Some(&self.buffer[..*len]),
            Some(Capture::Allocating { data, .. }) => Some(data),
            None => None,
        }
    }

    /// Take the captured payload as an owned `Vec<u8>`.
    fn take_payload(&mut self) -> Vec<u8> {
        match self.capture.take() {
            Some(Capture::Fixed { len }) => self.buffer[..len].to_vec(),
            Some(Capture::Allocating { data, .. }) => data,
            None => Vec::new(),
        }
    }
}

impl Default for Parser {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_all(input: &[u8], terminator: Option<u8>) -> Option<Command> {
        let mut p = Parser::new();
        for &b in input {
            p.next(b);
        }
        p.end(terminator)
    }

    fn parse_split(input: &[u8], split: usize, terminator: Option<u8>) -> Option<Command> {
        let mut p = Parser::new();
        for &b in &input[..split] {
            p.next(b);
        }
        for &b in &input[split..] {
            p.next(b);
        }
        p.end(terminator)
    }

    #[test]
    fn every_split_matches_whole() {
        let cases: &[(&[u8], Option<u8>)] = &[
            (b"0;hello world", None),
            (b"2;", Some(b'\x07')),
            (b"7;file:///tmp/x", None),
            (b"8;id=foo;https://example.com", None),
            (b"8;;", None),
            (b"133;A;aid=14;cl=line", None),
            (b"133;C;cmdline=$'echo hi'", None),
            (b"133;L", None),
            (b"9;Alert!", None),
            (b"9;1;250", None),
            (b"9;4;1;42", None),
            (b"9;12", None),
            (b"777;notify;Title;Body", None),
            (b"52;c;aGVsbG8=", None),
            (b"22;cell", None),
            (b"21;1;rgb:ff/00/00", None),
            (b"4;0;rgb:ffff/0000/0000;1;?", None),
            (b"104;0;1", None),
            (b"10;?", None),
            (b"11;rgb:0/0/0", None),
            (b"3008;x=1", None),
            (b"66;1", None),
            (b"72;1;text", None),
            (b"5522;c;aGVsbG8=", None),
            (b"1337;File=name", None),
            (b"55;garbage", None),
            (b"99999;garbage", None),
        ];
        for (input, term) in cases {
            let whole = parse_all(input, *term);
            for split in 0..=input.len() {
                let chunked = parse_split(input, split, *term);
                assert_eq!(
                    chunked,
                    whole,
                    "split {split} of {:?} diverged",
                    String::from_utf8_lossy(input)
                );
            }
        }
    }

    #[test]
    fn unknown_prefix_is_invalid_and_discards() {
        let mut p = Parser::new();
        p.next(b'X');
        assert_eq!(p.state(), State::Invalid);
        p.next(b';');
        p.next(b'x');
        assert_eq!(p.end(None), None);
        assert_eq!(p.state(), State::Invalid);
    }

    #[test]
    fn fixed_capture_bound_is_enforced() {
        // The parser reserves one byte for the NUL terminator, so a fixed
        // capture holds at most MAX_BUF-1 payload bytes. A payload of
        // exactly MAX_BUF-1 bytes succeeds...
        let mut p = Parser::new();
        for &b in b"0;" {
            p.next(b);
        }
        for _ in 0..(MAX_BUF - 1) {
            p.next(b'a');
        }
        assert_eq!(
            p.end(None),
            Some(Command::ChangeWindowTitle("a".repeat(MAX_BUF - 1)))
        );

        // ...and the MAX_BUF-th payload byte overflows into invalid.
        let mut p = Parser::new();
        for &b in b"0;" {
            p.next(b);
        }
        for _ in 0..MAX_BUF {
            p.next(b'a');
        }
        assert_eq!(p.state(), State::Invalid);
        assert_eq!(p.end(None), None);
    }

    #[test]
    fn allocating_capture_bound_is_enforced() {
        let mut p = Parser::new();
        p.max_allocating_bytes = 16;
        for &b in b"52;" {
            p.next(b);
        }
        for _ in 0..16 {
            p.next(b'a');
        }
        assert_eq!(p.payload().unwrap().len(), 16);
        p.next(b'a');
        assert_eq!(p.state(), State::Invalid);
    }

    #[test]
    fn invalid_mid_payload_keeps_bound() {
        // A malformed payload that would otherwise grow must not grow: once
        // the bound is hit the state is invalid and further bytes are
        // dropped.
        let mut p = Parser::new();
        p.max_allocating_bytes = 4;
        for &b in b"52;abcde" {
            p.next(b);
        }
        assert_eq!(p.state(), State::Invalid);
        assert_eq!(p.end(None), None);
        p.next(b'e');
        assert_eq!(p.state(), State::Invalid);
    }
}

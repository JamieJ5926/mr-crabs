//! APC (Application Program Command) handling, ported from Ghostty
//! `src/terminal/apc.zig`.
//!
//! APC sequences (`ESC _` or C1 `0x9F` ... `ST`) carry protocol payloads.
//! Ghostty recognizes two protocols: kitty graphics (`G` immediately) and
//! the Ghostty glyph protocol (`25a1;...`). vte does not deliver APC strings,
//! so the terminal layer uses [`Scanner`] to separate APC payload bytes from
//! the stream before feeding the rest to vte.
//!
//! Bounds: per-protocol [`Handler::max_bytes`], plus an optional bounded
//! capture of unsupported identifiers ([`Handler::unknown_max_bytes`]).
//! Exceeding a bound discards the remainder of the sequence.

use crate::limits::{APC_GLYPH_MAX_BYTES, APC_KITTY_MAX_BYTES, APC_UNKNOWN_MAX_BYTES};

/// The Ghostty glyph protocol identifier (`apc/glyph.zig`).
pub const GLYPH_IDENTIFIER: &[u8] = b"25a1";

/// Recognized APC protocols.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum Protocol {
    Kitty,
    Glyph,
}

impl Protocol {
    pub fn default_max_bytes(self) -> usize {
        match self {
            Self::Kitty => APC_KITTY_MAX_BYTES,
            Self::Glyph => APC_GLYPH_MAX_BYTES,
        }
    }
}

/// A recognized or unsupported APC command.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Command {
    /// Kitty graphics protocol command payload (base64-encoded image data is
    /// NOT decoded here; decoding belongs to the graphics slice).
    Kitty { payload: Vec<u8> },
    /// Ghostty glyph protocol request.
    Glyph(GlyphRequest),
    /// An unsupported APC retained for the optional unknown callback.
    Unknown { content: Vec<u8>, truncated: bool },
}

/// A glyph protocol request: an action byte followed by `key=value` pairs.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GlyphRequest {
    pub action: GlyphAction,
    pub pairs: Vec<(String, String)>,
}

/// Glyph protocol actions.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GlyphAction {
    Query,
    Set,
    Delete,
}

/// The APC handler (Ghostty `apc.Handler`).
pub struct Handler {
    state: State,
    /// Maximum content bytes retained for unsupported APC identifiers; zero
    /// drops and ignores unknown APC values.
    pub unknown_max_bytes: usize,
    /// Maximum bytes each APC protocol can buffer.
    pub max_bytes: std::collections::HashMap<Protocol, usize>,
}

enum State {
    Inactive,
    Ignore,
    Identify {
        len: usize,
        buf: [u8; GLYPH_IDENTIFIER.len()],
    },
    Kitty {
        data: Vec<u8>,
        max_bytes: usize,
    },
    Glyph {
        data: Vec<u8>,
        max_bytes: usize,
    },
    Unknown {
        data: Vec<u8>,
        max_bytes: usize,
        truncated: bool,
    },
}

impl Handler {
    pub fn new() -> Self {
        let mut max_bytes = std::collections::HashMap::new();
        max_bytes.insert(Protocol::Kitty, Protocol::Kitty.default_max_bytes());
        max_bytes.insert(Protocol::Glyph, Protocol::Glyph.default_max_bytes());
        Self {
            state: State::Inactive,
            unknown_max_bytes: APC_UNKNOWN_MAX_BYTES,
            max_bytes,
        }
    }

    pub fn deinit(&mut self) {
        self.state = State::Inactive;
    }

    /// Begin a new APC sequence.
    pub fn start(&mut self) {
        self.state = State::Identify {
            len: 0,
            buf: [0; GLYPH_IDENTIFIER.len()],
        };
    }

    #[cfg(test)]
    fn state(&self) -> &State {
        &self.state
    }

    /// Feed one APC payload byte.
    pub fn feed(&mut self, byte: u8) {
        match &mut self.state {
            State::Inactive => unreachable!("feed before start"),
            State::Ignore => {}
            State::Unknown {
                data,
                max_bytes,
                truncated,
            } => {
                append_bounded(data, byte, *max_bytes, truncated);
            }
            State::Identify { len, buf } => {
                if *len == 0 && byte == b'G' {
                    // Kitty graphics detected immediately.
                    let max = self
                        .max_bytes
                        .get(&Protocol::Kitty)
                        .copied()
                        .unwrap_or_else(|| Protocol::Kitty.default_max_bytes());
                    self.state = State::Kitty {
                        data: Vec::new(),
                        max_bytes: max,
                    };
                    return;
                }
                if byte == b';' {
                    if buf[..*len] == *GLYPH_IDENTIFIER {
                        let max = self
                            .max_bytes
                            .get(&Protocol::Glyph)
                            .copied()
                            .unwrap_or_else(|| Protocol::Glyph.default_max_bytes());
                        self.state = State::Glyph {
                            data: Vec::new(),
                            max_bytes: max,
                        };
                    } else {
                        // Copy the identifier prefix out of the borrow so the
                        // handler can be reborrowed mutably below.
                        let prefix = buf[..*len].to_vec();
                        self.begin_unknown(&prefix, &[byte]);
                    }
                    return;
                }
                if *len >= buf.len() {
                    let prefix = buf[..*len].to_vec();
                    self.begin_unknown(&prefix, &[byte]);
                    return;
                }
                buf[*len] = byte;
                *len += 1;
                // Once the buffered input is no longer a prefix of a known
                // protocol, it is an unsupported identifier.
                if self.unknown_max_bytes > 0 && byte != GLYPH_IDENTIFIER[*len - 1] {
                    let captured = buf[..*len].to_vec();
                    self.begin_unknown(&captured, &[]);
                }
            }
            State::Kitty { data, max_bytes } => {
                if data.len() >= *max_bytes {
                    self.state = State::Ignore;
                    return;
                }
                data.push(byte);
            }
            State::Glyph { data, max_bytes } => {
                if data.len() >= *max_bytes {
                    self.state = State::Ignore;
                    return;
                }
                data.push(byte);
            }
        }
    }

    /// Feed a slice of APC payload bytes in bulk.
    pub fn feed_slice(&mut self, bytes: &[u8]) {
        let mut rem = bytes;
        while !rem.is_empty() {
            match &mut self.state {
                State::Inactive => unreachable!("feed before start"),
                State::Ignore => return,
                State::Unknown {
                    data,
                    max_bytes,
                    truncated,
                } => {
                    append_bounded_slice(data, rem, *max_bytes, truncated);
                    return;
                }
                State::Identify { .. } => {
                    self.feed(rem[0]);
                    rem = &rem[1..];
                }
                State::Kitty { data, max_bytes } => {
                    let room = max_bytes.saturating_sub(data.len());
                    if room == 0 {
                        self.state = State::Ignore;
                        return;
                    }
                    let take = room.min(rem.len());
                    data.extend_from_slice(&rem[..take]);
                    if take < rem.len() {
                        self.state = State::Ignore;
                    }
                    return;
                }
                State::Glyph { data, max_bytes } => {
                    let room = max_bytes.saturating_sub(data.len());
                    if room == 0 {
                        self.state = State::Ignore;
                        return;
                    }
                    let take = room.min(rem.len());
                    data.extend_from_slice(&rem[..take]);
                    if take < rem.len() {
                        self.state = State::Ignore;
                    }
                    return;
                }
            }
        }
    }

    /// Complete the current APC, returning the command if any.
    pub fn end(&mut self) -> Option<Command> {
        let command = match &mut self.state {
            State::Inactive => unreachable!("end before start"),
            State::Ignore | State::Identify { .. } => None,
            State::Unknown {
                data, truncated, ..
            } => {
                let content = std::mem::take(data);
                Some(Command::Unknown {
                    content,
                    truncated: *truncated,
                })
            }
            State::Kitty { data, .. } => Some(Command::Kitty {
                payload: std::mem::take(data),
            }),
            State::Glyph { data, .. } => {
                parse_glyph_request(std::mem::take(data)).map(Command::Glyph)
            }
        };
        self.state = State::Inactive;
        command
    }

    fn begin_unknown(&mut self, prefix: &[u8], suffix: &[u8]) {
        let max_bytes = self.unknown_max_bytes;
        if max_bytes == 0 {
            self.state = State::Ignore;
            return;
        }
        let mut data = Vec::new();
        let mut truncated = false;
        append_bounded_slice(&mut data, prefix, max_bytes, &mut truncated);
        append_bounded_slice(&mut data, suffix, max_bytes, &mut truncated);
        self.state = State::Unknown {
            data,
            max_bytes,
            truncated,
        };
    }
}

fn append_bounded(data: &mut Vec<u8>, byte: u8, max_bytes: usize, truncated: &mut bool) {
    if data.len() >= max_bytes {
        *truncated = true;
        return;
    }
    data.push(byte);
}

fn append_bounded_slice(data: &mut Vec<u8>, bytes: &[u8], max_bytes: usize, truncated: &mut bool) {
    let room = max_bytes.saturating_sub(data.len());
    if bytes.len() > room {
        *truncated = true;
    }
    data.extend_from_slice(&bytes[..room.min(bytes.len())]);
}

fn parse_glyph_request(data: Vec<u8>) -> Option<GlyphRequest> {
    let action = match data.first() {
        Some(b'q') => GlyphAction::Query,
        Some(b's') => GlyphAction::Set,
        Some(b'd') => GlyphAction::Delete,
        _ => return None,
    };
    let rest = if data.first() == Some(&b';') {
        &data[1..]
    } else {
        data.as_slice()
    };
    let mut pairs = Vec::new();
    for item in rest.split(|&b| b == b';') {
        if let Some(eq) = item.iter().position(|&b| b == b'=') {
            pairs.push((
                String::from_utf8_lossy(&item[..eq]).into_owned(),
                String::from_utf8_lossy(&item[eq + 1..]).into_owned(),
            ));
        }
    }
    Some(GlyphRequest { action, pairs })
}

impl Default for Handler {
    fn default() -> Self {
        Self::new()
    }
}

/// Byte-level APC scanner for streams vte cannot see.
///
/// vte consumes APC strings (`ESC _ ... ST`, C1 `0x9F ... ST`) silently, so
/// the terminal layer routes bytes through this scanner first: non-APC bytes
/// pass through to the vte processor, while an APC string's payload bytes
/// are delivered to the APC [`Handler`].
///
/// Termination matches Ghostty's `Parser.zig`: `ST` (`ESC \` or C1 `0x9C`)
/// terminates normally; `CAN`/`SUB`/`ESC`/C1 bytes abort the string.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScannerState {
    Ground,
    Escape,
    Apc,
    ApcEscape,
}

/// The outcome of one scanned byte.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ScanStep {
    /// Byte belongs to the VT stream; deliver to vte.
    Stream(u8),
    /// APC string started (call the APC handler's `start`).
    Started,
    /// APC string ended normally (terminated by ST); deliver the payload
    /// bytes fed since `Started` to the APC handler's `end`.
    Ended,
    /// APC string aborted by CAN/SUB/ESC/C1.
    Aborted,
    /// APC control byte consumed while waiting to distinguish ESC ST from
    /// an aborting ESC sequence. Do not feed this byte to the handler.
    Pending,
    /// A previously pending ESC and the current non-APC byte both belong to
    /// the VT stream, in this order.
    StreamPair(u8, u8),
    /// APC payload byte; deliver to the APC handler's `feed`.
    Payload,
}

pub struct Scanner {
    state: ScannerState,
}

impl Scanner {
    pub fn new() -> Self {
        Self {
            state: ScannerState::Ground,
        }
    }

    pub fn state(&self) -> ScannerState {
        self.state
    }

    pub fn reset(&mut self) {
        self.state = ScannerState::Ground;
    }

    /// Scan one byte. In APC state, payload bytes are NOT returned (the
    /// caller feeds them to the APC handler); only the stream byte, start,
    /// and end markers are returned.
    pub fn next(&mut self, byte: u8) -> ScanStep {
        match self.state {
            ScannerState::Ground => match byte {
                0x1b => {
                    self.state = ScannerState::Escape;
                    ScanStep::Pending
                }
                0x9f => {
                    // C1 APC (8-bit)
                    self.state = ScannerState::Apc;
                    ScanStep::Started
                }
                _ => ScanStep::Stream(byte),
            },
            ScannerState::Escape => {
                if byte == b'_' {
                    self.state = ScannerState::Apc;
                    ScanStep::Started
                } else if byte == 0x1b {
                    // Release the previous ESC while retaining this ESC as
                    // the possible prefix of a subsequent APC sequence.
                    self.state = ScannerState::Escape;
                    ScanStep::Stream(0x1b)
                } else {
                    self.state = ScannerState::Ground;
                    ScanStep::StreamPair(0x1b, byte)
                }
            }
            ScannerState::Apc => match byte {
                0x1b => {
                    // Defer the decision until the following byte: ESC \
                    // terminates the string, while any other ESC sequence
                    // aborts it.
                    self.state = ScannerState::ApcEscape;
                    ScanStep::Pending
                }
                0x9c => {
                    // C1 ST
                    self.state = ScannerState::Ground;
                    ScanStep::Ended
                }
                0x18 | 0x1a => {
                    // CAN / SUB abort the string.
                    self.state = ScannerState::Ground;
                    ScanStep::Aborted
                }
                0x80..=0x9b | 0x9d..=0x9f => {
                    // Other C1 bytes abort the string (Ghostty treats most
                    // C1 as non-payload and exits the state).
                    self.state = ScannerState::Ground;
                    ScanStep::Aborted
                }
                _ => {
                    // Payload byte; the caller feeds it to the APC handler.
                    ScanStep::Payload
                }
            },
            ScannerState::ApcEscape => {
                self.state = ScannerState::Ground;
                if byte == b'\\' {
                    ScanStep::Ended
                } else {
                    ScanStep::Aborted
                }
            }
        }
    }
}

impl Default for Scanner {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_apc_command() {
        let mut h = Handler::new();
        h.start();
        for &c in b"Xabcdef1234" {
            h.feed(c);
        }
        assert!(h.end().is_none());
    }

    #[test]
    fn capture_unknown_apc_command() {
        let mut h = Handler::new();
        h.unknown_max_bytes = 5;
        h.start();
        h.feed_slice(b"abcd;payload");
        let result = h.end().unwrap();
        match result {
            Command::Unknown { content, truncated } => {
                assert_eq!(content, b"abcd;".to_vec());
                assert!(truncated);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn identify_overflow_and_mismatch() {
        let mut h = Handler::new();
        h.start();
        for &c in b"abcde;payload" {
            h.feed(c);
        }
        assert!(h.end().is_none());

        let mut h = Handler::new();
        h.start();
        for &c in b"25a" {
            h.feed(c);
        }
        assert!(h.end().is_none());
    }

    #[test]
    fn valid_glyph_command() {
        let mut h = Handler::new();
        h.start();
        h.feed_slice(b"25a1;q;cp=E0A0");
        let result = h.end().unwrap();
        match result {
            Command::Glyph(req) => {
                assert_eq!(req.action, GlyphAction::Query);
                assert_eq!(req.pairs, vec![("cp".to_string(), "E0A0".to_string())]);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn garbage_glyph_command() {
        let mut h = Handler::new();
        h.start();
        for &c in b"25a1;X" {
            h.feed(c);
        }
        assert!(h.end().is_none());
    }

    #[test]
    fn kitty_command_and_bounds() {
        let mut h = Handler::new();
        h.start();
        h.feed_slice(b"Gf=24,s=10,v=20;payload");
        let result = h.end().unwrap();
        match result {
            Command::Kitty { payload } => {
                assert_eq!(payload, b"f=24,s=10,v=20;payload".to_vec());
            }
            other => panic!("unexpected {other:?}"),
        }

        let mut h = Handler::new();
        h.max_bytes.insert(Protocol::Kitty, 4);
        h.start();
        h.feed_slice(b"Ga=t;abcd");
        h.feed(b'e');
        assert!(matches!(h.state(), State::Ignore));
        assert!(h.end().is_none());
    }

    #[test]
    fn scanner_detects_and_terminates() {
        let mut s = Scanner::new();
        let mut apc_payloads = 0;
        let mut stream = Vec::new();
        let mut saw_start = false;
        let mut ended = false;
        for &b in b"AB\x1b_Gpayload\x1b\\CD" {
            match s.next(b) {
                ScanStep::Stream(byte) => stream.push(byte),
                ScanStep::Started => saw_start = true,
                ScanStep::Payload => apc_payloads += 1,
                ScanStep::StreamPair(a, b) => stream.extend_from_slice(&[a, b]),
                ScanStep::Pending => {}
                ScanStep::Ended => ended = true,
                ScanStep::Aborted => panic!("unexpected abort"),
            }
        }
        assert!(saw_start && ended);
        assert_eq!(apc_payloads, 8); // "Gpayload"
        assert_eq!(stream, b"ABCD".to_vec());
        assert_eq!(s.state(), ScannerState::Ground);
    }

    #[test]
    fn scanner_aborts_on_can() {
        let mut s = Scanner::new();
        let steps: Vec<ScanStep> = b"X\x1b_Y\x18".iter().map(|&b| s.next(b)).collect();
        assert_eq!(steps[0], ScanStep::Stream(b'X'));
        assert_eq!(steps[1], ScanStep::Pending);
        assert_eq!(steps[2], ScanStep::Started);
        assert_eq!(steps[3], ScanStep::Payload);
        assert_eq!(steps[4], ScanStep::Aborted);
        assert_eq!(s.state(), ScannerState::Ground);
    }

    #[test]
    fn scanner_escape_after_apc_aborts() {
        let mut s = Scanner::new();
        let _ = s.next(b'\x1b');
        assert_eq!(s.next(b'_'), ScanStep::Started);
        // ESC inside APC waits for ST; any other following byte aborts.
        assert_eq!(s.next(b'\x1b'), ScanStep::Pending);
        assert_eq!(s.next(b'A'), ScanStep::Aborted);
    }
}

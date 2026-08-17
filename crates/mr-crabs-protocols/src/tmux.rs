//! tmux control mode parsing, ported from Ghostty
//! `src/terminal/tmux/control.zig`.
//!
//! The parser consumes the tmux control-mode byte stream (delivered inside
//! the `DCS 1000p ... ST` envelope by the [`crate::dcs::Handler`]) and
//! produces structured [`Notification`]s. Unknown output in the idle state
//! breaks the session (an `Exit` notification is returned); an oversized
//! buffer also breaks and reports `Exit`.

use crate::limits::TMUX_MAX_BYTES;

/// Possible notification types from tmux control mode.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Notification {
    /// Entering tmux control mode (synthetic; sent by the DCS hook).
    Enter,
    /// Exit (synthetic on DCS unhook, or when the connection breaks).
    Exit,
    /// End of a begin/end block with the raw output.
    BlockEnd(Vec<u8>),
    /// End of a begin/error block with the raw output.
    BlockErr(Vec<u8>),
    /// Raw output from a pane.
    Output { pane_id: usize, data: Vec<u8> },
    /// The client is now attached to session `id` named `name`.
    SessionChanged { id: usize, name: String },
    /// A session was created or destroyed.
    SessionsChanged,
    /// The layout of window `window_id` changed.
    LayoutChange {
        window_id: usize,
        layout: String,
        visible_layout: String,
        raw_flags: String,
    },
    /// The window `id` was linked to the current session.
    WindowAdd { id: usize },
    /// The window `id` was renamed to `name`.
    WindowRenamed { id: usize, name: String },
    /// The active pane in window `window_id` changed to pane `pane_id`.
    WindowPaneChanged { window_id: usize, pane_id: usize },
    /// The client detached.
    ClientDetached { client: String },
    /// The client is now attached to session `session_id` named `name`.
    ClientSessionChanged {
        client: String,
        session_id: usize,
        name: String,
    },
}

/// The control stream exceeded its configured byte bound.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ControlError;

/// The tmux control-mode parser.
pub struct ControlParser {
    state: State,
    buffer: Vec<u8>,
    /// Maximum buffer size in bytes; exceeding it breaks the session.
    pub max_bytes: usize,
}

enum State {
    Idle,
    Broken,
    Notification,
    Block,
}

impl ControlParser {
    pub fn new() -> Self {
        Self {
            state: State::Idle,
            buffer: Vec::new(),
            max_bytes: TMUX_MAX_BYTES,
        }
    }

    pub fn deinit(&mut self) {
        self.buffer.clear();
        self.state = State::Broken;
    }

    pub fn is_broken(&self) -> bool {
        matches!(self.state, State::Broken)
    }

    /// Handle one byte of input. Returns a notification when one completes;
    /// `Err(ControlError)` when the byte limit is exceeded (the parser is
    /// broken and all further input is dropped, and an `Exit` notification
    /// has been emitted).
    pub fn put(&mut self, byte: u8) -> Result<Option<Notification>, ControlError> {
        if matches!(self.state, State::Broken) {
            return Ok(None);
        }
        if self.buffer.len() >= self.max_bytes {
            self.broken();
            return Err(ControlError);
        }

        match self.state {
            State::Broken => return Ok(None),
            State::Idle => {
                if byte != b'%' {
                    self.broken();
                    return Ok(Some(Notification::Exit));
                }
                self.buffer.clear();
                self.state = State::Notification;
            }
            State::Notification => {
                if byte == b'\n' {
                    match self.parse_notification() {
                        Some(n) => return Ok(Some(n)),
                        None => {
                            // %begin transitions to Block inside the parser;
                            // every other failure resets to idle.
                            if !matches!(self.state, State::Block) {
                                self.reset_idle();
                            }
                            return Ok(None);
                        }
                    }
                }
            }
            State::Block => {
                if byte == b'\n' {
                    let written = &self.buffer;
                    let idx = written
                        .iter()
                        .rposition(|&b| b == b'\n')
                        .map(|v| v + 1)
                        .unwrap_or(0);
                    let line = &written[idx..];

                    if let Some(terminator) = parse_block_terminator(line) {
                        let mut output = written[..idx].to_vec();
                        while output.last() == Some(&b'\r') || output.last() == Some(&b'\n') {
                            output.pop();
                        }
                        self.state = State::Idle;
                        return Ok(Some(match terminator {
                            BlockTerminator::End => Notification::BlockEnd(output),
                            BlockTerminator::Err => Notification::BlockErr(output),
                        }));
                    }
                }
            }
        }

        self.buffer.push(byte);
        Ok(None)
    }

    fn parse_notification(&mut self) -> Option<Notification> {
        debug_assert!(matches!(self.state, State::Notification));
        let mut line = self.buffer.clone();
        if line.last() == Some(&b'\r') {
            line.pop();
        }
        let cmd_end = line.iter().position(|&b| b == b' ').unwrap_or(line.len());
        let cmd = &line[..cmd_end];

        let notification = if cmd == b"%begin" {
            // We don't validate the begin tokens (tmux guarantees ordered
            // begin/end pairs); move to block accumulation.
            self.state = State::Block;
            self.buffer.clear();
            return None;
        } else if cmd == b"%output" {
            let (pane_id, data) = parse_capture2(line.as_slice(), b"%output %", 2)?;
            Notification::Output {
                pane_id,
                data: data.into_bytes(),
            }
        } else if cmd == b"%session-changed" {
            let (id, name) = parse_capture2(line.as_slice(), b"%session-changed $", 2)?;
            Notification::SessionChanged { id, name }
        } else if cmd == b"%sessions-changed" {
            if line.as_slice() != b"%sessions-changed" {
                return None;
            }
            Notification::SessionsChanged
        } else if cmd == b"%layout-change" {
            let rest = strip_prefix(line.as_slice(), b"%layout-change @")?;
            let (window_id, rest) = split_num(rest)?;
            let rest = rest.strip_prefix(b" ")?;
            let (layout, rest) = split_space(rest)?;
            let (visible_layout, raw_flags) = split_space(rest)?;
            Notification::LayoutChange {
                window_id,
                layout,
                visible_layout,
                raw_flags: String::from_utf8_lossy(raw_flags).into_owned(),
            }
        } else if cmd == b"%window-add" {
            let rest = strip_prefix(line.as_slice(), b"%window-add @")?;
            let (id, _) = split_num(rest)?;
            Notification::WindowAdd { id }
        } else if cmd == b"%window-renamed" {
            let (id, name) = parse_capture2(line.as_slice(), b"%window-renamed @", 2)?;
            Notification::WindowRenamed { id, name }
        } else if cmd == b"%window-pane-changed" {
            let rest = strip_prefix(line.as_slice(), b"%window-pane-changed @")?;
            let (window_id, rest) = split_num(rest)?;
            let rest = strip_prefix(rest, b" %")?;
            let (pane_id, _) = split_num(rest)?;
            Notification::WindowPaneChanged { window_id, pane_id }
        } else if cmd == b"%client-detached" {
            let rest = strip_prefix(line.as_slice(), b"%client-detached ")?;
            Notification::ClientDetached {
                client: String::from_utf8_lossy(rest).into_owned(),
            }
        } else if cmd == b"%client-session-changed" {
            let rest = strip_prefix(line.as_slice(), b"%client-session-changed ")?;
            let (client, rest) = split_space(rest)?;
            let rest = strip_prefix(rest, b"$")?;
            let (session_id, rest) = split_num(rest)?;
            // Ghostty matches `\$([0-9]+) (.+)$`: the name is the remainder
            // after the separating space and must be non-empty.
            let name = rest.strip_prefix(b" ")?;
            if name.is_empty() {
                return None;
            }
            Notification::ClientSessionChanged {
                client,
                session_id,
                name: String::from_utf8_lossy(name).into_owned(),
            }
        } else {
            // Unknown notification: clear the buffer and return to idle.
            self.state = State::Idle;
            self.buffer.clear();
            return None;
        };

        self.state = State::Idle;
        Some(notification)
    }

    /// Any other parse failure: clear the buffer and return to idle
    /// (Ghostty keeps the connection alive for malformed notifications).
    fn reset_idle(&mut self) {
        self.state = State::Idle;
        self.buffer.clear();
    }

    /// Mark the parser broken, releasing the buffer.
    fn broken(&mut self) {
        self.state = State::Broken;
        self.buffer.clear();
    }
}

enum BlockTerminator {
    End,
    Err,
}

/// Block payload is raw data, so a line only terminates a block if it
/// exactly matches tmux's `%end`/`%error` guard-line shape.
fn parse_block_terminator(line_raw: &[u8]) -> Option<BlockTerminator> {
    let mut line = line_raw;
    if line.last() == Some(&b'\r') {
        line = &line[..line.len() - 1];
    }

    let mut fields = line.split(|&b| b == b' ');
    let cmd = fields.next()?;
    let terminator = if cmd == b"%end" {
        BlockTerminator::End
    } else if cmd == b"%error" {
        BlockTerminator::Err
    } else {
        return None;
    };

    let time = fields.next()?;
    let command_id = fields.next()?;
    let flags = fields.next()?;
    if fields.next().is_some() {
        return None;
    }
    parse_usize(time)?;
    parse_usize(command_id)?;
    parse_usize(flags)?;
    Some(terminator)
}

fn parse_usize(bytes: &[u8]) -> Option<usize> {
    std::str::from_utf8(bytes).ok()?.parse().ok()
}

fn split_num(rest: &[u8]) -> Option<(usize, &[u8])> {
    let end = rest
        .iter()
        .position(|&b| !b.is_ascii_digit())
        .unwrap_or(rest.len());
    let num = parse_usize(&rest[..end])?;
    Some((num, &rest[end..]))
}

fn split_space(rest: &[u8]) -> Option<(String, &[u8])> {
    let end = rest.iter().position(|&b| b == b' ').unwrap_or(rest.len());
    let first = String::from_utf8_lossy(&rest[..end]).into_owned();
    let rem = if end < rest.len() {
        &rest[end + 1..]
    } else {
        &rest[end..]
    };
    Some((first, rem))
}

fn strip_prefix<'a>(bytes: &'a [u8], prefix: &[u8]) -> Option<&'a [u8]> {
    bytes.strip_prefix(prefix)
}

/// Parse `%cmd $N rest` style lines: numeric id plus trailing string.
fn parse_capture2(line: &[u8], prefix: &[u8], _: usize) -> Option<(usize, String)> {
    let rest = strip_prefix(line, prefix)?;
    let (id, rest) = split_num(rest)?;
    let rest = rest.strip_prefix(b" ")?;
    Some((id, String::from_utf8_lossy(rest).into_owned()))
}

impl Default for ControlParser {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feed(parser: &mut ControlParser, input: &str) -> Vec<Notification> {
        let mut out = Vec::new();
        for &b in input.as_bytes() {
            if let Ok(Some(n)) = parser.put(b) {
                out.push(n);
            }
        }
        out
    }

    #[test]
    fn begin_end_empty() {
        let mut c = ControlParser::new();
        assert!(feed(&mut c, "%begin 1578922740 269 1\n").is_empty());
        assert!(feed(&mut c, "%end 1578922740 269 1").is_empty());
        let n = c.put(b'\n').unwrap().unwrap();
        assert_eq!(n, Notification::BlockEnd(vec![]));
    }

    #[test]
    fn begin_error_empty() {
        let mut c = ControlParser::new();
        feed(&mut c, "%begin 1578922740 269 1\n");
        feed(&mut c, "%error 1578922740 269 1");
        let n = c.put(b'\n').unwrap().unwrap();
        assert_eq!(n, Notification::BlockErr(vec![]));
    }

    #[test]
    fn begin_end_data() {
        let mut c = ControlParser::new();
        feed(&mut c, "%begin 1578922740 269 1\n");
        feed(&mut c, "hello\nworld\n");
        feed(&mut c, "%end 1578922740 269 1");
        let n = c.put(b'\n').unwrap().unwrap();
        assert_eq!(n, Notification::BlockEnd(b"hello\nworld".to_vec()));
    }

    #[test]
    fn block_payload_may_start_with_misleading_guard() {
        let mut c = ControlParser::new();
        feed(&mut c, "%begin 1 1 1\n");
        feed(&mut c, "%end not really\nhello\n");
        feed(&mut c, "%end 1 1 1");
        let n = c.put(b'\n').unwrap().unwrap();
        assert_eq!(
            n,
            Notification::BlockEnd(b"%end not really\nhello".to_vec())
        );

        let mut c = ControlParser::new();
        feed(&mut c, "%begin 1 1 1\n");
        feed(&mut c, "%error not really\nhello\n");
        feed(&mut c, "%error 1 1 1");
        let n = c.put(b'\n').unwrap().unwrap();
        assert_eq!(
            n,
            Notification::BlockErr(b"%error not really\nhello".to_vec())
        );
    }

    #[test]
    fn block_terminator_requires_exact_shape() {
        let mut c = ControlParser::new();
        feed(&mut c, "%begin 1 1 1\n");
        feed(&mut c, "%end 1 1 1 trailing\nhello\n");
        feed(&mut c, "%end 1 1 1");
        let n = c.put(b'\n').unwrap().unwrap();
        assert_eq!(
            n,
            Notification::BlockEnd(b"%end 1 1 1 trailing\nhello".to_vec())
        );

        let mut c = ControlParser::new();
        feed(&mut c, "%begin 1 1 1\n");
        feed(&mut c, "%end foo bar baz\nhello\n");
        feed(&mut c, "%end 1 1 1");
        let n = c.put(b'\n').unwrap().unwrap();
        assert_eq!(
            n,
            Notification::BlockEnd(b"%end foo bar baz\nhello".to_vec())
        );
    }

    #[test]
    fn output() {
        let mut c = ControlParser::new();
        feed(&mut c, "%output %42 foo bar baz");
        let n = c.put(b'\n').unwrap().unwrap();
        assert_eq!(
            n,
            Notification::Output {
                pane_id: 42,
                data: b"foo bar baz".to_vec()
            }
        );
    }

    #[test]
    fn session_changed() {
        let mut c = ControlParser::new();
        feed(&mut c, "%session-changed $42 foo");
        let n = c.put(b'\n').unwrap().unwrap();
        assert_eq!(
            n,
            Notification::SessionChanged {
                id: 42,
                name: "foo".into()
            }
        );
    }

    #[test]
    fn sessions_changed_cr() {
        let mut c = ControlParser::new();
        feed(&mut c, "%sessions-changed\r");
        let n = c.put(b'\n').unwrap().unwrap();
        assert_eq!(n, Notification::SessionsChanged);
    }

    #[test]
    fn layout_change() {
        let mut c = ControlParser::new();
        feed(
            &mut c,
            "%layout-change @2 1234x791,0,0{617x791,0,0,0,617x791,618,0,1} 1234x791,0,0{617x791,0,0,0,617x791,618,0,1} *-",
        );
        let n = c.put(b'\n').unwrap().unwrap();
        assert_eq!(
            n,
            Notification::LayoutChange {
                window_id: 2,
                layout: "1234x791,0,0{617x791,0,0,0,617x791,618,0,1}".into(),
                visible_layout: "1234x791,0,0{617x791,0,0,0,617x791,618,0,1}".into(),
                raw_flags: "*-".into(),
            }
        );
    }

    #[test]
    fn window_events() {
        let mut c = ControlParser::new();
        feed(&mut c, "%window-add @14");
        assert_eq!(
            c.put(b'\n').unwrap().unwrap(),
            Notification::WindowAdd { id: 14 }
        );

        let mut c = ControlParser::new();
        feed(&mut c, "%window-renamed @42 bar");
        assert_eq!(
            c.put(b'\n').unwrap().unwrap(),
            Notification::WindowRenamed {
                id: 42,
                name: "bar".into()
            }
        );

        let mut c = ControlParser::new();
        feed(&mut c, "%window-pane-changed @42 %2");
        assert_eq!(
            c.put(b'\n').unwrap().unwrap(),
            Notification::WindowPaneChanged {
                window_id: 42,
                pane_id: 2
            }
        );
    }

    #[test]
    fn client_events() {
        let mut c = ControlParser::new();
        feed(&mut c, "%client-detached /dev/pts/1");
        assert_eq!(
            c.put(b'\n').unwrap().unwrap(),
            Notification::ClientDetached {
                client: "/dev/pts/1".into()
            }
        );

        let mut c = ControlParser::new();
        feed(&mut c, "%client-session-changed /dev/pts/1 $2 mysession");
        assert_eq!(
            c.put(b'\n').unwrap().unwrap(),
            Notification::ClientSessionChanged {
                client: "/dev/pts/1".into(),
                session_id: 2,
                name: "mysession".into()
            }
        );
    }

    #[test]
    fn garbage_in_idle_breaks() {
        let mut c = ControlParser::new();
        let n = c.put(b'x').unwrap().unwrap();
        assert_eq!(n, Notification::Exit);
        assert!(c.is_broken());
        assert!(c.put(b'x').unwrap().is_none());
    }

    #[test]
    fn oversize_buffer_breaks() {
        let mut c = ControlParser::new();
        c.max_bytes = 8;
        for b in b"%begin" {
            assert!(c.put(*b).unwrap().is_none());
        }
        assert!(c.put(b'\n').unwrap().is_none());
        // Ghostty checks `len >= max_bytes` before appending, so exactly
        // `max_bytes` bytes fit and the next byte breaks the session.
        for _ in 0..8 {
            assert!(c.put(b'x').unwrap().is_none());
        }
        assert!(c.put(b'x').is_err());
        assert!(c.is_broken());
    }
}

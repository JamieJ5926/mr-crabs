//! DCS (Device Control String) command handling, ported from Ghostty
//! `src/terminal/dcs.zig`.
//!
//! The handler is hooked into the stream's `hook`/`put`/`unhook` points
//! (vte `Handler::hook`/`put`/`unhook`). It recognizes:
//!
//! * tmux control mode: `ESC P 1000 p` (the tmux control stream follows as
//!   the DCS payload, terminated by ST);
//! * XTGETTCAP: `ESC P + q` with hex-encoded terminfo keys;
//! * DECRQSS: `ESC P $ q` with a 1-2 byte setting name.
//!
//! All payloads are bounded by [`Handler::max_bytes`] (1 MiB default);
//! exceeding the bound discards the remainder.

use crate::limits::DCS_MAX_BYTES;
use crate::tmux::Notification;
use std::io::Write;

/// A DCS command resulting from hook/put/unhook.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Command {
    /// A tmux control-mode notification, including the synthetic `Enter`
    /// (hook) and `Exit` (unhook) lifecycle events.
    Tmux(Notification),
    /// XTGETTCAP request with uppercased hex-encoded keys.
    Xtgettcap { keys: Vec<Vec<u8>> },
    /// DECRQSS request.
    Decrqss(DecrqssRequest),
}

/// Supported DECRQSS settings (Ghostty `dcs.Command.DECRQSS`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DecrqssRequest {
    None,
    Sgr,
    Decscusr,
    Decstbm,
    Decslrm,
}

impl DecrqssRequest {
    /// Fixed upper bound for an encoded DECRPSS response (Ghostty).
    pub const MAX_RESPONSE_BYTES: usize = 256;

    /// Encode the response for this request using terminal state provided
    /// by the context (Ghostty `DECRQSS.encode`).
    pub fn encode(&self, ctx: &dyn DecrqssContext, out: &mut Vec<u8>) {
        let prefix_len = 5; // "\x1bP?$r" with the validity digit unknown yet.
        out.extend_from_slice(b"\x1bP0$r");
        debug_assert_eq!(out.len(), prefix_len);

        match self {
            Self::None => {}
            Self::Sgr => {
                ctx.sgr_attributes(out);
                out.push(b'm');
            }
            Self::Decscusr => {
                let blink = ctx.cursor_blinking();
                let style: u8 = match ctx.cursor_shape() {
                    CursorShapeKind::Block | CursorShapeKind::BlockHollow => {
                        if blink {
                            1
                        } else {
                            2
                        }
                    }
                    CursorShapeKind::Underline => {
                        if blink {
                            3
                        } else {
                            4
                        }
                    }
                    CursorShapeKind::Bar => {
                        if blink {
                            5
                        } else {
                            6
                        }
                    }
                };
                let _ = write!(out, "{style} q");
            }
            Self::Decstbm => {
                let _ = write!(
                    out,
                    "{};{}r",
                    ctx.scrolling_region_top() + 1,
                    ctx.scrolling_region_bottom() + 1
                );
            }
            Self::Decslrm => {
                if ctx.left_right_margins_enabled() {
                    let _ = write!(
                        out,
                        "{};{}s",
                        ctx.scrolling_region_left() + 1,
                        ctx.scrolling_region_right() + 1
                    );
                }
            }
        }
        // Rewrite the validity digit: 1 when the response body is nonempty.
        // Prefix is "\x1bP0$r" (ESC P digit $ r), digit at prefix_len-3.
        let valid = out.len() > prefix_len;
        out[prefix_len - 3] = if valid { b'1' } else { b'0' };
        out.extend_from_slice(b"\x1b\\");
    }
}

/// The cursor shape categories needed for the DECRQSS cursor style reply.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CursorShapeKind {
    Block,
    BlockHollow,
    Underline,
    Bar,
}

/// Terminal state a DECRQSS reply needs (implemented by the terminal crate).
pub trait DecrqssContext {
    /// The current SGR attributes, encoded like Ghostty `printAttributes`
    /// (a `;`-separated list without a trailing `m`).
    fn sgr_attributes(&self, out: &mut Vec<u8>);
    fn cursor_blinking(&self) -> bool;
    fn cursor_shape(&self) -> CursorShapeKind;
    fn scrolling_region_top(&self) -> usize;
    fn scrolling_region_bottom(&self) -> usize;
    fn left_right_margins_enabled(&self) -> bool;
    fn scrolling_region_left(&self) -> usize;
    fn scrolling_region_right(&self) -> usize;
}

/// DCS hook descriptor (the intro before the payload). The slices borrow
/// the vte hook call's runtime data (Ghostty `dcs.DcsIntro`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DcsIntro<'a> {
    pub params: &'a [u16],
    pub intermediates: &'a [u8],
    pub final_byte: u8,
}

/// The DCS handler state machine (Ghostty `dcs.Handler`).
pub struct Handler {
    state: State,
    /// Maximum bytes any DCS command can take; beyond this the handler
    /// discards the remainder (Ghostty default 1 MiB).
    pub max_bytes: usize,
    /// Whether tmux control mode recognition is enabled.
    pub tmux_enabled: bool,
}

enum State {
    Inactive,
    Ignore,
    Xtgettcap(Vec<u8>),
    Decrqss { data: [u8; 2], len: u8 },
    Tmux(Box<crate::tmux::ControlParser>),
}

impl Handler {
    pub fn new() -> Self {
        Self {
            state: State::Inactive,
            max_bytes: DCS_MAX_BYTES,
            tmux_enabled: true,
        }
    }

    pub fn deinit(&mut self) {
        self.discard();
    }

    /// Handle a DCS hook. Returns a command if one must run immediately.
    pub fn hook(&mut self, dcs: DcsIntro<'_>) -> Option<Command> {
        debug_assert!(matches!(self.state, State::Inactive));
        self.state = State::Ignore;

        let (state, command) = match (dcs.intermediates.len(), dcs.final_byte) {
            (0, b'p') => {
                // tmux control mode must start with ESC P 1000 p
                if !self.tmux_enabled || dcs.params.len() != 1 || dcs.params.first() != Some(&1000)
                {
                    return None;
                }
                (
                    State::Tmux(Box::default()),
                    Some(Command::Tmux(Notification::Enter)),
                )
            }
            (1, b'q') if dcs.intermediates[0] == b'+' => {
                // XTGETTCAP
                (State::Xtgettcap(Vec::new()), None)
            }
            (1, b'q') if dcs.intermediates[0] == b'$' => {
                // DECRQSS
                (
                    State::Decrqss {
                        data: [0; 2],
                        len: 0,
                    },
                    None,
                )
            }
            _ => return None,
        };
        self.state = state;
        command
    }

    /// Feed one DCS payload byte. Returns a tmux notification when one
    /// completes; an over-limit payload transitions to ignore and returns
    /// `Tmux(Exit)`.
    pub fn put(&mut self, byte: u8) -> Option<Command> {
        match &mut self.state {
            State::Inactive | State::Ignore => None,
            State::Tmux(parser) => match parser.put(byte) {
                Ok(Some(notification)) => Some(Command::Tmux(notification)),
                Ok(None) => None,
                Err(_) => Some(Command::Tmux(Notification::Exit)),
            },
            State::Xtgettcap(buf) => {
                if buf.len() >= self.max_bytes {
                    self.state = State::Ignore;
                    return None;
                }
                buf.push(byte);
                None
            }
            State::Decrqss { data, len } => {
                if usize::from(*len) >= data.len() {
                    self.state = State::Ignore;
                    return None;
                }
                data[usize::from(*len)] = byte;
                *len += 1;
                None
            }
        }
    }

    /// Unhook the DCS; returns the final command, if any.
    pub fn unhook(&mut self) -> Option<Command> {
        match std::mem::replace(&mut self.state, State::Inactive) {
            State::Inactive | State::Ignore => None,
            State::Tmux(_) => Some(Command::Tmux(Notification::Exit)),
            State::Xtgettcap(mut buf) => {
                // Upper-case all keys (Ghostty uppercases the collected
                // bytes) then split on ';'.
                for b in buf.iter_mut() {
                    b.make_ascii_uppercase();
                }
                let keys = buf
                    .split(|&b| b == b';')
                    .map(|k| k.to_vec())
                    .collect::<Vec<_>>();
                Some(Command::Xtgettcap { keys })
            }
            State::Decrqss { data, len } => {
                let request = match len {
                    0 => DecrqssRequest::None,
                    1 => match data[0] {
                        b'm' => DecrqssRequest::Sgr,
                        b'r' => DecrqssRequest::Decstbm,
                        b's' => DecrqssRequest::Decslrm,
                        _ => DecrqssRequest::None,
                    },
                    2 => match data[0] {
                        b' ' => match data[1] {
                            b'q' => DecrqssRequest::Decscusr,
                            _ => DecrqssRequest::None,
                        },
                        _ => DecrqssRequest::None,
                    },
                    _ => DecrqssRequest::None,
                };
                Some(Command::Decrqss(request))
            }
        }
    }

    fn discard(&mut self) {
        self.state = State::Inactive;
    }
}

impl Default for Handler {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_dcs_command() {
        let mut h = Handler::new();
        assert!(
            h.hook(DcsIntro {
                params: &[],
                intermediates: &[],
                final_byte: b'A'
            })
            .is_none()
        );
        assert!(matches!(h.state, State::Ignore));
        assert!(h.unhook().is_none());
        assert!(matches!(h.state, State::Inactive));
    }

    #[test]
    fn xtgettcap_command() {
        let mut h = Handler::new();
        assert!(
            h.hook(DcsIntro {
                params: &[],
                intermediates: b"+",
                final_byte: b'q'
            })
            .is_none()
        );
        for &b in b"536D756C78" {
            assert!(h.put(b).is_none());
        }
        let cmd = h.unhook().unwrap();
        match cmd {
            Command::Xtgettcap { keys } => {
                assert_eq!(keys, vec![b"536D756C78".to_vec()]);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn xtgettcap_mixed_case_and_multiple() {
        let mut h = Handler::new();
        assert!(
            h.hook(DcsIntro {
                params: &[],
                intermediates: b"+",
                final_byte: b'q'
            })
            .is_none()
        );
        for &b in b"536d756C78;536D756C78" {
            let _ = h.put(b);
        }
        let cmd = h.unhook().unwrap();
        match cmd {
            Command::Xtgettcap { keys } => {
                assert_eq!(keys, vec![b"536D756C78".to_vec(), b"536D756C78".to_vec()]);
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn decrqss_requests() {
        let mut h = Handler::new();
        assert!(
            h.hook(DcsIntro {
                params: &[],
                intermediates: b"$",
                final_byte: b'q'
            })
            .is_none()
        );
        assert!(h.put(b'm').is_none());
        assert_eq!(h.unhook(), Some(Command::Decrqss(DecrqssRequest::Sgr)));

        let mut h = Handler::new();
        assert!(
            h.hook(DcsIntro {
                params: &[],
                intermediates: b"$",
                final_byte: b'q'
            })
            .is_none()
        );
        assert!(h.put(b'z').is_none());
        assert_eq!(h.unhook(), Some(Command::Decrqss(DecrqssRequest::None)));

        let mut h = Handler::new();
        assert!(
            h.hook(DcsIntro {
                params: &[],
                intermediates: b"$",
                final_byte: b'q'
            })
            .is_none()
        );
        assert!(h.put(b' ').is_none());
        assert!(h.put(b'q').is_none());
        assert_eq!(h.unhook(), Some(Command::Decrqss(DecrqssRequest::Decscusr)));
    }

    #[test]
    fn decrqss_response_encoding() {
        struct Ctx;
        impl DecrqssContext for Ctx {
            fn sgr_attributes(&self, out: &mut Vec<u8>) {
                out.extend_from_slice(b"0;1;4:3");
            }
            fn cursor_blinking(&self) -> bool {
                false
            }
            fn cursor_shape(&self) -> CursorShapeKind {
                CursorShapeKind::Underline
            }
            fn scrolling_region_top(&self) -> usize {
                4
            }
            fn scrolling_region_bottom(&self) -> usize {
                19
            }
            fn left_right_margins_enabled(&self) -> bool {
                false
            }
            fn scrolling_region_left(&self) -> usize {
                0
            }
            fn scrolling_region_right(&self) -> usize {
                0
            }
        }

        let mut out = Vec::new();
        DecrqssRequest::None.encode(&Ctx, &mut out);
        assert_eq!(out, b"\x1bP0$r\x1b\\");

        let mut out = Vec::new();
        DecrqssRequest::Sgr.encode(&Ctx, &mut out);
        assert_eq!(out, b"\x1bP1$r0;1;4:3m\x1b\\");

        let mut out = Vec::new();
        DecrqssRequest::Decscusr.encode(&Ctx, &mut out);
        assert_eq!(out, b"\x1bP1$r4 q\x1b\\");

        let mut out = Vec::new();
        DecrqssRequest::Decstbm.encode(&Ctx, &mut out);
        assert_eq!(out, b"\x1bP1$r5;20r\x1b\\");

        let mut out = Vec::new();
        DecrqssRequest::Decslrm.encode(&Ctx, &mut out);
        assert_eq!(out, b"\x1bP0$r\x1b\\");
    }

    #[test]
    fn tmux_enter_and_exit() {
        let mut h = Handler::new();
        let cmd = h
            .hook(DcsIntro {
                params: &[1000],
                intermediates: &[],
                final_byte: b'p',
            })
            .unwrap();
        assert_eq!(cmd, Command::Tmux(Notification::Enter));
        assert!(h.unhook().is_some());
        // tmux without param 1000 is not recognized
        let mut h = Handler::new();
        assert!(
            h.hook(DcsIntro {
                params: &[1],
                intermediates: &[],
                final_byte: b'p'
            })
            .is_none()
        );
        let mut h = Handler::new();
        h.tmux_enabled = false;
        assert!(
            h.hook(DcsIntro {
                params: &[1000],
                intermediates: &[],
                final_byte: b'p'
            })
            .is_none()
        );
    }

    #[test]
    fn dcs_max_bytes_bounded() {
        let mut h = Handler::new();
        h.max_bytes = 8;
        assert!(
            h.hook(DcsIntro {
                params: &[],
                intermediates: b"+",
                final_byte: b'q'
            })
            .is_none()
        );
        for _ in 0..8 {
            assert!(h.put(b'X').is_none());
        }
        assert!(h.put(b'X').is_none()); // over limit -> ignore
        assert!(matches!(h.state, State::Ignore));
        assert!(h.unhook().is_none());
    }
}

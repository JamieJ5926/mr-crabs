//! Per-command OSC decoders, ported from Ghostty `src/terminal/osc/parsers/*`.
//!
//! Every decoder takes the bounded capture payload and produces an owned
//! [`Command`](crate::osc::Command). Malformed payloads mark the parser
//! invalid so the remainder of the sequence is discarded without allocation.

use super::{Command, Parser, State};
use crate::color::{self, ColorOperation, ColorRequest};
use crate::semantic_prompt::{Action, SemanticPrompt};

impl Parser {
    /// Mark the current sequence invalid (used by decoders on malformed
    /// payloads).
    pub fn fail(&mut self) {
        self.state = State::Invalid;
    }

    /// Payload bytes currently captured (after the `;` separator).
    pub fn captured(&self) -> &[u8] {
        self.payload().unwrap_or(&[])
    }

    /// Consume and return the captured payload.
    pub fn captured_owned(&mut self) -> Vec<u8> {
        self.take_payload()
    }
}

/// OSC 0/2: change the window title.
pub fn change_window_title(parser: &mut Parser) -> Option<Command> {
    let data = parser.captured();
    Some(Command::ChangeWindowTitle(
        String::from_utf8_lossy(data).into_owned(),
    ))
}

/// OSC 1: change the window icon.
pub fn change_window_icon(parser: &mut Parser) -> Option<Command> {
    let data = parser.captured();
    Some(Command::ChangeWindowIcon(
        String::from_utf8_lossy(data).into_owned(),
    ))
}

/// OSC 7: report the current working directory.
pub fn report_pwd(parser: &mut Parser) -> Option<Command> {
    let data = parser.captured();
    Some(Command::ReportPwd(
        String::from_utf8_lossy(data).into_owned(),
    ))
}

/// OSC 22: set the mouse shape.
pub fn mouse_shape(parser: &mut Parser) -> Option<Command> {
    let data = parser.captured();
    Some(Command::MouseShape(
        String::from_utf8_lossy(data).into_owned(),
    ))
}

/// OSC 8: hyperlinks. `payload` is `params;uri`. An empty URI ends the
/// active hyperlink; a non-empty URI with an `id` parameter starts a
/// hyperlink with that identity.
pub fn hyperlink(parser: &mut Parser) -> Option<Command> {
    let data = parser.captured();
    let Some(s) = data.iter().position(|&b| b == b';') else {
        parser.fail();
        return None;
    };
    let uri = &data[s + 1..];
    let kvs = &data[..s];

    let mut id: Option<String> = None;
    let mut kv_start = 0;
    while kv_start < kvs.len() {
        let kv_end = kvs[kv_start + 1..]
            .iter()
            .position(|&b| b == b':')
            .map(|i| kv_start + 1 + i)
            .unwrap_or(kvs.len());
        let kv = &kvs[kv_start..kv_end];
        let Some(v) = kv.iter().position(|&b| b == b'=') else {
            break;
        };
        let key = &kv[..v];
        let value = &kv[v + 1..];
        if key == b"id" && !value.is_empty() {
            id = Some(String::from_utf8_lossy(value).into_owned());
        }
        kv_start = kv_end + 1;
    }

    if uri.is_empty() {
        if id.is_some() {
            parser.fail();
            return None;
        }
        return Some(Command::HyperlinkEnd);
    }

    Some(Command::HyperlinkStart {
        id,
        uri: String::from_utf8_lossy(uri).into_owned(),
    })
}

/// OSC 133: semantic prompts.
pub fn semantic_prompt(parser: &mut Parser) -> Option<Command> {
    let data = parser.captured();
    if data.is_empty() {
        parser.fail();
        return None;
    }

    let make = |action: Action, options: &[u8]| -> Command {
        Command::SemanticPrompt(SemanticPrompt {
            action,
            options_unvalidated: String::from_utf8_lossy(options).into_owned(),
        })
    };

    let action = match data[0] {
        b'A' => Action::FreshLineNewPrompt,
        b'B' => Action::EndPromptStartInput,
        b'I' => Action::EndPromptStartInputTerminateEol,
        b'C' => Action::EndInputStartOutput,
        b'D' => Action::EndCommand,
        b'L' => {
            if data.len() > 1 {
                parser.fail();
                return None;
            }
            return Some(make(Action::FreshLine, &[]));
        }
        b'N' => Action::NewCommand,
        b'P' => Action::PromptStart,
        _ => {
            parser.fail();
            return None;
        }
    };

    if data.len() == 1 {
        return Some(make(action, &[]));
    }
    if data[1] != b';' {
        parser.fail();
        return None;
    }
    Some(make(action, &data[2..]))
}

/// OSC 52: clipboard contents.
pub fn clipboard(parser: &mut Parser) -> Option<Command> {
    let data = parser.captured();
    if data.is_empty() {
        parser.fail();
        return None;
    }
    if data[0] == b';' {
        return Some(Command::ClipboardContents {
            kind: b'c',
            data: data[1..].to_vec(),
        });
    }
    if data.len() < 2 || data[1] != b';' {
        parser.fail();
        return None;
    }
    Some(Command::ClipboardContents {
        kind: data[0],
        data: data[2..].to_vec(),
    })
}

/// OSC 4/5/10-19/104/105/110-119: color operations.
pub fn color(parser: &mut Parser, terminator: crate::Terminator) -> Option<Command> {
    let op = match parser.state() {
        State::N4 => ColorOperation::Osc4,
        State::N5 => ColorOperation::Osc5,
        State::N10 => ColorOperation::Osc10,
        State::N11 => ColorOperation::Osc11,
        State::N12 => ColorOperation::Osc12,
        State::N13 => ColorOperation::Osc13,
        State::N14 => ColorOperation::Osc14,
        State::N15 => ColorOperation::Osc15,
        State::N16 => ColorOperation::Osc16,
        State::N17 => ColorOperation::Osc17,
        State::N18 => ColorOperation::Osc18,
        State::N19 => ColorOperation::Osc19,
        State::N104 => ColorOperation::Osc104,
        State::N110 => ColorOperation::Osc110,
        State::N111 => ColorOperation::Osc111,
        State::N112 => ColorOperation::Osc112,
        State::N113 => ColorOperation::Osc113,
        State::N114 => ColorOperation::Osc114,
        State::N115 => ColorOperation::Osc115,
        State::N116 => ColorOperation::Osc116,
        State::N117 => ColorOperation::Osc117,
        State::N118 => ColorOperation::Osc118,
        State::N119 => ColorOperation::Osc119,
        _ => {
            parser.fail();
            return None;
        }
    };
    let body = parser.captured();
    let requests: Vec<ColorRequest> = color::parse_requests(op, body);
    Some(Command::ColorOperation {
        op,
        requests,
        terminator,
    })
}

/// OSC 21: kitty color protocol.
pub fn kitty_color(parser: &mut Parser) -> Option<Command> {
    let body = parser.captured();
    Some(Command::KittyColor {
        requests: color::parse_kitty_color_requests(body),
    })
}

/// OSC 9: iTerm2 notification or ConEmu extension.
pub fn osc9(parser: &mut Parser) -> Option<Command> {
    let data = parser.captured();

    // ConEmu-specific OSCs.
    if let Some(&first) = data.first() {
        match first {
            b'1' => {
                if data.len() < 2 {
                    return iterm2_notification(data);
                }
                match data[1] {
                    b';' => {
                        // OSC 9;1 sleep
                        let duration_ms = match std::str::from_utf8(&data[2..])
                            .ok()
                            .and_then(|value| value.parse::<u64>().ok())
                        {
                            Some(num) => num.min(10_000) as u16,
                            None => 100,
                        };
                        return Some(Command::ConemuSleep { duration_ms });
                    }
                    b'0' => {
                        // OSC 9;10 xterm keyboard/output emulation
                        if data.len() == 2 {
                            return Some(Command::ConemuXtermEmulation {
                                keyboard: Some(true),
                                output: Some(true),
                            });
                        }
                        if data.len() < 4 || data[2] != b';' {
                            return iterm2_notification(data);
                        }
                        return Some(match data[3] {
                            b'0' => Command::ConemuXtermEmulation {
                                keyboard: Some(false),
                                output: Some(false),
                            },
                            b'1' => Command::ConemuXtermEmulation {
                                keyboard: Some(true),
                                output: Some(true),
                            },
                            b'2' => Command::ConemuXtermEmulation {
                                keyboard: None,
                                output: Some(false),
                            },
                            b'3' => Command::ConemuXtermEmulation {
                                keyboard: None,
                                output: Some(true),
                            },
                            _ => return iterm2_notification(data),
                        });
                    }
                    b'1' => {
                        // OSC 9;11 comment
                        if data.len() < 3 || data[2] != b';' {
                            return iterm2_notification(data);
                        }
                        return Some(Command::ConemuComment(
                            String::from_utf8_lossy(&data[3..]).into_owned(),
                        ));
                    }
                    b'2' => {
                        // OSC 9;12 mark prompt start
                        return Some(Command::MarkPromptStart);
                    }
                    _ => return iterm2_notification(data),
                }
            }
            b'2' => {
                // OSC 9;2 show message box
                if data.len() < 2 || data[1] != b';' {
                    return iterm2_notification(data);
                }
                return Some(Command::ConemuShowMessageBox(
                    String::from_utf8_lossy(&data[2..]).into_owned(),
                ));
            }
            b'3' => {
                // OSC 9;3 change tab title
                if data.len() < 2 || data[1] != b';' {
                    return iterm2_notification(data);
                }
                if data.len() == 2 {
                    return Some(Command::ConemuChangeTabTitle(None));
                }
                return Some(Command::ConemuChangeTabTitle(Some(
                    String::from_utf8_lossy(&data[2..]).into_owned(),
                )));
            }
            b'4' => {
                // OSC 9;4 progress report
                if data.len() < 3 || data[1] != b';' {
                    return iterm2_notification(data);
                }
                let state = match data[2] {
                    b'0' => crate::osc::ProgressState::Remove,
                    b'1' => crate::osc::ProgressState::Set,
                    b'2' => crate::osc::ProgressState::Error,
                    b'3' => crate::osc::ProgressState::Indeterminate,
                    b'4' => crate::osc::ProgressState::Pause,
                    _ => return iterm2_notification(data),
                };
                let progress = match state {
                    crate::osc::ProgressState::Remove
                    | crate::osc::ProgressState::Indeterminate => None,
                    _ => {
                        if data.len() >= 4 && data[3] == b';' {
                            parse_u16(&data[4..]).map(|v| v.min(100) as u8)
                        } else {
                            None
                        }
                    }
                };
                return Some(Command::ConemuProgressReport { state, progress });
            }
            b'5' => return Some(Command::ConemuWaitInput),
            b'6' => {
                if data.len() < 2 || data[1] != b';' {
                    return iterm2_notification(data);
                }
                return Some(Command::ConemuGuimacro(
                    String::from_utf8_lossy(&data[2..]).into_owned(),
                ));
            }
            b'7' => {
                if data.len() < 2 || data[1] != b';' {
                    return iterm2_notification(data);
                }
                return Some(Command::ConemuRunProcess(
                    String::from_utf8_lossy(&data[2..]).into_owned(),
                ));
            }
            b'8' => {
                if data.len() < 2 || data[1] != b';' {
                    return iterm2_notification(data);
                }
                return Some(Command::ConemuOutputEnvironmentVariable(
                    String::from_utf8_lossy(&data[2..]).into_owned(),
                ));
            }
            b'9' => {
                // OSC 9;9 current working directory (ConEmu)
                if data.len() < 2 || data[1] != b';' {
                    return iterm2_notification(data);
                }
                return Some(Command::ReportPwd(
                    String::from_utf8_lossy(&data[2..]).into_owned(),
                ));
            }
            _ => {}
        }
    }

    iterm2_notification(data)
}

fn iterm2_notification(data: &[u8]) -> Option<Command> {
    Some(Command::ShowDesktopNotification {
        title: String::new(),
        body: String::from_utf8_lossy(data).into_owned(),
    })
}

fn parse_u16(data: &[u8]) -> Option<u16> {
    let s = std::str::from_utf8(data).ok()?;
    s.parse::<u16>().ok()
}

/// OSC 777: rxvt notify extension.
pub fn rxvt_extension(parser: &mut Parser) -> Option<Command> {
    let data = parser.captured();
    let Some(k) = data.iter().position(|&b| b == b';') else {
        parser.fail();
        return None;
    };
    let ext = &data[..k];
    if ext != b"notify" {
        parser.fail();
        return None;
    }
    let Some(t) = data[k + 1..].iter().position(|&b| b == b';') else {
        parser.fail();
        return None;
    };
    let t = k + 1 + t;
    Some(Command::ShowDesktopNotification {
        title: String::from_utf8_lossy(&data[k + 1..t]).into_owned(),
        body: String::from_utf8_lossy(&data[t + 1..]).into_owned(),
    })
}

/// Kitty "escape code safe UTF-8" (Ghostty `string_encoding.isSafeUtf8`):
/// valid UTF-8 with no C0, DEL, or C1 control characters.
pub fn is_safe_utf8(s: &[u8]) -> bool {
    let Ok(utf8) = std::str::from_utf8(s) else {
        return false;
    };
    !utf8
        .chars()
        .any(|c| (c as u32) <= 0x1f || c as u32 == 0x7f || (0x80..=0x9f).contains(&(c as u32)))
}

/// A malformed shell-escaped or percent-encoded string.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StringDecodeError;

/// Decode a string encoded like bash `printf %q` (Ghostty
/// `printfQDecode`). `$'...'` and `'...'` quoting is stripped, and the
/// recognized escapes `\ `, `\\`, `\"`, `\'`, `\$`, `\e`, `\n`, `\r`, `\t`,
/// `\v` are decoded. Any other escape is a decode error.
pub fn printf_q_decode(buf: &str, out: &mut Vec<u8>) -> Result<(), StringDecodeError> {
    let data: &str = if let Some(rest) = buf.strip_prefix("$'") {
        let Some(inner) = rest.strip_suffix('\'') else {
            return Err(StringDecodeError);
        };
        inner
    } else if let Some(rest) = buf.strip_prefix('\'') {
        let Some(inner) = rest.strip_suffix('\'') else {
            return Err(StringDecodeError);
        };
        inner
    } else {
        buf
    };

    let bytes = data.as_bytes();
    let mut src = 0;
    while src < bytes.len() {
        if bytes[src] != b'\\' {
            out.push(bytes[src]);
            src += 1;
            continue;
        }
        if src + 1 >= bytes.len() {
            return Err(StringDecodeError);
        }
        match bytes[src + 1] {
            b' ' | b'\\' | b'"' | b'\'' | b'$' => {
                out.push(bytes[src + 1]);
                src += 2;
            }
            b'e' => {
                out.push(0x1b);
                src += 2;
            }
            b'n' => {
                out.push(b'\n');
                src += 2;
            }
            b'r' => {
                out.push(b'\r');
                src += 2;
            }
            b't' => {
                out.push(b'\t');
                src += 2;
            }
            b'v' => {
                out.push(0x0b);
                src += 2;
            }
            _ => return Err(StringDecodeError),
        }
    }
    Ok(())
}

/// URL percent-decode into `out` (Ghostty `urlPercentDecode`).
pub fn url_percent_decode(buf: &[u8], out: &mut Vec<u8>) -> Result<(), StringDecodeError> {
    let mut src = 0;
    while src < buf.len() {
        if buf[src] != b'%' {
            out.push(buf[src]);
            src += 1;
            continue;
        }
        if src + 2 >= buf.len() {
            return Err(StringDecodeError);
        }
        let hi = hex(buf[src + 1]).ok_or(StringDecodeError)?;
        let lo = hex(buf[src + 2]).ok_or(StringDecodeError)?;
        out.push((hi << 4) | lo);
        src += 3;
    }
    Ok(())
}

fn hex(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

/// Kitty text sizing protocol (OSC 66): `1;key=value;...`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KittyTextSizing {
    pub version: u8,
    pub pairs: Vec<(String, String)>,
}

/// Kitty drag and drop protocol (OSC 72): `version:key=value:...`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KittyDnd {
    pub version: u8,
    pub pairs: Vec<(String, String)>,
}

/// Kitty clipboard protocol (OSC 5522): `kind;payload`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct KittyClipboard {
    pub kind: u8,
    pub data: Vec<u8>,
}

/// iTerm2 extension (OSC 1337): bounded key=value payload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Iterm2 {
    pub pairs: Vec<(String, String)>,
}

/// Context signal (OSC 3008): `key=value;...` pairs (UAPI spec).
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ContextSignal {
    pub pairs: Vec<(String, String)>,
}

fn parse_kv(payload: &[u8], separators: &[u8], max_pairs: usize) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    let mut start = 0;
    while start <= payload.len() && pairs.len() < max_pairs {
        let end = payload[start..]
            .iter()
            .position(|b| separators.contains(b))
            .map(|i| start + i)
            .unwrap_or(payload.len());
        let item = &payload[start..end];
        if let Some(eq) = item.iter().position(|&b| b == b'=') {
            pairs.push((
                String::from_utf8_lossy(&item[..eq]).into_owned(),
                String::from_utf8_lossy(&item[eq + 1..]).into_owned(),
            ));
        }
        if end == payload.len() {
            break;
        }
        start = end + 1;
    }
    pairs
}

fn parse_colon_kv(payload: &[u8], max_pairs: usize) -> Vec<(String, String)> {
    let mut pairs = Vec::new();
    let mut start = 0;
    while start < payload.len() && pairs.len() < max_pairs {
        let Some(eq_rel) = payload[start..].iter().position(|&b| b == b'=') else {
            break;
        };
        let eq = start + eq_rel;
        let mut end = payload.len();
        let mut cursor = eq + 1;
        while let Some(colon_rel) = payload[cursor..].iter().position(|&b| b == b':') {
            let colon = cursor + colon_rel;
            let candidate = &payload[colon + 1..];
            let key_len = candidate
                .iter()
                .take_while(|b| b.is_ascii_alphanumeric() || **b == b'_' || **b == b'-')
                .count();
            if key_len > 0 && candidate.get(key_len) == Some(&b'=') {
                end = colon;
                break;
            }
            cursor = colon + 1;
        }
        pairs.push((
            String::from_utf8_lossy(&payload[start..eq]).into_owned(),
            String::from_utf8_lossy(&payload[eq + 1..end]).into_owned(),
        ));
        if end == payload.len() {
            break;
        }
        start = end + 1;
    }
    pairs
}

/// OSC 66: kitty text sizing.
pub fn kitty_text_sizing(parser: &mut Parser) -> Option<Command> {
    let data = parser.captured();
    let version = data.first().copied().map(|v| v - b'0').unwrap_or(0);
    let rest = if data.first() == Some(&b';') {
        &data[1..]
    } else {
        data
    };
    let rest = if !rest.is_empty() && rest[0] == b';' {
        &rest[1..]
    } else {
        rest
    };
    Some(Command::KittyTextSizing(KittyTextSizing {
        version,
        pairs: parse_kv(rest, b";", 16),
    }))
}

/// OSC 72: kitty drag and drop.
pub fn kitty_dnd(parser: &mut Parser) -> Option<Command> {
    let data = parser.captured();
    let version = data.first().copied().map(|v| v - b'0').unwrap_or(0);
    let rest = if data.get(1) == Some(&b':') {
        &data[2..]
    } else {
        data.get(1..).unwrap_or_default()
    };
    Some(Command::KittyDnd(KittyDnd {
        version,
        pairs: parse_colon_kv(rest, 16),
    }))
}

/// OSC 5522: kitty clipboard protocol.
pub fn kitty_clipboard(parser: &mut Parser) -> Option<Command> {
    let data = parser.captured();
    if data.is_empty() {
        parser.fail();
        return None;
    }
    let kind = data[0];
    let rest = if data.len() > 1 && data[1] == b';' {
        &data[2..]
    } else {
        &data[1..]
    };
    Some(Command::KittyClipboard(KittyClipboard {
        kind,
        data: rest.to_vec(),
    }))
}

/// OSC 1337: iTerm2 extension.
pub fn iterm2(parser: &mut Parser) -> Option<Command> {
    let data = parser.captured();
    Some(Command::Iterm2(Iterm2 {
        pairs: parse_kv(data, b";", 16),
    }))
}

/// OSC 3008: hierarchical context signal.
pub fn context_signal(parser: &mut Parser) -> Option<Command> {
    let data = parser.captured();
    Some(Command::ContextSignal(ContextSignal {
        pairs: parse_kv(data, b";", 16),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(input: &[u8]) -> Option<Command> {
        let mut p = Parser::new();
        for &b in input {
            p.next(b);
        }
        p.end(None)
    }

    #[test]
    fn title_and_icon() {
        assert_eq!(
            parse(b"0;hello"),
            Some(Command::ChangeWindowTitle("hello".into()))
        );
        assert_eq!(
            parse("2;— ‐".as_bytes()),
            Some(Command::ChangeWindowTitle("— ‐".into()))
        );
        assert_eq!(
            parse(b"1;icon"),
            Some(Command::ChangeWindowIcon("icon".into()))
        );
    }

    #[test]
    fn pwd() {
        assert_eq!(
            parse(b"7;file:///tmp/example"),
            Some(Command::ReportPwd("file:///tmp/example".into()))
        );
        assert_eq!(parse(b"7;"), Some(Command::ReportPwd("".into())));
    }

    #[test]
    fn hyperlink_forms() {
        assert_eq!(
            parse(b"8;;http://example.com"),
            Some(Command::HyperlinkStart {
                id: None,
                uri: "http://example.com".into()
            })
        );
        assert_eq!(
            parse(b"8;id=foo;http://example.com"),
            Some(Command::HyperlinkStart {
                id: Some("foo".into()),
                uri: "http://example.com".into()
            })
        );
        // Empty id is treated as absent.
        assert_eq!(
            parse(b"8;id=;http://example.com"),
            Some(Command::HyperlinkStart {
                id: None,
                uri: "http://example.com".into()
            })
        );
        // Incomplete key stops key parsing but keeps the URI.
        assert_eq!(
            parse(b"8;id;http://example.com"),
            Some(Command::HyperlinkStart {
                id: None,
                uri: "http://example.com".into()
            })
        );
        assert_eq!(
            parse(b"8;=value:id=foo;http://example.com"),
            Some(Command::HyperlinkStart {
                id: Some("foo".into()),
                uri: "http://example.com".into()
            })
        );
        // Empty URI with an id is invalid.
        assert_eq!(parse(b"8;id=foo;"), None);
        assert_eq!(parse(b"8;;"), Some(Command::HyperlinkEnd));
    }

    #[test]
    fn semantic_prompt_actions() {
        use crate::semantic_prompt::Action as A;
        let cmd = parse(b"133;C").unwrap();
        assert_eq!(
            cmd,
            Command::SemanticPrompt(SemanticPrompt {
                action: A::EndInputStartOutput,
                options_unvalidated: "".into()
            })
        );
        let cmd = parse(b"133;Cextra");
        assert_eq!(cmd, None);
        let cmd = parse(b"133;C;aid=foo").unwrap();
        assert_eq!(
            cmd,
            Command::SemanticPrompt(SemanticPrompt {
                action: A::EndInputStartOutput,
                options_unvalidated: "aid=foo".into()
            })
        );
        assert_eq!(
            parse(b"133;L"),
            Some(Command::SemanticPrompt(SemanticPrompt {
                action: A::FreshLine,
                options_unvalidated: "".into()
            }))
        );
        assert_eq!(parse(b"133;L;x"), None);
        assert_eq!(parse(b"133;"), None);
    }

    #[test]
    fn osc9_notification_and_conemu() {
        assert_eq!(
            parse(b"9;Alert!"),
            Some(Command::ShowDesktopNotification {
                title: "".into(),
                body: "Alert!".into()
            })
        );
        assert_eq!(
            parse(b"9;1;250"),
            Some(Command::ConemuSleep { duration_ms: 250 })
        );
        assert_eq!(
            parse(b"9;1;999999"),
            Some(Command::ConemuSleep {
                duration_ms: 10_000
            })
        );
        assert_eq!(
            parse(b"9;1;"),
            Some(Command::ConemuSleep { duration_ms: 100 })
        );
        assert_eq!(
            parse(b"9;2;hello"),
            Some(Command::ConemuShowMessageBox("hello".into()))
        );
        assert_eq!(
            parse(b"9;3;tab"),
            Some(Command::ConemuChangeTabTitle(Some("tab".into())))
        );
        assert_eq!(parse(b"9;3;"), Some(Command::ConemuChangeTabTitle(None)));
        assert_eq!(
            parse(b"9;4;1;42"),
            Some(Command::ConemuProgressReport {
                state: crate::osc::ProgressState::Set,
                progress: Some(42)
            })
        );
        assert_eq!(
            parse(b"9;4;0"),
            Some(Command::ConemuProgressReport {
                state: crate::osc::ProgressState::Remove,
                progress: None
            })
        );
        assert_eq!(parse(b"9;5"), Some(Command::ConemuWaitInput));
        assert_eq!(parse(b"9;12"), Some(Command::MarkPromptStart));
        // A notification body that merely starts with a digit is iTerm2.
        assert_eq!(
            parse(b"9;hello world"),
            Some(Command::ShowDesktopNotification {
                title: "".into(),
                body: "hello world".into()
            })
        );
    }

    #[test]
    fn osc777_notify() {
        assert_eq!(
            parse(b"777;notify;Title;Body"),
            Some(Command::ShowDesktopNotification {
                title: "Title".into(),
                body: "Body".into()
            })
        );
        assert_eq!(parse(b"777;other;Title;Body"), None);
        assert_eq!(parse(b"777;notify;Title"), None);
    }

    #[test]
    fn clipboard_forms() {
        assert_eq!(
            parse(b"52;s;?"),
            Some(Command::ClipboardContents {
                kind: b's',
                data: b"?".to_vec()
            })
        );
        assert_eq!(
            parse(b"52;;?"),
            Some(Command::ClipboardContents {
                kind: b'c',
                data: b"?".to_vec()
            })
        );
        assert_eq!(
            parse(b"52;;"),
            Some(Command::ClipboardContents {
                kind: b'c',
                data: b"".to_vec()
            })
        );
        assert_eq!(parse(b"52;"), None);
    }

    #[test]
    fn color_commands() {
        let cmd = parse(b"4;0;rgb:ffff/0000/0000").unwrap();
        match cmd {
            Command::ColorOperation { op, requests, .. } => {
                assert_eq!(op, ColorOperation::Osc4);
                assert_eq!(
                    requests,
                    vec![ColorRequest::Set {
                        target: crate::color::ColorTarget::Palette(0),
                        color: crate::color::Rgb { r: 255, g: 0, b: 0 }
                    }]
                );
            }
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn safe_utf8() {
        assert!(is_safe_utf8("Hello world!".as_bytes()));
        assert!(is_safe_utf8("安全的ユニコード☀️".as_bytes()));
        assert!(!is_safe_utf8("No linebreaks\nallowed".as_bytes()));
        assert!(!is_safe_utf8("\x07no bells".as_bytes()));
        assert!(!is_safe_utf8(&[0x9f]));
    }

    #[test]
    fn printf_q() {
        let cases: &[(&str, &[u8])] = &[
            ("bobr\\ kurwa", b"bobr kurwa"),
            ("bobr\\nkurwa", b"bobr\nkurwa"),
            ("$'bobr kurwa'", b"bobr kurwa"),
            ("'bobr kurwa'", b"bobr kurwa"),
            ("plain", b"plain"),
        ];
        for (input, expected) in cases {
            let mut out = Vec::new();
            printf_q_decode(input, &mut out).unwrap();
            assert_eq!(out, *expected, "input {input:?}");
        }
        for bad in [
            "bobr\\dkurwa",
            "bobr kurwa\\",
            "$'bobr kurwa",
            "'bobr kurwa",
            "'",
            "$'",
        ] {
            let mut out = Vec::new();
            assert!(printf_q_decode(bad, &mut out).is_err(), "input {bad:?}");
        }
    }

    #[test]
    fn url_percent() {
        let mut out = Vec::new();
        url_percent_decode(b"bobr%20kurwa", &mut out).unwrap();
        assert_eq!(out, b"bobr kurwa");
        for bad in [
            b"%2".as_slice(),
            b"%".as_slice(),
            b"%%".as_slice(),
            b"%zz".as_slice(),
        ] {
            let mut out = Vec::new();
            assert!(url_percent_decode(bad, &mut out).is_err());
        }
    }

    #[test]
    fn kitty_clipboard_and_kv() {
        assert_eq!(
            parse(b"5522;c;aGVsbG8="),
            Some(Command::KittyClipboard(KittyClipboard {
                kind: b'c',
                data: b"aGVsbG8=".to_vec()
            }))
        );
        let cmd = parse(b"72;1:text=hello:url=file:///x").unwrap();
        match cmd {
            Command::KittyDnd(dnd) => {
                assert_eq!(dnd.version, 1);
                assert_eq!(
                    dnd.pairs,
                    vec![
                        ("text".to_string(), "hello".to_string()),
                        ("url".to_string(), "file:///x".to_string()),
                    ]
                );
            }
            other => panic!("unexpected {other:?}"),
        }
        let cmd = parse(b"3008;cwd=1").unwrap();
        match cmd {
            Command::ContextSignal(cs) => {
                assert_eq!(cs.pairs, vec![("cwd".to_string(), "1".to_string())]);
            }
            other => panic!("unexpected {other:?}"),
        }
    }
}

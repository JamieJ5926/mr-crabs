//! OSC 133 semantic prompt commands, ported from Ghostty
//! `src/terminal/osc/parsers/semantic_prompt.zig`.
//!
//! See https://gitlab.freedesktop.org/Per_Bothner/specifications/blob/master/proposals/semantic-prompts.md

/// The raw options string after the OSC 133 command, e.g. for
/// `133;A;aid=14;cl=line` the options are `aid=14;cl=line`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SemanticPrompt {
    pub action: Action,
    /// Raw unvalidated options (bounded by the OSC fixed capture).
    pub options_unvalidated: String,
}

impl SemanticPrompt {
    pub fn new(action: Action) -> Self {
        Self {
            action,
            options_unvalidated: String::new(),
        }
    }

    /// Read an option for this command. Returns `None` if unset or invalid.
    pub fn read_option(&self, option: Option) -> std::option::Option<OptionValue> {
        option.read(&self.options_unvalidated)
    }

    /// Write the decoded command line (if any) into `out`, following
    /// Ghostty: `cmdline` is `printf %q`-decoded, `cmdline_url` is URL
    /// percent-decoded. Returns an error on malformed encoding.
    pub fn write_command_line(
        &self,
        out: &mut Vec<u8>,
    ) -> Result<(), super::osc::parsers::StringDecodeError> {
        if let Some(OptionValue::Cmdline(command_line)) = self.read_option(Option::Cmdline) {
            super::osc::parsers::printf_q_decode(&command_line, out)?;
            return Ok(());
        }
        if let Some(OptionValue::CmdlineUrl(command_line)) = self.read_option(Option::CmdlineUrl) {
            super::osc::parsers::url_percent_decode(command_line.as_bytes(), out)?;
        }
        Ok(())
    }
}

/// A single semantic prompt action.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Action {
    /// 'L'
    FreshLine,
    /// 'A'
    FreshLineNewPrompt,
    /// 'N'
    NewCommand,
    /// 'P'
    PromptStart,
    /// 'B'
    EndPromptStartInput,
    /// 'I'
    EndPromptStartInputTerminateEol,
    /// 'C'
    EndInputStartOutput,
    /// 'D'
    EndCommand,
}

/// Click event coordinate modes (kitty `click_events`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClickEvents {
    Absolute,
    Relative,
}

/// The `cl` option: what kind of cursor key sequences the application
/// handles for click-to-move-cursor.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Click {
    Line,
    Multiple,
    ConservativeVertical,
    SmartVertical,
}

/// The `k` option: prompt kind.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PromptKind {
    Initial,
    Right,
    Continuation,
    Secondary,
}

impl PromptKind {
    fn init(c: u8) -> std::option::Option<Self> {
        match c {
            b'i' => Some(Self::Initial),
            b'r' => Some(Self::Right),
            b'c' => Some(Self::Continuation),
            b's' => Some(Self::Secondary),
            _ => None,
        }
    }
}

/// The `redraw` option (kitty extension, extended by Ghostty with `last`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Redraw {
    True,
    False,
    Last,
}

/// Typed values for each option.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OptionValue {
    Aid(String),
    Cl(Click),
    PromptKind(PromptKind),
    Err(String),
    Redraw(Redraw),
    SpecialKey(bool),
    ClickEvents(ClickEvents),
    Cmdline(String),
    CmdlineUrl(String),
    ExitCode(i32),
}

/// Recognized options.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Option {
    Aid,
    Cl,
    PromptKind,
    Err,
    Redraw,
    SpecialKey,
    ClickEvents,
    Cmdline,
    CmdlineUrl,
    ExitCode,
}

impl Option {
    fn key(self) -> &'static str {
        match self {
            Self::Aid => "aid",
            Self::Cl => "cl",
            Self::PromptKind => "k",
            Self::Err => "err",
            Self::Redraw => "redraw",
            Self::SpecialKey => "special_key",
            Self::ClickEvents => "click_events",
            Self::Cmdline => "cmdline",
            Self::CmdlineUrl => "cmdline_url",
            Self::ExitCode => unreachable!("exit_code is positional"),
        }
    }

    /// Read the option value from the raw options string. Malformed values
    /// return `None` (the OSC 133 spec says to ignore unknown/malformed
    /// options).
    pub fn read(self, raw: &str) -> std::option::Option<OptionValue> {
        let mut remaining = raw;
        while !remaining.is_empty() {
            let len = remaining.find(';').unwrap_or(remaining.len());
            let full = &remaining[..len];

            if self == Self::ExitCode {
                return full.parse::<i32>().ok().map(OptionValue::ExitCode);
            }

            let value = match full.find('=') {
                Some(eql) if &full[..eql] == self.key() => Some(&full[eql + 1..]),
                _ => {
                    if len < remaining.len() {
                        remaining = &remaining[len + 1..];
                        continue;
                    }
                    return None;
                }
            }?;

            return Some(match self {
                Self::Aid => OptionValue::Aid(value.to_owned()),
                Self::Cl => OptionValue::Cl(parse_click(value)?),
                Self::PromptKind => {
                    if value.len() == 1 {
                        OptionValue::PromptKind(PromptKind::init(value.as_bytes()[0])?)
                    } else {
                        return None;
                    }
                }
                Self::Err => OptionValue::Err(value.to_owned()),
                Self::Redraw => OptionValue::Redraw(match value {
                    "0" => Redraw::False,
                    "1" => Redraw::True,
                    "last" => Redraw::Last,
                    _ => return None,
                }),
                Self::ClickEvents => {
                    if value.len() == 1 {
                        OptionValue::ClickEvents(match value.as_bytes()[0] {
                            b'1' => ClickEvents::Absolute,
                            b'2' => ClickEvents::Relative,
                            _ => return None,
                        })
                    } else {
                        return None;
                    }
                }
                Self::SpecialKey => {
                    if value.len() == 1 {
                        OptionValue::SpecialKey(match value.as_bytes()[0] {
                            b'0' => false,
                            b'1' => true,
                            _ => return None,
                        })
                    } else {
                        return None;
                    }
                }
                Self::Cmdline => OptionValue::Cmdline(value.to_owned()),
                Self::CmdlineUrl => OptionValue::CmdlineUrl(value.to_owned()),
                Self::ExitCode => unreachable!(),
            });
        }
        None
    }
}

fn parse_click(value: &str) -> std::option::Option<Click> {
    if value.len() == 1 {
        match value.as_bytes()[0] {
            b'm' => return Some(Click::Multiple),
            b'v' => return Some(Click::ConservativeVertical),
            b'w' => return Some(Click::SmartVertical),
            _ => return None,
        }
    }
    if value == "line" {
        Some(Click::Line)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn options_parse() {
        let sp = SemanticPrompt {
            action: Action::EndInputStartOutput,
            options_unvalidated: "aid=foo;cl=line;cmdline=$'echo hi'".to_owned(),
        };
        assert_eq!(
            sp.read_option(Option::Aid),
            Some(OptionValue::Aid("foo".into()))
        );
        assert_eq!(
            sp.read_option(Option::Cl),
            Some(OptionValue::Cl(Click::Line))
        );
        assert_eq!(
            sp.read_option(Option::Cmdline),
            Some(OptionValue::Cmdline("$'echo hi'".into()))
        );

        let mut out = Vec::new();
        sp.write_command_line(&mut out).unwrap();
        assert_eq!(out, b"echo hi");
    }

    #[test]
    fn cmdline_url_decode() {
        let sp = SemanticPrompt {
            action: Action::EndInputStartOutput,
            options_unvalidated: "cmdline_url=echo%20bobr".to_owned(),
        };
        let mut out = Vec::new();
        sp.write_command_line(&mut out).unwrap();
        assert_eq!(out, b"echo bobr");
    }

    #[test]
    fn malformed_options_are_ignored() {
        let sp = SemanticPrompt {
            action: Action::PromptStart,
            options_unvalidated: "k=x;redraw=2;click_events=3;special_key=2".to_owned(),
        };
        assert_eq!(sp.read_option(Option::PromptKind), None);
        assert_eq!(sp.read_option(Option::Redraw), None);
        assert_eq!(sp.read_option(Option::ClickEvents), None);
        assert_eq!(sp.read_option(Option::SpecialKey), None);
        assert_eq!(sp.read_option(Option::Aid), None);
    }

    #[test]
    fn exit_code_is_positional() {
        let sp = SemanticPrompt {
            action: Action::EndCommand,
            options_unvalidated: "42".to_owned(),
        };
        assert_eq!(
            sp.read_option(Option::ExitCode),
            Some(OptionValue::ExitCode(42))
        );
        let sp = SemanticPrompt {
            action: Action::EndCommand,
            options_unvalidated: "not-a-number".to_owned(),
        };
        assert_eq!(sp.read_option(Option::ExitCode), None);
    }

    #[test]
    fn redraw_last() {
        let sp = SemanticPrompt {
            action: Action::FreshLineNewPrompt,
            options_unvalidated: "redraw=last".to_owned(),
        };
        assert_eq!(
            sp.read_option(Option::Redraw),
            Some(OptionValue::Redraw(Redraw::Last))
        );
    }
}

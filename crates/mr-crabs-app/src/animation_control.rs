use mr_crabs_config::TextAnimation;

use crate::settings::ANIMATION_OSC_KEY;

const OSC_PREFIX: &[u8] = b"\x1b]1337;";
pub const ANIMATION_STATE_REPLY_KEY: &str = "mr_crabs_animation_state";
pub const ANIMATION_SAVED_REPLY_KEY: &str = "mr_crabs_animation_saved";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OscTerminator {
    Bell,
    St,
}

impl OscTerminator {
    pub const fn bytes(self) -> &'static [u8] {
        match self {
            Self::Bell => b"\x07",
            Self::St => b"\x1b\\",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextAnimationChoice {
    None,
    Streaming,
    Typewriter,
}

impl TextAnimationChoice {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Streaming => "streaming",
            Self::Typewriter => "typewriter",
        }
    }

    pub const fn as_config(self) -> TextAnimation {
        match self {
            Self::None => TextAnimation::Disabled,
            Self::Streaming => TextAnimation::Streaming,
            Self::Typewriter => TextAnimation::Typewriter,
        }
    }

    pub const fn from_config(value: TextAnimation) -> Self {
        match value {
            TextAnimation::Disabled => Self::None,
            TextAnimation::Streaming => Self::Streaming,
            TextAnimation::Typewriter => Self::Typewriter,
        }
    }

    fn parse(value: &str) -> Result<Self, ProtocolError> {
        match value {
            "none" => Ok(Self::None),
            "streaming" => Ok(Self::Streaming),
            "typewriter" => Ok(Self::Typewriter),
            _ => Err(ProtocolError::InvalidValue),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AnimationSelection {
    pub text: TextAnimationChoice,
    pub cursor_trail: bool,
}

impl AnimationSelection {
    pub const fn none() -> Self {
        Self {
            text: TextAnimationChoice::None,
            cursor_trail: false,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OverlaySource {
    Override,
    Global,
}

impl OverlaySource {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Override => "o",
            Self::Global => "g",
        }
    }

    fn parse(value: &str) -> Result<Self, ProtocolError> {
        match value {
            "o" => Ok(Self::Override),
            "g" => Ok(Self::Global),
            _ => Err(ProtocolError::InvalidValue),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AnimationSnapshot {
    pub selection: AnimationSelection,
    pub text_source: OverlaySource,
    pub trail_source: OverlaySource,
    pub save_available: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AnimationTextSetting {
    None,
    Streaming,
    Typewriter,
    Inherit,
}

impl AnimationTextSetting {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Streaming => "streaming",
            Self::Typewriter => "typewriter",
            Self::Inherit => "inherit",
        }
    }

    fn parse(value: &str) -> Result<Self, ProtocolError> {
        match value {
            "none" => Ok(Self::None),
            "streaming" => Ok(Self::Streaming),
            "typewriter" => Ok(Self::Typewriter),
            "inherit" => Ok(Self::Inherit),
            _ => Err(ProtocolError::InvalidValue),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AnimationTrailSetting {
    Off,
    On,
    Inherit,
}

impl AnimationTrailSetting {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Off => "0",
            Self::On => "1",
            Self::Inherit => "inherit",
        }
    }

    fn parse(value: &str) -> Result<Self, ProtocolError> {
        match value {
            "0" => Ok(Self::Off),
            "1" => Ok(Self::On),
            "inherit" => Ok(Self::Inherit),
            _ => Err(ProtocolError::InvalidValue),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AnimationControl {
    /// An existing name from `ANIMATION_PRESETS`, applied as a live overlay.
    Preset(String),
    Query {
        terminator: OscTerminator,
    },
    State {
        text: AnimationTextSetting,
        trail: AnimationTrailSetting,
    },
    Save {
        text: TextAnimationChoice,
        trail: bool,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SaveErrorCode {
    Invalid,
    NoPath,
    Io,
    Json,
    Reload,
}

impl SaveErrorCode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Invalid => "invalid",
            Self::NoPath => "no-path",
            Self::Io => "io",
            Self::Json => "json",
            Self::Reload => "reload",
        }
    }

    fn parse(value: &str) -> Result<Self, ProtocolError> {
        match value {
            "invalid" => Ok(Self::Invalid),
            "no-path" => Ok(Self::NoPath),
            "io" => Ok(Self::Io),
            "json" => Ok(Self::Json),
            "reload" => Ok(Self::Reload),
            _ => Err(ProtocolError::InvalidValue),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AnimationReply {
    Snapshot(AnimationSnapshot),
    Saved(Result<(), SaveErrorCode>),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProtocolError {
    InvalidPrefix,
    InvalidUtf8,
    InvalidValue,
    InvalidField,
    DuplicateField,
    MissingField,
    InvalidPreset,
    TrailingBytes,
}

fn encode_osc(value: &str, terminator: OscTerminator) -> Vec<u8> {
    let mut bytes =
        Vec::with_capacity(OSC_PREFIX.len() + ANIMATION_OSC_KEY.len() + value.len() + 2);
    bytes.extend_from_slice(OSC_PREFIX);
    bytes.extend_from_slice(ANIMATION_OSC_KEY.as_bytes());
    bytes.push(b'=');
    bytes.extend_from_slice(value.as_bytes());
    bytes.extend_from_slice(terminator.bytes());
    bytes
}

pub fn selection_control(selection: AnimationSelection) -> AnimationControl {
    AnimationControl::State {
        text: match selection.text {
            TextAnimationChoice::None => AnimationTextSetting::None,
            TextAnimationChoice::Streaming => AnimationTextSetting::Streaming,
            TextAnimationChoice::Typewriter => AnimationTextSetting::Typewriter,
        },
        trail: if selection.cursor_trail {
            AnimationTrailSetting::On
        } else {
            AnimationTrailSetting::Off
        },
    }
}

pub fn snapshot_control(snapshot: AnimationSnapshot) -> AnimationControl {
    AnimationControl::State {
        text: match snapshot.text_source {
            OverlaySource::Override => match snapshot.selection.text {
                TextAnimationChoice::None => AnimationTextSetting::None,
                TextAnimationChoice::Streaming => AnimationTextSetting::Streaming,
                TextAnimationChoice::Typewriter => AnimationTextSetting::Typewriter,
            },
            OverlaySource::Global => AnimationTextSetting::Inherit,
        },
        trail: match snapshot.trail_source {
            OverlaySource::Override => {
                if snapshot.selection.cursor_trail {
                    AnimationTrailSetting::On
                } else {
                    AnimationTrailSetting::Off
                }
            }
            OverlaySource::Global => AnimationTrailSetting::Inherit,
        },
    }
}

impl AnimationControl {
    pub fn parse_payload(payload: &str) -> Result<Self, ProtocolError> {
        if payload == "?" {
            return Ok(Self::Query {
                terminator: OscTerminator::Bell,
            });
        }
        if let Some(name) = payload.strip_prefix("state;") {
            let fields = strict_fields(name, &["text", "trail"])?;
            return Ok(Self::State {
                text: AnimationTextSetting::parse(fields[0].1)?,
                trail: AnimationTrailSetting::parse(fields[1].1)?,
            });
        }
        if payload.starts_with("save;") {
            return Err(ProtocolError::InvalidValue);
        }
        if payload.is_empty() || payload.contains(';') {
            return Err(ProtocolError::InvalidValue);
        }
        if crate::settings::animation_preset(payload).is_none() {
            return Err(ProtocolError::InvalidPreset);
        }
        Ok(Self::Preset(payload.to_owned()))
    }

    pub fn parse_osc(bytes: &[u8]) -> Result<Self, ProtocolError> {
        let (payload, terminator) = parse_frame(bytes)?;
        let mut command = Self::parse_payload(payload)?;
        if let Self::Query {
            terminator: queried,
        } = &mut command
        {
            *queried = terminator;
        }
        Ok(command)
    }

    pub fn encode(&self) -> Vec<u8> {
        self.encode_with_terminator(OscTerminator::Bell)
    }

    pub fn encode_with_terminator(&self, terminator: OscTerminator) -> Vec<u8> {
        let value = match self {
            Self::Preset(name) => name.clone(),
            Self::Query { .. } => "?".to_owned(),
            Self::State { text, trail } => {
                format!("state;text={};trail={}", text.as_str(), trail.as_str())
            }
            Self::Save { text, trail } => format!(
                "save;text={};trail={}",
                text.as_str(),
                if *trail { "1" } else { "0" }
            ),
        };
        encode_osc(&value, terminator)
    }
}

impl AnimationReply {
    pub fn parse_payload(payload: &str) -> Result<Self, ProtocolError> {
        if let Some(rest) = payload.strip_prefix("v1;") {
            let fields = strict_fields(rest, &["text", "trail", "text_src", "trail_src", "save"])?;
            let text = TextAnimationChoice::parse(fields[0].1)?;
            let trail = match fields[1].1 {
                "0" => false,
                "1" => true,
                _ => return Err(ProtocolError::InvalidValue),
            };
            let save_available = match fields[4].1 {
                "0" => false,
                "1" => true,
                _ => return Err(ProtocolError::InvalidValue),
            };
            return Ok(Self::Snapshot(AnimationSnapshot {
                selection: AnimationSelection {
                    text,
                    cursor_trail: trail,
                },
                text_source: OverlaySource::parse(fields[2].1)?,
                trail_source: OverlaySource::parse(fields[3].1)?,
                save_available,
            }));
        }
        if payload == "ok" {
            return Ok(Self::Saved(Ok(())));
        }
        if let Some(code) = payload.strip_prefix("err;code=") {
            return Ok(Self::Saved(Err(SaveErrorCode::parse(code)?)));
        }
        Err(ProtocolError::InvalidValue)
    }

    pub fn encode(&self) -> Vec<u8> {
        self.encode_with_terminator(OscTerminator::Bell)
    }

    pub fn encode_with_terminator(&self, terminator: OscTerminator) -> Vec<u8> {
        let (key, value) = match self {
            Self::Snapshot(snapshot) => (
                ANIMATION_STATE_REPLY_KEY,
                format!(
                    "v1;text={};trail={};text_src={};trail_src={};save={}",
                    snapshot.selection.text.as_str(),
                    if snapshot.selection.cursor_trail {
                        "1"
                    } else {
                        "0"
                    },
                    snapshot.text_source.as_str(),
                    snapshot.trail_source.as_str(),
                    if snapshot.save_available { "1" } else { "0" },
                ),
            ),
            Self::Saved(result) => (
                ANIMATION_SAVED_REPLY_KEY,
                match result {
                    Ok(()) => "ok".to_owned(),
                    Err(code) => format!("err;code={}", code.as_str()),
                },
            ),
        };
        let mut bytes = Vec::with_capacity(OSC_PREFIX.len() + key.len() + value.len() + 2);
        bytes.extend_from_slice(OSC_PREFIX);
        bytes.extend_from_slice(key.as_bytes());
        bytes.push(b'=');
        bytes.extend_from_slice(value.as_bytes());
        bytes.extend_from_slice(terminator.bytes());
        bytes
    }
}

fn strict_fields<'a>(
    input: &'a str,
    expected: &[&str],
) -> Result<Vec<(&'a str, &'a str)>, ProtocolError> {
    let fields: Vec<_> = input
        .split(';')
        .map(|field| field.split_once('=').ok_or(ProtocolError::InvalidField))
        .collect::<Result<_, _>>()?;
    if fields.len() != expected.len() {
        return Err(ProtocolError::MissingField);
    }
    for (index, (key, _)) in fields.iter().enumerate() {
        if *key != expected[index] {
            if expected[..index].contains(key) {
                return Err(ProtocolError::DuplicateField);
            }
            return Err(ProtocolError::InvalidField);
        }
    }
    Ok(fields)
}

fn parse_frame(bytes: &[u8]) -> Result<(&str, OscTerminator), ProtocolError> {
    if !bytes.starts_with(OSC_PREFIX) {
        return Err(ProtocolError::InvalidPrefix);
    }
    let body = &bytes[OSC_PREFIX.len()..];
    if body.ends_with(b"\x07") {
        std::str::from_utf8(&body[..body.len() - 1])
            .map(|payload| (payload, OscTerminator::Bell))
            .map_err(|_| ProtocolError::InvalidUtf8)
    } else if body.ends_with(b"\x1b\\") {
        std::str::from_utf8(&body[..body.len() - 2])
            .map(|payload| (payload, OscTerminator::St))
            .map_err(|_| ProtocolError::InvalidUtf8)
    } else {
        Err(ProtocolError::TrailingBytes)
    }
}

pub const SCANNER_MAX_CANDIDATE: usize = 4096;
pub const SCANNER_MAX_REPLIES: usize = 32;
pub const SCANNER_MAX_KEY_INPUT: usize = 4096;

#[derive(Clone, Debug, Default)]
pub struct ReplyScanner {
    state: ScanState,
    candidate: Vec<u8>,
    key_input: Vec<u8>,
    replies: Vec<AnimationReply>,
    overflowed: bool,
}

#[derive(Clone, Debug, Default)]
enum ScanState {
    #[default]
    Ground,
    Esc,
    Osc,
    OscEsc,
}
impl ReplyScanner {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn overflowed(&self) -> bool {
        self.overflowed
    }

    pub fn push(&mut self, bytes: &[u8]) {
        if self.overflowed {
            return;
        }
        for &byte in bytes {
            if self.overflowed {
                return;
            }
            self.push_byte(byte);
        }
    }

    pub fn drain_replies(&mut self) -> Vec<AnimationReply> {
        std::mem::take(&mut self.replies)
    }

    pub fn drain_key_input(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.key_input)
    }

    fn push_byte(&mut self, byte: u8) {
        if self.overflowed {
            return;
        }
        match self.state {
            ScanState::Ground => match byte {
                0x1b => self.state = ScanState::Esc,
                0x9d => {
                    self.candidate.clear();
                    self.candidate.push(byte);
                    self.state = ScanState::Osc;
                }
                _ => {
                    if self.key_input.len() >= SCANNER_MAX_KEY_INPUT {
                        self.mark_overflow();
                        return;
                    }
                    self.key_input.push(byte);
                }
            },
            ScanState::Esc => {
                if byte == b']' {
                    self.candidate.clear();
                    self.candidate.extend_from_slice(b"\x1b]");
                    self.state = ScanState::Osc;
                } else {
                    if self.key_input.len() >= SCANNER_MAX_KEY_INPUT {
                        self.mark_overflow();
                        return;
                    }
                    self.key_input.push(0x1b);
                    self.state = ScanState::Ground;
                    self.push_byte(byte);
                }
            }
            ScanState::Osc => {
                if self.candidate.len() >= SCANNER_MAX_CANDIDATE {
                    self.mark_overflow();
                    return;
                }
                self.candidate.push(byte);
                match byte {
                    0x07 => self.finish_candidate(OscTerminator::Bell),
                    0x1b => self.state = ScanState::OscEsc,
                    _ => {}
                }
            }
            ScanState::OscEsc => {
                if self.candidate.len() >= SCANNER_MAX_CANDIDATE {
                    self.mark_overflow();
                    return;
                }
                self.candidate.push(byte);
                if byte == b'\\' {
                    self.finish_candidate(OscTerminator::St);
                } else {
                    self.state = ScanState::Osc;
                }
            }
        }
    }

    fn mark_overflow(&mut self) {
        self.overflowed = true;
        self.state = ScanState::Ground;
        self.candidate.clear();
        self.key_input.clear();
        self.replies.clear();
    }

    fn finish_candidate(&mut self, terminator: OscTerminator) {
        let candidate = std::mem::take(&mut self.candidate);
        self.state = ScanState::Ground;
        let Some((key, value)) = candidate_payload(&candidate, terminator) else {
            self.append_key_candidate(&candidate);
            return;
        };
        let parsed = match key {
            ANIMATION_STATE_REPLY_KEY | ANIMATION_SAVED_REPLY_KEY => {
                AnimationReply::parse_payload(value).ok()
            }
            _ => None,
        };
        if let Some(reply) = parsed {
            if self.replies.len() >= SCANNER_MAX_REPLIES {
                self.mark_overflow();
                return;
            }
            self.replies.push(reply);
        } else {
            self.append_key_candidate(&candidate);
        }
    }

    fn append_key_candidate(&mut self, candidate: &[u8]) {
        if candidate.len() > SCANNER_MAX_KEY_INPUT.saturating_sub(self.key_input.len()) {
            self.mark_overflow();
            return;
        }
        self.key_input.extend_from_slice(candidate);
    }
}

fn candidate_payload<'a>(
    candidate: &'a [u8],
    terminator: OscTerminator,
) -> Option<(&'a str, &'a str)> {
    let prefix_len = if candidate.starts_with(b"\x1b]1337;") {
        7
    } else if candidate.starts_with(b"\x9d1337;") {
        6
    } else {
        return None;
    };
    let end = match terminator {
        OscTerminator::Bell => candidate.len().checked_sub(1)?,
        OscTerminator::St => candidate.len().checked_sub(2)?,
    };
    let body = std::str::from_utf8(&candidate[prefix_len..end]).ok()?;
    let (key, value) = body.split_once('=')?;
    if key == ANIMATION_STATE_REPLY_KEY || key == ANIMATION_SAVED_REPLY_KEY {
        Some((key, value))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn osc(value: &str, term: OscTerminator) -> Vec<u8> {
        let mut bytes = Vec::from(OSC_PREFIX);
        bytes.extend_from_slice(value.as_bytes());
        bytes.extend_from_slice(term.bytes());
        bytes
    }

    #[test]
    fn control_encoding_exact_bytes() {
        assert_eq!(
            AnimationControl::Query {
                terminator: OscTerminator::Bell
            }
            .encode(),
            b"\x1b]1337;mr_crabs_animation=?\x07"
        );
        assert_eq!(
            AnimationControl::Preset("typewriter".into()).encode_with_terminator(OscTerminator::St),
            b"\x1b]1337;mr_crabs_animation=typewriter\x1b\\"
        );
        assert_eq!(
            AnimationControl::State {
                text: AnimationTextSetting::Inherit,
                trail: AnimationTrailSetting::On
            }
            .encode(),
            b"\x1b]1337;mr_crabs_animation=state;text=inherit;trail=1\x07"
        );
        assert_eq!(
            AnimationControl::Save {
                text: TextAnimationChoice::Streaming,
                trail: false
            }
            .encode(),
            b"\x1b]1337;mr_crabs_animation=save;text=streaming;trail=0\x07"
        );
        assert_eq!(
            AnimationControl::parse_payload("save;text=streaming;trail=0"),
            Err(ProtocolError::InvalidValue)
        );
    }

    #[test]
    fn strict_invalid_values_are_rejected() {
        assert_eq!(
            AnimationControl::parse_payload("state;text=none;text=streaming"),
            Err(ProtocolError::DuplicateField)
        );
        assert_eq!(
            AnimationControl::parse_payload("save;text=none;trail=inherit"),
            Err(ProtocolError::InvalidValue)
        );
        assert_eq!(
            AnimationReply::parse_payload("v1;text=none;trail=2;text_src=o;trail_src=g;save=1"),
            Err(ProtocolError::InvalidValue)
        );
    }

    #[test]
    fn replies_encode_and_parse_with_bel_and_st() {
        let snapshot = AnimationReply::Snapshot(AnimationSnapshot {
            selection: AnimationSelection {
                text: TextAnimationChoice::Typewriter,
                cursor_trail: true,
            },
            text_source: OverlaySource::Override,
            trail_source: OverlaySource::Global,
            save_available: true,
        });
        for term in [OscTerminator::Bell, OscTerminator::St] {
            let encoded = snapshot.encode_with_terminator(term);
            let start = b"\x1b]1337;mr_crabs_animation_state=".len();
            let end = encoded.len() - term.bytes().len();
            assert_eq!(
                AnimationReply::parse_payload(std::str::from_utf8(&encoded[start..end]).unwrap()),
                Ok(snapshot)
            );
            let mut scanner = ReplyScanner::new();
            scanner.push(&snapshot.encode_with_terminator(term));
            assert_eq!(scanner.drain_replies(), vec![snapshot]);
        }
        assert_eq!(
            AnimationReply::parse_payload("ok"),
            Ok(AnimationReply::Saved(Ok(())))
        );
        assert_eq!(
            AnimationReply::parse_payload("err;code=no-path"),
            Ok(AnimationReply::Saved(Err(SaveErrorCode::NoPath)))
        );
    }

    #[test]
    fn scanner_handles_mixed_split_input_and_preserves_keys() {
        let reply = osc("mr_crabs_animation_saved=ok", OscTerminator::St);
        let mut scanner = ReplyScanner::new();
        scanner.push(b"a\x1b[Dx");
        scanner.push(&reply[..9]);
        scanner.push(&reply[9..]);
        scanner.push(b"\n");
        assert_eq!(scanner.drain_replies(), vec![AnimationReply::Saved(Ok(()))]);
        assert_eq!(scanner.drain_key_input(), b"a\x1b[Dx\n");
    }

    #[test]
    fn scanner_does_not_echo_or_collide_with_similar_keys() {
        let mut scanner = ReplyScanner::new();
        scanner.push(b"\x1b]1337;not_mr_crabs_animation_state=v1;text=none\x07");
        scanner.push(b"\x1b]1337;mr_crabs_animation_state_extra=v1;text=none\x07");
        assert!(scanner.drain_replies().is_empty());
        assert!(!scanner.drain_key_input().is_empty());
    }

    #[test]
    fn scanner_overflow_stops_accepting_and_yields_no_capture() {
        let mut scanner = ReplyScanner::new();
        scanner.push(&vec![b'x'; SCANNER_MAX_KEY_INPUT]);
        scanner.push(b"y");
        assert!(scanner.overflowed());
        assert!(scanner.drain_replies().is_empty());
        assert!(scanner.drain_key_input().is_empty());
        scanner.push(osc("mr_crabs_animation_saved=ok", OscTerminator::Bell).as_slice());
        assert!(scanner.drain_replies().is_empty());

        let mut osc_scanner = ReplyScanner::new();
        osc_scanner.push(b"\x1b]");
        osc_scanner.push(&vec![b'a'; SCANNER_MAX_CANDIDATE]);
        assert!(osc_scanner.overflowed());
        assert!(osc_scanner.drain_replies().is_empty());
    }

    fn saved_reply_osc() -> Vec<u8> {
        osc("mr_crabs_animation_saved=ok", OscTerminator::Bell)
    }

    #[test]
    fn scanner_overflows_when_completed_foreign_candidate_exceeds_key_cap() {
        let mut scanner = ReplyScanner::new();
        scanner.push(&vec![b'x'; SCANNER_MAX_KEY_INPUT - 8]);
        let mut foreign = Vec::from(OSC_PREFIX);
        foreign.extend_from_slice(b"foreign=payload-too-large");
        foreign.extend_from_slice(OscTerminator::Bell.bytes());
        scanner.push(&foreign);
        assert!(scanner.overflowed());
        assert!(scanner.drain_replies().is_empty());
        assert!(scanner.drain_key_input().is_empty());
    }

    #[test]
    fn scanner_overflows_when_malformed_candidate_exceeds_key_cap() {
        let mut scanner = ReplyScanner::new();
        scanner.push(&vec![b'x'; SCANNER_MAX_KEY_INPUT - 4]);
        scanner.push(b"\x1b]1337;mr_crabs_animation_state=not-a-reply\x07");
        assert!(scanner.overflowed());
        assert!(scanner.drain_replies().is_empty());
        assert!(scanner.drain_key_input().is_empty());
    }

    #[test]
    fn scanner_overflows_on_the_33rd_reply() {
        let mut scanner = ReplyScanner::new();
        let reply = saved_reply_osc();
        for _ in 0..SCANNER_MAX_REPLIES {
            scanner.push(&reply);
            assert!(!scanner.overflowed());
        }
        assert_eq!(scanner.replies.len(), SCANNER_MAX_REPLIES);
        scanner.push(&reply);
        assert!(scanner.overflowed());
        assert!(scanner.drain_replies().is_empty());
        assert!(scanner.drain_key_input().is_empty());
    }
}

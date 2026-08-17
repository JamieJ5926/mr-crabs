//! Glyph Protocol request parsing.
//!
//! Faithful port of `src/terminal/apc/glyph/request.zig`. The parser is
//! deliberately strict: a request must be at least `verb;` (a bare single
//! verb is normalized by appending `;`), and register requests must carry a
//! payload separator. Options are decoded lazily from the raw option string,
//! so malformed options report `None` while duplicate options use the last
//! value, exactly like the oracle.

use crate::glyph::MAX_PAYLOAD_SIZE;

/// Errors from request parsing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RequestError {
    InvalidFormat,
    /// The command buffer exceeded its byte bound.
    OutOfMemory,
}

/// A parsed glyph APC request with the verb classified eagerly.
#[derive(Clone, Debug, PartialEq)]
pub enum Request {
    Support,
    Query(Query),
    Register(Register),
    Clear(Clear),
}

impl Request {
    /// Parse an owned raw command payload (the bytes after `25a1;`).
    pub fn parse(raw: Vec<u8>) -> Result<Request, RequestError> {
        if raw.len() < 2 || raw[1] != b';' {
            return Err(RequestError::InvalidFormat);
        }
        match raw[0] {
            b's' => Ok(Request::Support),
            b'q' => Ok(Request::Query(Query::new(raw))),
            b'r' => Ok(Request::Register(
                Register::new(raw).ok_or(RequestError::InvalidFormat)?,
            )),
            b'c' => Ok(Request::Clear(Clear::new(raw))),
            _ => Err(RequestError::InvalidFormat),
        }
    }
}

/// Buffered parser for the bytes after the `25a1;` prefix.
pub struct RequestParser {
    data: Vec<u8>,
    max_bytes: usize,
}

impl RequestParser {
    pub fn new(max_bytes: usize) -> Self {
        Self {
            data: Vec::new(),
            max_bytes,
        }
    }

    /// Append one byte of APC payload.
    pub fn feed(&mut self, byte: u8) -> Result<(), RequestError> {
        if self.data.len() >= self.max_bytes {
            return Err(RequestError::OutOfMemory);
        }
        self.data.push(byte);
        Ok(())
    }

    /// Append a slice of APC payload bytes.
    pub fn feed_slice(&mut self, bytes: &[u8]) -> Result<(), RequestError> {
        if self.data.len().saturating_add(bytes.len()) > self.max_bytes {
            return Err(RequestError::OutOfMemory);
        }
        self.data.extend_from_slice(bytes);
        Ok(())
    }

    /// Finish parsing and return an owned request.
    pub fn complete(mut self) -> Result<Request, RequestError> {
        // Normalize bare single-byte verbs like `s` into `s;`.
        if self.data.len() == 1 {
            self.data.push(b';');
        }
        Request::parse(self.data)
    }
}

/// Find the last occurrence of `key=value` in a `;`-delimited option list.
fn option_value<'a>(raw: &'a [u8], key: &[u8]) -> Option<&'a [u8]> {
    let mut result: Option<&[u8]> = None;
    let mut remaining = raw;
    while !remaining.is_empty() {
        let end = remaining
            .iter()
            .position(|&b| b == b';')
            .unwrap_or(remaining.len());
        let full = &remaining[..end];
        if let Some(eq) = full.iter().position(|&b| b == b'=') {
            if full[..eq] == *key {
                result = Some(&full[eq + 1..]);
            }
        }
        if end == remaining.len() {
            break;
        }
        remaining = &remaining[end + 1..];
    }
    result
}

fn parse_hex_cp(value: &[u8]) -> Option<u32> {
    let s = std::str::from_utf8(value).ok()?;
    let cp = u32::from_str_radix(s, 16).ok()?;
    if cp > 0x10FFFF {
        return None;
    }
    Some(cp)
}

fn parse_u32(value: &[u8]) -> Option<u32> {
    std::str::from_utf8(value).ok()?.parse().ok()
}

fn parse_fraction(value: &[u8]) -> Option<f64> {
    let v: f64 = std::str::from_utf8(value).ok()?.parse().ok()?;
    if !(0.0..=1.0).contains(&v) {
        return None;
    }
    Some(v)
}

/// Codepoint coverage query (`q`).
#[derive(Clone, Debug, PartialEq)]
pub struct Query {
    raw: Vec<u8>,
}

impl Query {
    pub fn new(raw: Vec<u8>) -> Self {
        Self { raw }
    }

    /// The queried codepoint, or None when absent or malformed.
    pub fn get(&self, option: QueryOption) -> Option<u32> {
        match option {
            QueryOption::Cp => option_value(&self.raw[2..], b"cp").and_then(parse_hex_cp),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueryOption {
    Cp,
}

/// Glyph payload formats named by the protocol.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Format {
    Glyf,
    Colrv0,
    Colrv1,
}

impl Format {
    pub fn init(value: &[u8]) -> Option<Format> {
        match value {
            b"glyf" => Some(Format::Glyf),
            b"colrv0" => Some(Format::Colrv0),
            b"colrv1" => Some(Format::Colrv1),
            _ => None,
        }
    }
}

/// Register reply verbosity.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reply {
    None,
    All,
    Failures,
}

impl Reply {
    pub fn init(value: &[u8]) -> Option<Reply> {
        match value {
            b"0" => Some(Reply::None),
            b"1" => Some(Reply::All),
            b"2" => Some(Reply::Failures),
            _ => None,
        }
    }
}

/// Unicode cell width.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Width {
    Narrow,
    Wide,
}

impl Width {
    pub fn init(value: &[u8]) -> Option<Width> {
        match value {
            b"1" => Some(Width::Narrow),
            b"2" => Some(Width::Wide),
            _ => None,
        }
    }
}

/// Glyph scale policy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Size {
    Height,
    Advance,
    Contain,
    Cover,
    Stretch,
}

impl Size {
    pub fn init(value: &[u8]) -> Option<Size> {
        match value {
            b"height" => Some(Size::Height),
            b"advance" => Some(Size::Advance),
            b"contain" => Some(Size::Contain),
            b"cover" => Some(Size::Cover),
            b"stretch" => Some(Size::Stretch),
            _ => None,
        }
    }
}

/// Glyph placement within the render span.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Align {
    pub horizontal: Horizontal,
    pub vertical: Vertical,
}

impl Default for Align {
    fn default() -> Self {
        Self {
            horizontal: Horizontal::Center,
            vertical: Vertical::Center,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Horizontal {
    Start,
    Center,
    End,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Vertical {
    Start,
    Center,
    End,
    Baseline,
}

impl Align {
    pub fn init(value: &[u8]) -> Option<Align> {
        let s = std::str::from_utf8(value).ok()?;
        let mut it = s.split(',');
        let horizontal = match it.next()? {
            "start" => Horizontal::Start,
            "center" => Horizontal::Center,
            "end" => Horizontal::End,
            _ => return None,
        };
        let vertical = match it.next()? {
            "start" => Vertical::Start,
            "center" => Vertical::Center,
            "end" => Vertical::End,
            "baseline" => Vertical::Baseline,
            _ => return None,
        };
        if it.next().is_some() {
            return None;
        }
        Some(Align {
            horizontal,
            vertical,
        })
    }
}

/// Fractional insets from the render span edges.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pad {
    pub top: f64,
    pub right: f64,
    pub bottom: f64,
    pub left: f64,
}

impl Default for Pad {
    fn default() -> Self {
        Self {
            top: 0.0,
            right: 0.0,
            bottom: 0.0,
            left: 0.0,
        }
    }
}

impl Pad {
    pub fn init(value: &[u8]) -> Option<Pad> {
        let s = std::str::from_utf8(value).ok()?;
        let mut it = s.split(',');
        let top = parse_fraction(it.next()?.as_bytes())?;
        let right = parse_fraction(it.next()?.as_bytes())?;
        let bottom = parse_fraction(it.next()?.as_bytes())?;
        let left = parse_fraction(it.next()?.as_bytes())?;
        if it.next().is_some() {
            return None;
        }
        // Degenerate padding is treated as no padding (spec §8.5.2).
        if left + right >= 1.0 || top + bottom >= 1.0 {
            return Some(Pad::default());
        }
        Some(Pad {
            top,
            right,
            bottom,
            left,
        })
    }
}

/// Glyph registration request (`r`).
#[derive(Clone, Debug, PartialEq)]
pub struct Register {
    raw: Vec<u8>,
    payload_idx: usize,
}

impl Register {
    pub fn new(raw: Vec<u8>) -> Option<Register> {
        if raw.len() < 2 || raw[0] != b'r' || raw[1] != b';' {
            return None;
        }
        let payload_idx = raw.iter().rposition(|&b| b == b';')?;
        if payload_idx <= 1 {
            return None;
        }
        Some(Register { raw, payload_idx })
    }

    fn raw_options(&self) -> &[u8] {
        &self.raw[2..self.payload_idx]
    }

    /// The base64 payload (raw, unvalidated).
    pub fn payload(&self) -> &[u8] {
        &self.raw[self.payload_idx + 1..]
    }

    /// The raw option segment (for tests and diagnostics).
    pub fn raw(&self) -> &[u8] {
        &self.raw
    }

    /// Decode a register option, applying protocol defaults when absent.
    pub fn get(&self, option: RegisterOption) -> Option<RegisterValue> {
        let raw = self.raw_options();
        if option_value(raw, option.key()).is_none() {
            return match option {
                RegisterOption::Cp => None,
                RegisterOption::Fmt => Some(RegisterValue::Fmt(Format::Glyf)),
                RegisterOption::Reply => Some(RegisterValue::Reply(Reply::All)),
                RegisterOption::Upm => Some(RegisterValue::Upm(1000)),
                // `aw`/`lh` default to the resolved `upm` value (which itself
                // defaults to 1000), preserving the variant type.
                RegisterOption::Aw => {
                    Some(RegisterValue::Aw(match self.get(RegisterOption::Upm) {
                        Some(RegisterValue::Upm(v)) => v,
                        _ => 1000,
                    }))
                }
                RegisterOption::Lh => {
                    Some(RegisterValue::Lh(match self.get(RegisterOption::Upm) {
                        Some(RegisterValue::Upm(v)) => v,
                        _ => 1000,
                    }))
                }
                RegisterOption::Width => Some(RegisterValue::Width(Width::Narrow)),
                RegisterOption::Size => Some(RegisterValue::Size(Size::Height)),
                RegisterOption::Align => Some(RegisterValue::Align(Align::default())),
                RegisterOption::Pad => Some(RegisterValue::Pad(Pad::default())),
            };
        }
        let value = option_value(raw, option.key())?;
        Some(match option {
            RegisterOption::Cp => RegisterValue::Cp(parse_hex_cp(value)?),
            RegisterOption::Fmt => RegisterValue::Fmt(Format::init(value)?),
            RegisterOption::Reply => RegisterValue::Reply(Reply::init(value).unwrap_or(Reply::All)),
            RegisterOption::Upm => RegisterValue::Upm(parse_u32(value)?),
            RegisterOption::Aw => RegisterValue::Aw(parse_u32(value)?),
            RegisterOption::Lh => RegisterValue::Lh(parse_u32(value)?),
            RegisterOption::Width => RegisterValue::Width(Width::init(value)?),
            RegisterOption::Size => RegisterValue::Size(Size::init(value)?),
            RegisterOption::Align => RegisterValue::Align(Align::init(value)?),
            RegisterOption::Pad => RegisterValue::Pad(Pad::init(value)?),
        })
    }
}

/// Register options.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RegisterOption {
    Cp,
    Fmt,
    Reply,
    Upm,
    Aw,
    Lh,
    Width,
    Size,
    Align,
    Pad,
}

impl RegisterOption {
    pub fn key(self) -> &'static [u8] {
        match self {
            RegisterOption::Cp => b"cp",
            RegisterOption::Fmt => b"fmt",
            RegisterOption::Reply => b"reply",
            RegisterOption::Upm => b"upm",
            RegisterOption::Aw => b"aw",
            RegisterOption::Lh => b"lh",
            RegisterOption::Width => b"width",
            RegisterOption::Size => b"size",
            RegisterOption::Align => b"align",
            RegisterOption::Pad => b"pad",
        }
    }
}

/// Decoded register option values.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RegisterValue {
    Cp(u32),
    Fmt(Format),
    Reply(Reply),
    Upm(u32),
    Aw(u32),
    Lh(u32),
    Width(Width),
    Size(Size),
    Align(Align),
    Pad(Pad),
}

/// Registration clear request (`c`).
#[derive(Clone, Debug, PartialEq)]
pub struct Clear {
    raw: Vec<u8>,
}

impl Clear {
    pub fn new(raw: Vec<u8>) -> Self {
        Self { raw }
    }

    fn raw_options(&self) -> &[u8] {
        &self.raw[2..]
    }

    /// The target codepoint, or None when absent or malformed.
    pub fn get(&self, option: ClearOption) -> Option<u32> {
        match option {
            ClearOption::Cp => option_value(self.raw_options(), b"cp").and_then(parse_hex_cp),
        }
    }

    /// Whether an option was provided, even if malformed.
    pub fn has(&self, option: ClearOption) -> bool {
        match option {
            ClearOption::Cp => option_value(self.raw_options(), b"cp").is_some(),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClearOption {
    Cp,
}

/// Bound exported for callers: the payload size limit applies to decoded
/// glyph data.
pub const PAYLOAD_SIZE_LIMIT: usize = MAX_PAYLOAD_SIZE;

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(data: &str) -> Result<Request, RequestError> {
        let mut p = RequestParser::new(1024 * 1024);
        for b in data.bytes() {
            p.feed(b)?;
        }
        p.complete()
    }

    #[test]
    fn support_command() {
        assert!(matches!(parse("s").unwrap(), Request::Support));
    }

    #[test]
    fn query_command() {
        let req = parse("q;cp=E0A0").unwrap();
        match req {
            Request::Query(q) => assert_eq!(q.get(QueryOption::Cp), Some(0xE0A0)),
            _ => panic!("expected query"),
        }
    }

    #[test]
    fn register_command_with_payload() {
        let req = parse("r;cp=e0a0;fmt=glyf;upm=1000;reply=2;QQ==").unwrap();
        match req {
            Request::Register(r) => {
                assert_eq!(r.get(RegisterOption::Cp), Some(RegisterValue::Cp(0xE0A0)));
                assert_eq!(
                    r.get(RegisterOption::Fmt),
                    Some(RegisterValue::Fmt(Format::Glyf))
                );
                assert_eq!(r.get(RegisterOption::Upm), Some(RegisterValue::Upm(1000)));
                assert_eq!(
                    r.get(RegisterOption::Reply),
                    Some(RegisterValue::Reply(Reply::Failures))
                );
                assert_eq!(r.payload(), b"QQ==");
            }
            _ => panic!("expected register"),
        }
    }

    #[test]
    fn register_defaults() {
        let req = parse("r;cp=e0a0;QQ==").unwrap();
        match req {
            Request::Register(r) => {
                assert_eq!(
                    r.get(RegisterOption::Fmt),
                    Some(RegisterValue::Fmt(Format::Glyf))
                );
                assert_eq!(r.get(RegisterOption::Upm), Some(RegisterValue::Upm(1000)));
                assert_eq!(r.get(RegisterOption::Aw), Some(RegisterValue::Aw(1000)));
                assert_eq!(r.get(RegisterOption::Lh), Some(RegisterValue::Lh(1000)));
                assert_eq!(
                    r.get(RegisterOption::Width),
                    Some(RegisterValue::Width(Width::Narrow))
                );
                assert_eq!(
                    r.get(RegisterOption::Size),
                    Some(RegisterValue::Size(Size::Height))
                );
                assert_eq!(
                    r.get(RegisterOption::Reply),
                    Some(RegisterValue::Reply(Reply::All))
                );
            }
            _ => panic!("expected register"),
        }
    }

    #[test]
    fn register_aw_lh_default_to_upm() {
        let req = parse("r;cp=e0a0;upm=2048;QQ==").unwrap();
        match req {
            Request::Register(r) => {
                assert_eq!(r.get(RegisterOption::Aw), Some(RegisterValue::Aw(2048)));
                assert_eq!(r.get(RegisterOption::Lh), Some(RegisterValue::Lh(2048)));
            }
            _ => panic!("expected register"),
        }
    }

    #[test]
    fn register_sizing_and_placement_options() {
        let req = parse("r;cp=e0a0;upm=2048;aw=1024;lh=1536;width=2;size=contain;align=end,baseline;pad=0.1,0.2,0.3,0.4;QQ==").unwrap();
        match req {
            Request::Register(r) => {
                assert_eq!(
                    r.get(RegisterOption::Width),
                    Some(RegisterValue::Width(Width::Wide))
                );
                assert_eq!(
                    r.get(RegisterOption::Size),
                    Some(RegisterValue::Size(Size::Contain))
                );
                assert_eq!(
                    r.get(RegisterOption::Align),
                    Some(RegisterValue::Align(Align {
                        horizontal: Horizontal::End,
                        vertical: Vertical::Baseline
                    }))
                );
                assert_eq!(
                    r.get(RegisterOption::Pad),
                    Some(RegisterValue::Pad(Pad {
                        top: 0.1,
                        right: 0.2,
                        bottom: 0.3,
                        left: 0.4
                    }))
                );
            }
            _ => panic!("expected register"),
        }
    }

    #[test]
    fn register_invalid_options_yield_none() {
        let req =
            parse("r;cp=e0a0;width=3;size=invalid;align=center,middle;pad=0,1.2,0,0;QQ==").unwrap();
        match req {
            Request::Register(r) => {
                assert_eq!(r.get(RegisterOption::Width), None);
                assert_eq!(r.get(RegisterOption::Size), None);
                assert_eq!(r.get(RegisterOption::Align), None);
                assert_eq!(r.get(RegisterOption::Pad), None);
            }
            _ => panic!("expected register"),
        }
    }

    #[test]
    fn register_degenerate_padding_defaults_to_zero() {
        let req = parse("r;cp=e0a0;pad=0.4,0.2,0.6,0.1;QQ==").unwrap();
        match req {
            Request::Register(r) => {
                assert_eq!(
                    r.get(RegisterOption::Pad),
                    Some(RegisterValue::Pad(Pad::default()))
                );
            }
            _ => panic!("expected register"),
        }
    }

    #[test]
    fn register_invalid_reply_falls_back_to_all() {
        let req = parse("r;cp=e0a0;reply=9;QQ==").unwrap();
        match req {
            Request::Register(r) => {
                assert_eq!(
                    r.get(RegisterOption::Reply),
                    Some(RegisterValue::Reply(Reply::All))
                );
            }
            _ => panic!("expected register"),
        }
    }

    #[test]
    fn register_duplicate_options_use_last() {
        let req = parse("r;cp=e0a0;reply=1;reply=2;QQ==").unwrap();
        match req {
            Request::Register(r) => {
                assert_eq!(
                    r.get(RegisterOption::Reply),
                    Some(RegisterValue::Reply(Reply::Failures))
                );
            }
            _ => panic!("expected register"),
        }
    }

    #[test]
    fn register_requires_payload_separator() {
        for data in ["r", "r;cp=e0a0", "r;foo"] {
            assert_eq!(parse(data), Err(RequestError::InvalidFormat));
        }
    }

    #[test]
    fn clear_tracks_malformed_cp_presence() {
        for data in ["c;cp=zz", "c;cp=", "c;cp=200000"] {
            let req = parse(data).unwrap();
            match req {
                Request::Clear(c) => {
                    assert!(c.has(ClearOption::Cp));
                    assert_eq!(c.get(ClearOption::Cp), None);
                }
                _ => panic!("expected clear"),
            }
        }
        let req = parse("c;cp=e0a0").unwrap();
        match req {
            Request::Clear(c) => {
                assert!(c.has(ClearOption::Cp));
                assert_eq!(c.get(ClearOption::Cp), Some(0xE0A0));
            }
            _ => panic!("expected clear"),
        }
    }

    #[test]
    fn invalid_command() {
        assert_eq!(parse("x"), Err(RequestError::InvalidFormat));
        assert_eq!(parse(""), Err(RequestError::InvalidFormat));
    }

    #[test]
    fn parser_bounds() {
        let mut p = RequestParser::new(4);
        p.feed_slice(b"abcd").unwrap();
        assert_eq!(p.feed(b'e'), Err(RequestError::OutOfMemory));
    }
}

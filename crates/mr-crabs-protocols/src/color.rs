//! OSC color operations, ported from Ghostty `src/terminal/osc/parsers/color.zig`
//! and `src/terminal/color.zig` (RGB parsing/encoding).
//!
//! X11 color names come from the vendored `rgb.txt` data file (X11/MIT
//! licensed), byte-identical to the oracle's
//! `src/terminal/res/rgb.txt`.

use crate::Terminator;
use std::fmt::Write as _;

/// An 8-bit-per-channel RGB color (Ghostty `color.RGB`).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct Rgb {
    pub r: u8,
    pub g: u8,
    pub b: u8,
}

impl Rgb {
    /// `rgb:rrrr/gggg/bbbb` with 16-bit-scaled channels (Ghostty
    /// `encodeRgb16`).
    pub fn encode_rgb16(&self, out: &mut String) {
        let _ = write!(
            out,
            "rgb:{:04x}/{:04x}/{:04x}",
            u16::from(self.r) * 257,
            u16::from(self.g) * 257,
            u16::from(self.b) * 257
        );
    }

    /// `rgb:rr/gg/bb` (Ghostty `encodeRgb8`).
    pub fn encode_rgb8(&self, out: &mut String) {
        let _ = write!(out, "rgb:{:02x}/{:02x}/{:02x}", self.r, self.g, self.b);
    }

    /// Parse a color specification (Ghostty `RGB.parse`):
    ///
    /// 1. `rgb:<red>/<green>/<blue>` with 1-4 hex digits per channel;
    /// 2. `rgbi:<red>/<green>/<blue>` with float intensities in [0, 1];
    /// 3. `#rgb`, `#rrggbb`, `#rrrgggbbb`, `#rrrrggggbbbb`, and the same
    ///    forms without `#` (3 or 6 hex digits);
    /// 4. X11 color names (case-insensitive).
    pub fn parse(value: &str) -> Result<Rgb, InvalidColor> {
        let input = value.trim_matches([' ', '\t']);
        if input.is_empty() {
            return Err(InvalidColor);
        }

        if input.starts_with('#') {
            return match input.len() {
                4 => Ok(Rgb {
                    r: Rgb::from_hex(&input[1..2])?,
                    g: Rgb::from_hex(&input[2..3])?,
                    b: Rgb::from_hex(&input[3..4])?,
                }),
                7 => Ok(Rgb {
                    r: Rgb::from_hex(&input[1..3])?,
                    g: Rgb::from_hex(&input[3..5])?,
                    b: Rgb::from_hex(&input[5..7])?,
                }),
                10 => Ok(Rgb {
                    r: Rgb::from_hex(&input[1..4])?,
                    g: Rgb::from_hex(&input[4..7])?,
                    b: Rgb::from_hex(&input[7..10])?,
                }),
                13 => Ok(Rgb {
                    r: Rgb::from_hex(&input[1..5])?,
                    g: Rgb::from_hex(&input[5..9])?,
                    b: Rgb::from_hex(&input[9..13])?,
                }),
                _ => Err(InvalidColor),
            };
        }

        if let Some(rgb) = x11_lookup(input) {
            return Ok(rgb);
        }

        match input.len() {
            3 => {
                return Ok(Rgb {
                    r: Rgb::from_hex(&input[0..1])?,
                    g: Rgb::from_hex(&input[1..2])?,
                    b: Rgb::from_hex(&input[2..3])?,
                });
            }
            6 => {
                return Ok(Rgb {
                    r: Rgb::from_hex(&input[0..2])?,
                    g: Rgb::from_hex(&input[2..4])?,
                    b: Rgb::from_hex(&input[4..6])?,
                });
            }
            _ => {}
        }

        if input.len() < 8 || &input[0..3] != "rgb" {
            return Err(InvalidColor);
        }
        let mut i = 3;
        let use_intensity = if input.as_bytes()[i] == b'i' {
            i += 1;
            true
        } else {
            false
        };
        if input.as_bytes()[i] != b':' {
            return Err(InvalidColor);
        }
        i += 1;

        let next_channel = |input: &str, i: &mut usize| -> Result<u8, InvalidColor> {
            let rest = &input[*i..];
            let end = rest.find('/').ok_or(InvalidColor)?;
            let slice = &rest[..end];
            *i += slice.len() + 1;
            if use_intensity {
                from_intensity(slice)
            } else {
                Rgb::from_hex(slice)
            }
        };

        let r = next_channel(input, &mut i)?;
        let g = next_channel(input, &mut i)?;
        let b = if use_intensity {
            from_intensity(&input[i..])?
        } else {
            Rgb::from_hex(&input[i..])?
        };
        Ok(Rgb { r, g, b })
    }

    /// Parse 1-4 hex digits scaled to 8 bits (Ghostty `RGB.fromHex`).
    fn from_hex(value: &str) -> Result<u8, InvalidColor> {
        if value.is_empty() || value.len() > 4 {
            return Err(InvalidColor);
        }
        let color = u16::from_str_radix(value, 16).map_err(|_| InvalidColor)?;
        let divisor: u16 = match value.len() {
            1 => 0x0F,
            2 => 0xFF,
            3 => 0x0FFF,
            4 => 0xFFFF,
            _ => unreachable!(),
        };
        Ok((u32::from(color) * 255 / u32::from(divisor)) as u8)
    }
}

/// Error returned by [`Rgb::parse`].
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidColor;

fn from_intensity(value: &str) -> Result<u8, InvalidColor> {
    let i: f64 = value.parse().map_err(|_| InvalidColor)?;
    if !(0.0..=1.0).contains(&i) {
        return Err(InvalidColor);
    }
    Ok((i * 255.0) as u8)
}

/// Look up an X11 color name (case-insensitive). The table is sorted by
/// lowercased name, so this is a plain binary search.
pub fn x11_lookup(name: &str) -> Option<Rgb> {
    let table = X11_TABLE.as_slice();
    let name_lower = name.to_ascii_lowercase();
    table
        .binary_search_by(|(n, _)| {
            n.as_bytes()
                .iter()
                .map(u8::to_ascii_lowercase)
                .cmp(name_lower.as_bytes().iter().copied())
        })
        .ok()
        .map(|i| table[i].1)
}

/// X11 rgb.txt entries as sorted `(name, rgb)` pairs.
///
/// Generated lazily from the vendored rgb.txt data file
/// (`terminfo_res/rgb.txt`), which is byte-identical to the Ghostty oracle's
/// `src/terminal/res/rgb.txt` (X11/MIT license).
static X11_TABLE: std::sync::LazyLock<Vec<(&'static str, Rgb)>> = std::sync::LazyLock::new(|| {
    const DATA: &str = include_str!("terminfo_res/rgb.txt");
    let mut entries: Vec<(&'static str, Rgb)> = Vec::new();
    for line in DATA.lines() {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            continue;
        }
        let r: u8 = line[0..3].trim().parse().unwrap();
        let g: u8 = line[4..7].trim().parse().unwrap();
        let b: u8 = line[8..11].trim().parse().unwrap();
        let name = line[12..].trim();
        entries.push((name, Rgb { r, g, b }));
    }
    entries.sort_by_key(|a| a.0.to_ascii_lowercase());
    entries
});

/// Which OSC operation is being processed (Ghostty `color.Operation`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ColorOperation {
    Osc4,
    Osc5,
    Osc10,
    Osc11,
    Osc12,
    Osc13,
    Osc14,
    Osc15,
    Osc16,
    Osc17,
    Osc18,
    Osc19,
    Osc104,
    Osc105,
    Osc110,
    Osc111,
    Osc112,
    Osc113,
    Osc114,
    Osc115,
    Osc116,
    Osc117,
    Osc118,
    Osc119,
}

/// A single operation related to the terminal color palette (Ghostty
/// `color.Request`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ColorRequest {
    Set { target: ColorTarget, color: Rgb },
    Query(ColorTarget),
    Reset(ColorTarget),
    ResetPalette,
    ResetSpecial,
}

/// The color being operated on (Ghostty `color.Target`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ColorTarget {
    Palette(u8),
    Special(SpecialColor),
    Dynamic(DynamicColor),
}

/// Special colors indexable through OSC 4/5 (Ghostty `color.Special`:
/// bold/underline/blink/reverse/italic — indices 5-7 are unassigned and yield
/// None, matching Ghostty `std.enums.fromInt(..., u3)` failure).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SpecialColor {
    Bold = 0,
    Underline = 1,
    Blink = 2,
    Reverse = 3,
    Italic = 4,
}

impl SpecialColor {
    fn from_index(index: u8) -> Option<Self> {
        Some(match index {
            0 => Self::Bold,
            1 => Self::Underline,
            2 => Self::Blink,
            3 => Self::Reverse,
            4 => Self::Italic,
            _ => return None,
        })
    }
}

/// Dynamic colors operated by OSC 10-19 (Ghostty `color.Dynamic`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DynamicColor {
    Foreground = 10,
    Background = 11,
    Cursor = 12,
    PointerForeground = 13,
    PointerBackground = 14,
    TektronixForeground = 15,
    TektronixBackground = 16,
    HighlightBackground = 17,
    TektronixCursor = 18,
    HighlightForeground = 19,
}

impl DynamicColor {
    pub fn next(self) -> Option<Self> {
        Some(match self {
            Self::Foreground => Self::Background,
            Self::Background => Self::Cursor,
            Self::Cursor => Self::PointerForeground,
            Self::PointerForeground => Self::PointerBackground,
            Self::PointerBackground => Self::TektronixForeground,
            Self::TektronixForeground => Self::TektronixBackground,
            Self::TektronixBackground => Self::HighlightBackground,
            Self::HighlightBackground => Self::TektronixCursor,
            Self::TektronixCursor => Self::HighlightForeground,
            Self::HighlightForeground => return None,
        })
    }
}

/// Parse the color operation body (everything after the `;`), matching
/// Ghostty `parseColor`. Malformed input yields the requests accumulated so
/// far (xterm behavior).
pub fn parse_requests(op: ColorOperation, body: &[u8]) -> Vec<ColorRequest> {
    let body = String::from_utf8_lossy(body);
    let mut it = body.split(';');
    match op {
        ColorOperation::Osc4 | ColorOperation::Osc5 => parse_get_set_ansi(op, &mut it),
        ColorOperation::Osc104 | ColorOperation::Osc105 => parse_reset_ansi(op, &mut it),
        ColorOperation::Osc10
        | ColorOperation::Osc11
        | ColorOperation::Osc12
        | ColorOperation::Osc13
        | ColorOperation::Osc14
        | ColorOperation::Osc15
        | ColorOperation::Osc16
        | ColorOperation::Osc17
        | ColorOperation::Osc18
        | ColorOperation::Osc19 => parse_get_set_dynamic(dynamic_for(op), &mut it),
        ColorOperation::Osc110
        | ColorOperation::Osc111
        | ColorOperation::Osc112
        | ColorOperation::Osc113
        | ColorOperation::Osc114
        | ColorOperation::Osc115
        | ColorOperation::Osc116
        | ColorOperation::Osc117
        | ColorOperation::Osc118
        | ColorOperation::Osc119 => parse_reset_dynamic(dynamic_for(op), &mut it),
    }
}

fn dynamic_for(op: ColorOperation) -> DynamicColor {
    match op {
        ColorOperation::Osc10 | ColorOperation::Osc110 => DynamicColor::Foreground,
        ColorOperation::Osc11 | ColorOperation::Osc111 => DynamicColor::Background,
        ColorOperation::Osc12 | ColorOperation::Osc112 => DynamicColor::Cursor,
        ColorOperation::Osc13 | ColorOperation::Osc113 => DynamicColor::PointerForeground,
        ColorOperation::Osc14 | ColorOperation::Osc114 => DynamicColor::PointerBackground,
        ColorOperation::Osc15 | ColorOperation::Osc115 => DynamicColor::TektronixForeground,
        ColorOperation::Osc16 | ColorOperation::Osc116 => DynamicColor::TektronixBackground,
        ColorOperation::Osc17 | ColorOperation::Osc117 => DynamicColor::HighlightBackground,
        ColorOperation::Osc18 | ColorOperation::Osc118 => DynamicColor::TektronixCursor,
        ColorOperation::Osc19 | ColorOperation::Osc119 => DynamicColor::HighlightForeground,
        _ => unreachable!(),
    }
}

fn parse_get_set_ansi(op: ColorOperation, it: &mut std::str::Split<'_, char>) -> Vec<ColorRequest> {
    let mut result = Vec::new();
    loop {
        let color_str = match it.next() {
            Some(v) => v,
            None => return result,
        };
        let spec_str = match it.next() {
            Some(v) => v,
            None => return result,
        };
        let color: u16 = match color_str.parse() {
            Ok(v) => v,
            Err(_) => return result,
        };
        let target = match op {
            ColorOperation::Osc5 => {
                match u8::try_from(color).ok().and_then(SpecialColor::from_index) {
                    Some(s) => ColorTarget::Special(s),
                    None => return result,
                }
            }
            ColorOperation::Osc4 => {
                if let Ok(idx) = u8::try_from(color) {
                    ColorTarget::Palette(idx)
                } else if color >= 256 {
                    match SpecialColor::from_index((color - 256) as u8) {
                        Some(s) => ColorTarget::Special(s),
                        None => return result,
                    }
                } else {
                    return result;
                }
            }
            _ => unreachable!(),
        };
        if spec_str == "?" {
            result.push(ColorRequest::Query(target));
            continue;
        }
        match Rgb::parse(spec_str) {
            Ok(rgb) => result.push(ColorRequest::Set { target, color: rgb }),
            Err(_) => return result,
        }
    }
}

fn parse_reset_ansi(op: ColorOperation, it: &mut std::str::Split<'_, char>) -> Vec<ColorRequest> {
    let mut result = Vec::new();
    loop {
        let color_str = match it.next() {
            None => {
                if result.is_empty() {
                    result.push(match op {
                        ColorOperation::Osc104 => ColorRequest::ResetPalette,
                        ColorOperation::Osc105 => ColorRequest::ResetSpecial,
                        _ => unreachable!(),
                    });
                }
                return result;
            }
            Some(v) => v,
        };
        if color_str.is_empty() {
            continue;
        }
        let color: u16 = match color_str.parse() {
            Ok(v) => v,
            Err(_) => continue,
        };
        let target = match op {
            ColorOperation::Osc105 => {
                match u8::try_from(color).ok().and_then(SpecialColor::from_index) {
                    Some(s) => ColorTarget::Special(s),
                    None => continue,
                }
            }
            ColorOperation::Osc104 => {
                if let Ok(idx) = u8::try_from(color) {
                    ColorTarget::Palette(idx)
                } else if color >= 256 {
                    match SpecialColor::from_index((color - 256) as u8) {
                        Some(s) => ColorTarget::Special(s),
                        None => continue,
                    }
                } else {
                    continue;
                }
            }
            _ => unreachable!(),
        };
        result.push(ColorRequest::Reset(target));
    }
}

fn parse_get_set_dynamic(
    start: DynamicColor,
    it: &mut std::str::Split<'_, char>,
) -> Vec<ColorRequest> {
    let mut result = Vec::new();
    let mut color = start;
    loop {
        let spec_str = match it.next() {
            Some(v) => v,
            None => return result,
        };
        if spec_str == "?" {
            result.push(ColorRequest::Query(ColorTarget::Dynamic(color)));
        } else {
            match Rgb::parse(spec_str) {
                Ok(rgb) => result.push(ColorRequest::Set {
                    target: ColorTarget::Dynamic(color),
                    color: rgb,
                }),
                Err(_) => return result,
            }
        }
        match color.next() {
            Some(next) => color = next,
            None => return result,
        }
    }
}

fn parse_reset_dynamic(
    color: DynamicColor,
    it: &mut std::str::Split<'_, char>,
) -> Vec<ColorRequest> {
    let mut result = Vec::new();
    if it.next().is_some() {
        return result;
    }
    result.push(ColorRequest::Reset(ColorTarget::Dynamic(color)));
    result
}

/// Encode an xterm color report response (Ghostty `writeXtermColorReport`).
/// Palette targets use `OSC 4;{i};{rgb16}{terminator}`, dynamic foreground/
/// background/cursor use their OSC number; everything else writes nothing.
pub fn write_xterm_color_report(
    target: ColorTarget,
    color: Rgb,
    terminator: Terminator,
    out: &mut String,
) {
    match target {
        ColorTarget::Palette(i) => {
            let _ = write!(out, "\x1b]4;{i};");
            color.encode_rgb16(out);
            out.push_str(std::str::from_utf8(terminator.bytes()).unwrap());
        }
        ColorTarget::Dynamic(dynamic) => match dynamic {
            DynamicColor::Foreground | DynamicColor::Background | DynamicColor::Cursor => {
                let _ = write!(out, "\x1b]{};", dynamic as u8);
                color.encode_rgb16(out);
                out.push_str(std::str::from_utf8(terminator.bytes()).unwrap());
            }
            _ => {}
        },
        ColorTarget::Special(_) => {}
    }
}

/// A kitty color protocol request (OSC 21), Ghostty `kitty_color.OSC` item.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KittyColorRequest {
    Set { key: KittyColorKey, color: Rgb },
    Reset(KittyColorKey),
    Query(KittyColorKey),
}

/// Kitty color keys: palette index or special color name.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KittyColorKey {
    Palette(u8),
    Foreground,
    Background,
    Cursor,
    SelectionForeground,
    SelectionBackground,
}

impl KittyColorKey {
    pub fn parse(raw: &str) -> Option<Self> {
        if let Ok(idx) = raw.parse::<u8>() {
            return Some(Self::Palette(idx));
        }
        Some(match raw {
            "foreground" => Self::Foreground,
            "background" => Self::Background,
            "cursor" => Self::Cursor,
            "selection_foreground" => Self::SelectionForeground,
            "selection_background" => Self::SelectionBackground,
            _ => return None,
        })
    }
}

/// Parse an OSC 21 kitty color body into typed requests. The body is a list
/// of `key=color` pairs for set, `key` for reset, `key=?` for query; `q`
/// queries the full palette. Unparseable items are skipped (kitty ignores
/// malformed entries).
pub fn parse_kitty_color_requests(body: &[u8]) -> Vec<KittyColorRequest> {
    let body = String::from_utf8_lossy(body);
    let mut result = Vec::new();
    for item in body.split(';') {
        if item.is_empty() || item == "q" {
            continue;
        }
        if let Some((key_raw, value)) = item.split_once('=') {
            let Some(key) = KittyColorKey::parse(key_raw) else {
                continue;
            };
            if value == "?" {
                result.push(KittyColorRequest::Query(key));
            } else if let Ok(rgb) = Rgb::parse(value) {
                result.push(KittyColorRequest::Set { key, color: rgb });
            }
        } else if let Some(key) = KittyColorKey::parse(item) {
            result.push(KittyColorRequest::Reset(key));
        }
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rgb_parse_forms() {
        assert_eq!(
            Rgb::parse("#fff").unwrap(),
            Rgb {
                r: 255,
                g: 255,
                b: 255
            }
        );
        assert_eq!(Rgb::parse("#ff0000").unwrap(), Rgb { r: 255, g: 0, b: 0 });
        assert_eq!(
            Rgb::parse("#ffff00000000").unwrap(),
            Rgb { r: 255, g: 0, b: 0 }
        );
        assert_eq!(Rgb::parse("ff0000").unwrap(), Rgb { r: 255, g: 0, b: 0 });
        assert_eq!(Rgb::parse("f00").unwrap(), Rgb { r: 255, g: 0, b: 0 });
        assert_eq!(
            Rgb::parse("rgb:ff/00/00").unwrap(),
            Rgb { r: 255, g: 0, b: 0 }
        );
        assert_eq!(
            Rgb::parse("rgb:ffff/0000/0000").unwrap(),
            Rgb { r: 255, g: 0, b: 0 }
        );
        assert_eq!(
            Rgb::parse("rgbi:1.0/0.0/0.5").unwrap(),
            Rgb {
                r: 255,
                g: 0,
                b: 127
            }
        );
        assert!(Rgb::parse("").is_err());
        assert!(Rgb::parse("#ggg").is_err());
        assert!(Rgb::parse("rgbi:2/0/0").is_err());
        assert!(Rgb::parse("rgb:ff/00").is_err());
    }

    #[test]
    fn rgb_x11_names() {
        assert_eq!(
            Rgb::parse("white").unwrap(),
            Rgb {
                r: 255,
                g: 255,
                b: 255
            }
        );
        assert_eq!(Rgb::parse("black").unwrap(), Rgb { r: 0, g: 0, b: 0 });
        assert_eq!(Rgb::parse("red").unwrap(), Rgb { r: 255, g: 0, b: 0 });
        assert_eq!(
            Rgb::parse("FoReStGReEn").unwrap(),
            Rgb {
                r: 34,
                g: 139,
                b: 34
            }
        );
        assert_eq!(
            Rgb::parse("medium spring green").unwrap(),
            Rgb {
                r: 0,
                g: 250,
                b: 154
            }
        );
        assert!(Rgb::parse("nosuchcolor").is_err());
        assert!(X11_TABLE.len() > 700);
    }

    #[test]
    fn rgb_encode16() {
        let mut s = String::new();
        Rgb { r: 255, g: 0, b: 0 }.encode_rgb16(&mut s);
        assert_eq!(s, "rgb:ffff/0000/0000");
    }

    #[test]
    fn osc4_parse_and_report() {
        let body = b"0;rgb:ffff/0000/0000;1;?";
        let reqs = parse_requests(ColorOperation::Osc4, body);
        assert_eq!(
            reqs,
            vec![
                ColorRequest::Set {
                    target: ColorTarget::Palette(0),
                    color: Rgb { r: 255, g: 0, b: 0 }
                },
                ColorRequest::Query(ColorTarget::Palette(1)),
            ]
        );

        let mut report = String::new();
        write_xterm_color_report(
            ColorTarget::Palette(1),
            Rgb { r: 1, g: 2, b: 3 },
            Terminator::St,
            &mut report,
        );
        assert_eq!(report, "\x1b]4;1;rgb:0101/0202/0303\x1b\\");
    }
    #[test]
    fn osc104_empty_resets_palette() {
        let reqs = parse_requests(ColorOperation::Osc104, b"");
        assert_eq!(reqs, vec![ColorRequest::ResetPalette]);
        let reqs = parse_requests(ColorOperation::Osc105, b"");
        assert_eq!(reqs, vec![ColorRequest::ResetSpecial]);
    }

    #[test]
    fn osc104_indexed_reset_and_special_256() {
        // Ghostty osc/parsers/color.zig: 0-255→palette; 256-260→color.Special
        // (0=bold…4=italic; 5-7 invalid/ignored).
        let reqs = parse_requests(ColorOperation::Osc104, b"0;1;256;300;garbage");
        assert_eq!(
            reqs,
            vec![
                ColorRequest::Reset(ColorTarget::Palette(0)),
                ColorRequest::Reset(ColorTarget::Palette(1)),
                ColorRequest::Reset(ColorTarget::Special(SpecialColor::Bold)),
            ]
        );
    }

    #[test]
    fn osc10_dynamic_query() {
        let reqs = parse_requests(ColorOperation::Osc10, b"?");
        assert_eq!(
            reqs,
            vec![ColorRequest::Query(ColorTarget::Dynamic(
                DynamicColor::Foreground
            ))]
        );
    }

    #[test]
    fn osc11_dynamic_sequence() {
        let reqs = parse_requests(ColorOperation::Osc11, b"#000000;#111111");
        assert_eq!(
            reqs,
            vec![
                ColorRequest::Set {
                    target: ColorTarget::Dynamic(DynamicColor::Background),
                    color: Rgb { r: 0, g: 0, b: 0 }
                },
                ColorRequest::Set {
                    target: ColorTarget::Dynamic(DynamicColor::Cursor),
                    color: Rgb {
                        r: 17,
                        g: 17,
                        b: 17
                    }
                },
            ]
        );
    }

    #[test]
    fn kitty_color_parse() {
        let reqs = parse_kitty_color_requests(b"1=#ff0000;foreground=rgb:0/255/0;cursor=?;3");
        assert_eq!(
            reqs,
            vec![
                KittyColorRequest::Set {
                    key: KittyColorKey::Palette(1),
                    color: Rgb { r: 255, g: 0, b: 0 }
                },
                KittyColorRequest::Set {
                    key: KittyColorKey::Foreground,
                    color: Rgb { r: 0, g: 37, b: 0 }
                },
                KittyColorRequest::Query(KittyColorKey::Cursor),
                KittyColorRequest::Reset(KittyColorKey::Palette(3)),
            ]
        );
    }
}

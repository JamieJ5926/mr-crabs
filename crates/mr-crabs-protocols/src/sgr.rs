//! SGR attribute tracking for DECRQSS replies.
//!
//! The terminal layer observes `terminal_attribute` (CSI SGR) through its
//! handler wrapper and maintains an [`SgrState`]; [`SgrState::print_attributes`]
//! produces the `;`-separated attribute list Ghostty's `printAttributes`
//! emits, which feeds the DECRQSS `sgr` reply.

use crate::color::Rgb;
use std::io::Write;

/// The underline styles representable in SGR (Ghostty `UnderlineStyle`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UnderlineStyle {
    None,
    Solid,
    Curly,
    Dashed,
    Dotted,
    Double,
}

/// Tracked SGR attribute state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SgrState {
    pub bold: bool,
    pub faint: bool,
    pub italic: bool,
    pub underline: UnderlineStyle,
    pub blink: bool,
    pub inverse: bool,
    pub invisible: bool,
    pub strikethrough: bool,
    pub overline: bool,
    pub foreground: Option<ColorSpec>,
    pub background: Option<ColorSpec>,
}

/// A foreground/background color as set by SGR.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ColorSpec {
    /// `38;5;N` / `48;5;N`
    Indexed(u8),
    /// `38;2;R;G;B` / `48;2;R;G;B`
    Rgb(Rgb),
}

impl SgrState {
    pub fn new() -> Self {
        Self {
            bold: false,
            faint: false,
            italic: false,
            underline: UnderlineStyle::None,
            blink: false,
            inverse: false,
            invisible: false,
            strikethrough: false,
            overline: false,
            foreground: None,
            background: None,
        }
    }

    /// Reset to the default state (SGR 0 / RIS).
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// Apply one SGR attribute (Ghostty `Screen.setAttribute` ordering used by
    /// `printAttributes`). Bold/faint are exclusive only via SGR 22; they do
    /// not clear each other when set individually.
    pub fn apply(&mut self, attr: SgrAttr) {
        match attr {
            SgrAttr::Reset => self.reset(),
            SgrAttr::Bold => self.bold = true,
            SgrAttr::Faint => self.faint = true,
            SgrAttr::Italic => self.italic = true,
            SgrAttr::Underline(style) => self.underline = style,
            SgrAttr::NoUnderline => self.underline = UnderlineStyle::None,
            SgrAttr::Blink => self.blink = true,
            SgrAttr::NoBlink => self.blink = false,
            SgrAttr::Inverse => self.inverse = true,
            SgrAttr::NoInverse => self.inverse = false,
            SgrAttr::Invisible => self.invisible = true,
            SgrAttr::NoInvisible => self.invisible = false,
            SgrAttr::Strikethrough => self.strikethrough = true,
            SgrAttr::NoStrikethrough => self.strikethrough = false,
            SgrAttr::Overline => self.overline = true,
            SgrAttr::NoOverline => self.overline = false,
            SgrAttr::Foreground(Some(color)) => self.foreground = Some(color),
            SgrAttr::Foreground(None) => self.foreground = None,
            SgrAttr::Background(Some(color)) => self.background = Some(color),
            SgrAttr::Background(None) => self.background = None,
        }
    }

    /// Encode the current attributes as a `;`-separated list (Ghostty
    /// `Terminal.printAttributes`). The list always starts with `0`; order is
    /// bold/faint/italic/underline/overline then blink/inverse/invisible/
    /// strikethrough; a trailing `m` is added by the DECRQSS encoder.
    pub fn print_attributes(&self, out: &mut Vec<u8>) {
        out.extend_from_slice(b"0");
        if self.bold {
            out.extend_from_slice(b";1");
        }
        if self.faint {
            out.extend_from_slice(b";2");
        }
        if self.italic {
            out.extend_from_slice(b";3");
        }
        match self.underline {
            UnderlineStyle::None => {}
            UnderlineStyle::Solid => out.extend_from_slice(b";4"),
            UnderlineStyle::Double => out.extend_from_slice(b";4:2"),
            UnderlineStyle::Curly => out.extend_from_slice(b";4:3"),
            UnderlineStyle::Dotted => out.extend_from_slice(b";4:4"),
            UnderlineStyle::Dashed => out.extend_from_slice(b";4:5"),
        }
        if self.overline {
            out.extend_from_slice(b";53");
        }
        if self.blink {
            out.extend_from_slice(b";5");
        }
        if self.inverse {
            out.extend_from_slice(b";7");
        }
        if self.invisible {
            out.extend_from_slice(b";8");
        }
        if self.strikethrough {
            out.extend_from_slice(b";9");
        }
        if let Some(fg) = &self.foreground {
            out.extend_from_slice(b";");
            encode_color(fg, false, out);
        }
        if let Some(bg) = &self.background {
            out.extend_from_slice(b";");
            encode_color(bg, true, out);
        }
    }
}

fn encode_color(color: &ColorSpec, background: bool, out: &mut Vec<u8>) {
    match color {
        ColorSpec::Indexed(i) => {
            // Ghostty Terminal.printAttributes short-forms 0-7/8-15; 16-255 use :5:.
            if *i >= 16 {
                let _ = write!(out, "{}:5:{}", if background { 48 } else { 38 }, i);
            } else if *i >= 8 {
                let _ = write!(out, "{}{}", if background { 10 } else { 9 }, i - 8);
            } else {
                let _ = write!(out, "{}{}", if background { 4 } else { 3 }, i);
            }
        }
        ColorSpec::Rgb(rgb) => {
            let _ = write!(
                out,
                "{}:2::{}:{}:{}",
                if background { 48 } else { 38 },
                rgb.r,
                rgb.g,
                rgb.b
            );
        }
    }
}

/// One SGR attribute as observed from the vte `terminal_attribute` hook.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SgrAttr {
    Reset,
    Bold,
    Faint,
    Italic,
    Underline(UnderlineStyle),
    NoUnderline,
    Blink,
    NoBlink,
    Inverse,
    NoInverse,
    Invisible,
    NoInvisible,
    Strikethrough,
    NoStrikethrough,
    Overline,
    NoOverline,
    Foreground(Option<ColorSpec>),
    Background(Option<ColorSpec>),
}

impl Default for SgrState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn prints_default_and_styles() {
        let s = SgrState::new();
        let mut out = Vec::new();
        s.print_attributes(&mut out);
        assert_eq!(out, b"0");
    }

    #[test]
    fn ghostty_docrqss_example() {
        let mut s = SgrState::new();
        s.apply(SgrAttr::Bold);
        s.apply(SgrAttr::Underline(UnderlineStyle::Curly));
        let mut out = Vec::new();
        s.print_attributes(&mut out);
        assert_eq!(out, b"0;1;4:3");
    }

    #[test]
    fn ghostty_largest_response_example() {
        let mut s = SgrState::new();
        for attr in [
            SgrAttr::Bold,
            SgrAttr::Faint,
            SgrAttr::Italic,
            SgrAttr::Underline(UnderlineStyle::Dashed),
            SgrAttr::Blink,
            SgrAttr::Inverse,
            SgrAttr::Invisible,
            SgrAttr::Strikethrough,
            SgrAttr::Overline,
            SgrAttr::Foreground(Some(ColorSpec::Rgb(Rgb {
                r: 255,
                g: 255,
                b: 255,
            }))),
            SgrAttr::Background(Some(ColorSpec::Rgb(Rgb {
                r: 255,
                g: 255,
                b: 255,
            }))),
        ] {
            s.apply(attr);
        }
        let mut out = Vec::new();
        s.print_attributes(&mut out);
        // Ghostty Terminal.printAttributes order: 1,2,3,4,53,5,7,8,9 then colors.
        // Dashed underline = 4:5 (Ghostty sgr.Attribute.Underline: dotted=4,
        // dashed=5, style.zig formatter tests).
        assert_eq!(
            out,
            b"0;1;2;3;4:5;53;5;7;8;9;38:2::255:255:255;48:2::255:255:255"
        );
    }
    #[test]
    fn indexed_colors() {
        let mut s = SgrState::new();
        s.apply(SgrAttr::Foreground(Some(ColorSpec::Indexed(196))));
        let mut out = Vec::new();
        s.print_attributes(&mut out);
        assert_eq!(out, b"0;38:5:196");
    }

    #[test]
    fn resets_clear() {
        let mut s = SgrState::new();
        s.apply(SgrAttr::Bold);
        s.apply(SgrAttr::Foreground(Some(ColorSpec::Indexed(1))));
        s.apply(SgrAttr::Reset);
        assert_eq!(s, SgrState::new());
    }
}

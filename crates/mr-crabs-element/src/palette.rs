//! Deterministic mapping from the terminal's normalized colors to GPUI
//! [`Hsla`] values, plus the element's default palette (background, cursor,
//! selection). Pure functions, headless-testable.

use gpui::{Hsla, UnderlineStyle, hsla, px, rgba};
use mr_crabs_terminal::{NamedColorValue, NormalizedColor, Style as TermStyle};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerminalPalette {
    pub foreground: [u8; 3],
    pub background: [u8; 3],
    pub cursor: [u8; 3],
    pub selection: [u8; 3],
    pub background_opacity: f32,
}

impl TerminalPalette {
    pub const fn dark(background_opacity: f32) -> Self {
        Self {
            foreground: [0xe5, 0xe5, 0xe5],
            background: [0x0d, 0x0d, 0x0d],
            cursor: [0xe5, 0xe5, 0xe5],
            selection: [0x52, 0x78, 0xc8],
            background_opacity,
        }
    }

    pub const fn light(background_opacity: f32) -> Self {
        Self {
            foreground: [0x20, 0x20, 0x20],
            background: [0xf5, 0xf5, 0xf5],
            cursor: [0x20, 0x20, 0x20],
            selection: [0x52, 0x78, 0xc8],
            background_opacity,
        }
    }

    pub fn background_color(self) -> Hsla {
        rgb_hsla(self.background, self.background_opacity.clamp(0.0, 1.0))
    }

    pub fn cursor_color(self) -> Hsla {
        rgb_hsla(self.cursor, 1.0)
    }

    pub fn selection_color(self) -> Hsla {
        rgb_hsla(self.selection, 0.3)
    }
}

impl Default for TerminalPalette {
    fn default() -> Self {
        Self::dark(1.0)
    }
}
/// The classic xterm 16-color ANSI palette (RGB).
pub const ANSI_PALETTE: [[u8; 3]; 16] = [
    [0x00, 0x00, 0x00], // Black
    [0xcd, 0x00, 0x00], // Red
    [0x00, 0xcd, 0x00], // Green
    [0xcd, 0xcd, 0x00], // Yellow
    [0x00, 0x00, 0xee], // Blue
    [0xcd, 0x00, 0xcd], // Magenta
    [0x00, 0xcd, 0xcd], // Cyan
    [0xe5, 0xe5, 0xe5], // White
    [0x7f, 0x7f, 0x7f], // BrightBlack
    [0xff, 0x00, 0x00], // BrightRed
    [0x00, 0xff, 0x00], // BrightGreen
    [0xff, 0xff, 0x00], // BrightYellow
    [0x5c, 0x5c, 0xff], // BrightBlue
    [0xff, 0x00, 0xff], // BrightMagenta
    [0x00, 0xff, 0xff], // BrightCyan
    [0xff, 0xff, 0xff], // BrightWhite
];

/// Map a named ANSI color to RGB.
pub fn named_rgb(value: NamedColorValue) -> [u8; 3] {
    use NamedColorValue::*;
    match value {
        Black => ANSI_PALETTE[0],
        Red => ANSI_PALETTE[1],
        Green => ANSI_PALETTE[2],
        Yellow => ANSI_PALETTE[3],
        Blue => ANSI_PALETTE[4],
        Magenta => ANSI_PALETTE[5],
        Cyan => ANSI_PALETTE[6],
        White => ANSI_PALETTE[7],
        BrightBlack => ANSI_PALETTE[8],
        BrightRed => ANSI_PALETTE[9],
        BrightGreen => ANSI_PALETTE[10],
        BrightYellow => ANSI_PALETTE[11],
        BrightBlue => ANSI_PALETTE[12],
        BrightMagenta => ANSI_PALETTE[13],
        BrightCyan => ANSI_PALETTE[14],
        BrightWhite => ANSI_PALETTE[15],
        Foreground => [0xe5, 0xe5, 0xe5],
        Background => [0x0d, 0x0d, 0x0d],
        Cursor => [0xe5, 0xe5, 0xe5],
        DimBlack => [0x33, 0x33, 0x33],
        DimRed => [0x8b, 0x00, 0x00],
        DimGreen => [0x00, 0x8b, 0x00],
        DimYellow => [0x8b, 0x8b, 0x00],
        DimBlue => [0x00, 0x00, 0x8b],
        DimMagenta => [0x8b, 0x00, 0x8b],
        DimCyan => [0x00, 0x8b, 0x8b],
        DimWhite => [0x99, 0x99, 0x99],
        BrightForeground => [0xff, 0xff, 0xff],
        DimForeground => [0xa0, 0xa0, 0xa0],
    }
}
pub fn named_rgb_with_palette(value: NamedColorValue, palette: TerminalPalette) -> [u8; 3] {
    match value {
        NamedColorValue::Foreground => palette.foreground,
        NamedColorValue::Background => palette.background,
        NamedColorValue::Cursor => palette.cursor,
        other => named_rgb(other),
    }
}

/// Map an indexed color to RGB using the standard xterm rules: indices
/// 0–15 use the ANSI palette, 16–231 the 6×6×6 color cube, 232–255 the
/// grayscale ramp.
pub fn indexed_rgb(index: u8) -> [u8; 3] {
    match index {
        0..=15 => ANSI_PALETTE[usize::from(index)],
        16..=231 => {
            let value = u16::from(index) - 16;
            let r = value / 36;
            let g = (value / 6) % 6;
            let b = value % 6;
            [cube_level(r), cube_level(g), cube_level(b)]
        }
        232..=255 => {
            let level = 8 + u16::from(index - 232) * 10;
            let v = level.min(255) as u8;
            [v, v, v]
        }
    }
}

/// Map a normalized terminal color to GPUI [`Hsla`].
///
/// Terminal colors are opaque RGB triples; `gpui::rgba` interprets its `u32`
/// as RRGGBBAA, so the alpha byte must be set explicitly (a bare RGB value
/// would otherwise read the blue channel as alpha).
pub fn color_to_hsla(color: &NormalizedColor) -> Hsla {
    let rgb = match color {
        NormalizedColor::Named(name) => named_rgb(*name),
        NormalizedColor::Indexed(index) => indexed_rgb(*index),
        NormalizedColor::Rgb(rgb) => *rgb,
    };
    let hex = (u32::from(rgb[0]) << 24)
        | (u32::from(rgb[1]) << 16)
        | (u32::from(rgb[2]) << 8)
        | 0x0000_00ff;
    Hsla::from(rgba(hex))
}
pub fn color_to_hsla_with_palette(color: &NormalizedColor, palette: TerminalPalette) -> Hsla {
    let rgb = match color {
        NormalizedColor::Named(name) => named_rgb_with_palette(*name, palette),
        NormalizedColor::Indexed(index) => indexed_rgb(*index),
        NormalizedColor::Rgb(rgb) => *rgb,
    };
    rgb_hsla(rgb, 1.0)
}

pub fn style_foreground_with_palette(style: &TermStyle, palette: TerminalPalette) -> Hsla {
    color_to_hsla_with_palette(&style.foreground, palette)
}

pub fn style_background_with_palette(style: &TermStyle, palette: TerminalPalette) -> Hsla {
    color_to_hsla_with_palette(&style.background, palette)
}

/// The text color for a terminal style.
pub fn style_foreground(style: &TermStyle) -> Hsla {
    color_to_hsla(&style.foreground)
}

/// The cell background color for a terminal style.
pub fn style_background(style: &TermStyle) -> Hsla {
    color_to_hsla(&style.background)
}

/// The underline decoration for a terminal style, when it carries an
/// underline color.
pub fn style_underline(style: &TermStyle) -> Option<UnderlineStyle> {
    style.underline.as_ref().map(|color| UnderlineStyle {
        thickness: px(1.0),
        color: Some(color_to_hsla(color)),
        wavy: false,
    })
}

/// Default element background (near-black).
pub fn background_color() -> Hsla {
    hsla(0.0, 0.0, 0.05, 1.0)
}

/// Default cursor color (near-white).
pub fn cursor_color() -> Hsla {
    hsla(0.0, 0.0, 0.9, 1.0)
}

/// Default selection overlay (translucent blue).
pub fn selection_color() -> Hsla {
    hsla(0.6, 0.5, 0.6, 0.3)
}
fn rgb_hsla(rgb: [u8; 3], alpha: f32) -> Hsla {
    let alpha = (alpha * 255.0).round() as u32;
    let hex =
        (u32::from(rgb[0]) << 24) | (u32::from(rgb[1]) << 16) | (u32::from(rgb[2]) << 8) | alpha;
    Hsla::from(rgba(hex))
}

fn cube_level(index: u16) -> u8 {
    const LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    LEVELS[usize::from(index.min(5))]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn named_colors_map_to_palette() {
        assert_eq!(named_rgb(NamedColorValue::Red), [0xcd, 0x00, 0x00]);
        assert_eq!(named_rgb(NamedColorValue::BrightCyan), [0x00, 0xff, 0xff]);
        assert_eq!(named_rgb(NamedColorValue::Black), [0x00, 0x00, 0x00]);
        // Foreground/background/cursor have deterministic (non-palette) values.
        assert_eq!(named_rgb(NamedColorValue::Foreground), [0xe5, 0xe5, 0xe5]);
        assert_eq!(named_rgb(NamedColorValue::Background), [0x0d, 0x0d, 0x0d]);
    }

    #[test]
    fn indexed_colors_follow_xterm_rules() {
        assert_eq!(indexed_rgb(0), ANSI_PALETTE[0]);
        assert_eq!(indexed_rgb(15), ANSI_PALETTE[15]);
        // 16 = (0,0,0) cube corner.
        assert_eq!(indexed_rgb(16), [0, 0, 0]);
        // 46 = (0,5,0) cube corner: pure green.
        assert_eq!(indexed_rgb(46), [0, 255, 0]);
        // 58 = (1,1,0) cube level.
        assert_eq!(indexed_rgb(58), [95, 95, 0]);
        // 231 = (5,5,5).
        assert_eq!(indexed_rgb(231), [255, 255, 255]);
        // 232 = gray 8.
        assert_eq!(indexed_rgb(232), [8, 8, 8]);
        // 255 = gray 238.
        assert_eq!(indexed_rgb(255), [238, 238, 238]);
    }

    #[test]
    fn rgb_colors_pass_through_opaque() {
        let color = NormalizedColor::Rgb([1, 2, 3]);
        assert_eq!(color_to_hsla(&color), Hsla::from(rgba(0x010203ff)));
        assert_eq!(color_to_hsla(&color).a, 1.0);
        // A blue-heavy channel must not leak into alpha.
        let red = color_to_hsla(&NormalizedColor::Rgb([255, 0, 0]));
        assert_eq!(red.a, 1.0);
    }

    #[test]
    fn hsla_channels_are_in_range() {
        for value in [
            NormalizedColor::Named(NamedColorValue::BrightRed),
            NormalizedColor::Indexed(123),
            NormalizedColor::Rgb([200, 100, 50]),
        ] {
            let h = color_to_hsla(&value);
            assert!((0.0..=1.0).contains(&h.h));
            assert!((0.0..=1.0).contains(&h.s));
            assert!((0.0..=1.0).contains(&h.l));
            assert_eq!(h.a, 1.0);
        }
    }

    #[test]
    fn style_mapping_uses_all_three_channels() {
        let style = TermStyle {
            foreground: NormalizedColor::Rgb([10, 20, 30]),
            background: NormalizedColor::Named(NamedColorValue::Blue),
            underline: Some(NormalizedColor::Rgb([1, 1, 1])),
        };
        assert_eq!(style_foreground(&style), Hsla::from(rgba(0x0a141eff)));
        assert_eq!(style_background(&style), Hsla::from(rgba(0x0000eeff)));
        let underline = style_underline(&style).expect("underline color present");
        assert_eq!(underline.color, Some(Hsla::from(rgba(0x010101ff))));
        assert!(!underline.wavy);
    }

    #[test]
    fn style_without_underline_has_none() {
        assert_eq!(style_underline(&TermStyle::default()), None);
    }

    #[test]
    fn configured_palette_controls_named_colors_and_background_opacity() {
        let palette = TerminalPalette::light(0.5);
        assert_eq!(
            named_rgb_with_palette(NamedColorValue::Foreground, palette),
            [0x20, 0x20, 0x20]
        );
        assert_eq!(
            named_rgb_with_palette(NamedColorValue::Background, palette),
            [0xf5, 0xf5, 0xf5]
        );
        assert!((palette.background_color().a - 0.5).abs() < 0.01);
    }

    #[test]
    fn defaults_are_opaque_and_deterministic() {
        assert_eq!(background_color(), background_color());
        assert_eq!(cursor_color(), cursor_color());
        assert_eq!(selection_color(), selection_color());
        assert_eq!(background_color().a, 1.0);
        assert_eq!(cursor_color().a, 1.0);
        assert_eq!(selection_color().a, 0.3);
    }
}

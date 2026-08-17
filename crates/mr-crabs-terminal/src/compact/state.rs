//! Cursor, mode, charset, tab, color, and pen state for CompactEngine.

use std::ops::Range;

use vte::ansi::{
    Attr, CharsetIndex, Color, CursorShape, CursorStyle, KeyboardModes, NamedColor, Rgb,
    StandardCharset,
};

use crate::compact::flags;
use crate::side_tables::StyleTable;
use crate::{NamedColorValue, NormalizedColor, Style, TerminalMode};

pub const TITLE_STACK_MAX: usize = 4096;
pub const KEYBOARD_STACK_MAX: usize = 16;
pub const COLOR_COUNT: usize = 260;
pub const TAB_INTERVAL: usize = 8;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct CursorPos {
    pub row: u16,
    pub col: u16,
    pub wrap_pending: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Pen {
    pub style: u16,
    pub flags: u16,
    pub hyperlink: Option<u32>,
    pub fg: NormalizedColor,
    pub bg: NormalizedColor,
    pub underline: Option<NormalizedColor>,
    style_dirty: bool,
}

impl Default for Pen {
    fn default() -> Self {
        Self {
            style: 0,
            flags: 0,
            hyperlink: None,
            fg: NormalizedColor::Named(NamedColorValue::Foreground),
            bg: NormalizedColor::Named(NamedColorValue::Background),
            underline: None,
            style_dirty: false,
        }
    }
}

impl Pen {
    pub fn intern(&mut self, table: &mut StyleTable) {
        if !self.style_dirty {
            return;
        }
        self.style = table.intern(Style {
            foreground: self.fg,
            background: self.bg,
            underline: self.underline,
        });
        self.style_dirty = false;
    }

    pub fn erased_cell(&mut self, table: &mut StyleTable) -> crate::Cell {
        let style = if matches!(self.bg, NormalizedColor::Named(NamedColorValue::Background)) {
            0
        } else {
            self.intern(table);
            self.style
        };
        crate::Cell {
            content: u32::from(' '),
            style,
            flags: 0,
        }
    }

    pub fn apply_attr(&mut self, attr: Attr) {
        match attr {
            Attr::Foreground(color) => {
                self.fg = normalize_color(color);
                self.style_dirty = true;
            }
            Attr::Background(color) => {
                self.bg = normalize_color(color);
                self.style_dirty = true;
            }
            Attr::UnderlineColor(color) => {
                self.underline = color.map(normalize_color);
                self.style_dirty = true;
            }
            Attr::Reset => *self = Self::default(),
            Attr::Reverse => self.flags |= flags::INVERSE,
            Attr::CancelReverse => self.flags &= !flags::INVERSE,
            Attr::Bold => self.flags |= flags::BOLD,
            Attr::CancelBold => self.flags &= !flags::BOLD,
            Attr::Dim => self.flags |= flags::DIM,
            Attr::CancelBoldDim => self.flags &= !(flags::BOLD | flags::DIM),
            Attr::Italic => self.flags |= flags::ITALIC,
            Attr::CancelItalic => self.flags &= !flags::ITALIC,
            Attr::Underline => {
                self.flags &= !flags::ALL_UNDERLINES;
                self.flags |= flags::UNDERLINE;
            }
            Attr::DoubleUnderline => {
                self.flags &= !flags::ALL_UNDERLINES;
                self.flags |= flags::DOUBLE_UNDERLINE;
            }
            Attr::Undercurl => {
                self.flags &= !flags::ALL_UNDERLINES;
                self.flags |= flags::UNDERCURL;
            }
            Attr::DottedUnderline => {
                self.flags &= !flags::ALL_UNDERLINES;
                self.flags |= flags::DOTTED_UNDERLINE;
            }
            Attr::DashedUnderline => {
                self.flags &= !flags::ALL_UNDERLINES;
                self.flags |= flags::DASHED_UNDERLINE;
            }
            Attr::CancelUnderline => self.flags &= !flags::ALL_UNDERLINES,
            Attr::Hidden => self.flags |= flags::HIDDEN,
            Attr::CancelHidden => self.flags &= !flags::HIDDEN,
            Attr::Strike => self.flags |= flags::STRIKEOUT,
            Attr::CancelStrike => self.flags &= !flags::STRIKEOUT,
            _ => {}
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SavedCursor {
    pub pos: CursorPos,
    pub pen: Pen,
    pub charsets: [StandardCharset; 4],
    pub active_charset: CharsetIndex,
}

impl Default for SavedCursor {
    fn default() -> Self {
        Self {
            pos: CursorPos::default(),
            pen: Pen::default(),
            charsets: [StandardCharset::Ascii; 4],
            active_charset: CharsetIndex::G0,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TabStops {
    stops: Vec<bool>,
}

impl TabStops {
    pub fn new(cols: usize) -> Self {
        let mut stops = vec![false; cols];
        for (index, stop) in stops.iter_mut().enumerate() {
            *stop = index != 0 && index % TAB_INTERVAL == 0;
        }
        Self { stops }
    }

    pub fn resize(&mut self, cols: usize) {
        let old = self.stops.len();
        self.stops.resize(cols, false);
        for index in old..cols {
            self.stops[index] = index != 0 && index % TAB_INTERVAL == 0;
        }
    }

    pub fn set(&mut self, col: usize, on: bool) {
        if let Some(stop) = self.stops.get_mut(col) {
            *stop = on;
        }
    }

    pub fn is_set(&self, col: usize) -> bool {
        self.stops.get(col).copied().unwrap_or(false)
    }

    pub fn clear_all(&mut self) {
        for stop in &mut self.stops {
            *stop = false;
        }
    }

    pub fn next(&self, col: usize) -> Option<usize> {
        let cols = self.stops.len();
        if col + 1 >= cols {
            return None;
        }
        (col + 1..cols).find(|&index| self.stops[index])
    }

    pub fn prev(&self, col: usize) -> Option<usize> {
        if col == 0 {
            return None;
        }
        (0..col).rev().find(|&index| self.stops[index])
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ModeBits(u32);

impl ModeBits {
    const SHOW_CURSOR: u32 = 1 << 0;
    const APP_CURSOR: u32 = 1 << 1;
    const APP_KEYPAD: u32 = 1 << 2;
    const MOUSE_REPORT_CLICK: u32 = 1 << 3;
    const BRACKETED_PASTE: u32 = 1 << 4;
    const SGR_MOUSE: u32 = 1 << 5;
    const MOUSE_MOTION: u32 = 1 << 6;
    const LINE_WRAP: u32 = 1 << 7;
    const LINE_FEED_NEW_LINE: u32 = 1 << 8;
    const ORIGIN: u32 = 1 << 9;
    const INSERT: u32 = 1 << 10;
    const FOCUS_IN_OUT: u32 = 1 << 11;
    const ALT_SCREEN: u32 = 1 << 12;
    const MOUSE_DRAG: u32 = 1 << 13;
    const UTF8_MOUSE: u32 = 1 << 14;
    const ALTERNATE_SCROLL: u32 = 1 << 15;
    const VI: u32 = 1 << 16;
    const URGENCY_HINTS: u32 = 1 << 17;
    const DISAMBIGUATE_ESC_CODES: u32 = 1 << 18;
    const REPORT_EVENT_TYPES: u32 = 1 << 19;
    const REPORT_ALTERNATE_KEYS: u32 = 1 << 20;
    const REPORT_ALL_KEYS_AS_ESC: u32 = 1 << 21;
    const REPORT_ASSOCIATED_TEXT: u32 = 1 << 22;
    const BLINKING_CURSOR: u32 = 1 << 23;
    const MOUSE_MODE: u32 = Self::MOUSE_REPORT_CLICK | Self::MOUSE_DRAG | Self::MOUSE_MOTION;
    const KITTY: u32 = Self::DISAMBIGUATE_ESC_CODES
        | Self::REPORT_EVENT_TYPES
        | Self::REPORT_ALTERNATE_KEYS
        | Self::REPORT_ALL_KEYS_AS_ESC
        | Self::REPORT_ASSOCIATED_TEXT;

    pub fn default_live() -> Self {
        Self(Self::SHOW_CURSOR | Self::LINE_WRAP | Self::ALTERNATE_SCROLL | Self::URGENCY_HINTS)
    }

    pub fn insert(&mut self, bit: u32) {
        self.0 |= bit;
    }

    pub fn remove(&mut self, bit: u32) {
        self.0 &= !bit;
    }

    pub fn contains(self, bit: u32) -> bool {
        self.0 & bit == bit
    }

    pub fn toggle_alt(&mut self) {
        self.0 ^= Self::ALT_SCREEN;
    }

    pub fn reset_keep_vi(&mut self) {
        let vi = self.0 & Self::VI;
        *self = Self::default_live();
        self.0 |= vi;
    }

    pub fn apply_kitty(&mut self, mode: KeyboardModes, replace: KittyApply) {
        let incoming = kitty_bits(mode);
        let active = self.0 & Self::KITTY;
        self.0 &= !Self::KITTY;
        let next = match replace {
            KittyApply::Replace => incoming,
            KittyApply::Union => active | incoming,
            KittyApply::Difference => active & !incoming,
        };
        self.0 |= next;
    }

    pub fn to_terminal_modes(self) -> Vec<TerminalMode> {
        const MAP: &[(u32, TerminalMode)] = &[
            (ModeBits::SHOW_CURSOR, TerminalMode::ShowCursor),
            (ModeBits::APP_CURSOR, TerminalMode::AppCursor),
            (ModeBits::APP_KEYPAD, TerminalMode::AppKeypad),
            (ModeBits::MOUSE_REPORT_CLICK, TerminalMode::MouseReportClick),
            (ModeBits::BRACKETED_PASTE, TerminalMode::BracketedPaste),
            (ModeBits::SGR_MOUSE, TerminalMode::SgrMouse),
            (ModeBits::MOUSE_MOTION, TerminalMode::MouseMotion),
            (ModeBits::LINE_WRAP, TerminalMode::LineWrap),
            (ModeBits::LINE_FEED_NEW_LINE, TerminalMode::LineFeedNewLine),
            (ModeBits::ORIGIN, TerminalMode::Origin),
            (ModeBits::INSERT, TerminalMode::Insert),
            (ModeBits::FOCUS_IN_OUT, TerminalMode::FocusInOut),
            (ModeBits::ALT_SCREEN, TerminalMode::AltScreen),
            (ModeBits::MOUSE_DRAG, TerminalMode::MouseDrag),
            (ModeBits::UTF8_MOUSE, TerminalMode::Utf8Mouse),
            (ModeBits::ALTERNATE_SCROLL, TerminalMode::AlternateScroll),
            (ModeBits::VI, TerminalMode::Vi),
            (ModeBits::URGENCY_HINTS, TerminalMode::UrgencyHints),
            (
                ModeBits::DISAMBIGUATE_ESC_CODES,
                TerminalMode::DisambiguateEscCodes,
            ),
            (ModeBits::REPORT_EVENT_TYPES, TerminalMode::ReportEventTypes),
            (
                ModeBits::REPORT_ALTERNATE_KEYS,
                TerminalMode::ReportAlternateKeys,
            ),
            (
                ModeBits::REPORT_ALL_KEYS_AS_ESC,
                TerminalMode::ReportAllKeysAsEsc,
            ),
            (
                ModeBits::REPORT_ASSOCIATED_TEXT,
                TerminalMode::ReportAssociatedText,
            ),
        ];
        MAP.iter()
            .filter_map(|(bit, mode)| self.contains(*bit).then_some(*mode))
            .collect()
    }

    pub fn has_terminal_mode(self, mode: TerminalMode) -> bool {
        self.contains(mode_bit(mode))
    }

    pub const fn show_cursor() -> u32 {
        Self::SHOW_CURSOR
    }
    pub const fn app_cursor() -> u32 {
        Self::APP_CURSOR
    }
    pub const fn app_keypad() -> u32 {
        Self::APP_KEYPAD
    }
    pub const fn mouse_report_click() -> u32 {
        Self::MOUSE_REPORT_CLICK
    }
    pub const fn bracketed_paste() -> u32 {
        Self::BRACKETED_PASTE
    }
    pub const fn sgr_mouse() -> u32 {
        Self::SGR_MOUSE
    }
    pub const fn mouse_motion() -> u32 {
        Self::MOUSE_MOTION
    }
    pub const fn line_wrap() -> u32 {
        Self::LINE_WRAP
    }
    pub const fn line_feed_new_line() -> u32 {
        Self::LINE_FEED_NEW_LINE
    }
    pub const fn origin() -> u32 {
        Self::ORIGIN
    }
    pub const fn insert_mode() -> u32 {
        Self::INSERT
    }
    pub const fn focus_in_out() -> u32 {
        Self::FOCUS_IN_OUT
    }
    pub const fn mouse_drag() -> u32 {
        Self::MOUSE_DRAG
    }
    pub const fn utf8_mouse() -> u32 {
        Self::UTF8_MOUSE
    }
    pub const fn alternate_scroll() -> u32 {
        Self::ALTERNATE_SCROLL
    }
    pub const fn urgency_hints() -> u32 {
        Self::URGENCY_HINTS
    }
    pub const fn blinking_cursor() -> u32 {
        Self::BLINKING_CURSOR
    }
    pub const fn mouse_mode() -> u32 {
        Self::MOUSE_MODE
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum KittyApply {
    Replace,
    Union,
    Difference,
}

fn kitty_bits(mode: KeyboardModes) -> u32 {
    let mut bits = 0;
    if mode.contains(KeyboardModes::DISAMBIGUATE_ESC_CODES) {
        bits |= ModeBits::DISAMBIGUATE_ESC_CODES;
    }
    if mode.contains(KeyboardModes::REPORT_EVENT_TYPES) {
        bits |= ModeBits::REPORT_EVENT_TYPES;
    }
    if mode.contains(KeyboardModes::REPORT_ALTERNATE_KEYS) {
        bits |= ModeBits::REPORT_ALTERNATE_KEYS;
    }
    if mode.contains(KeyboardModes::REPORT_ALL_KEYS_AS_ESC) {
        bits |= ModeBits::REPORT_ALL_KEYS_AS_ESC;
    }
    if mode.contains(KeyboardModes::REPORT_ASSOCIATED_TEXT) {
        bits |= ModeBits::REPORT_ASSOCIATED_TEXT;
    }
    bits
}

fn mode_bit(mode: TerminalMode) -> u32 {
    match mode {
        TerminalMode::ShowCursor => ModeBits::SHOW_CURSOR,
        TerminalMode::AppCursor => ModeBits::APP_CURSOR,
        TerminalMode::AppKeypad => ModeBits::APP_KEYPAD,
        TerminalMode::MouseReportClick => ModeBits::MOUSE_REPORT_CLICK,
        TerminalMode::BracketedPaste => ModeBits::BRACKETED_PASTE,
        TerminalMode::SgrMouse => ModeBits::SGR_MOUSE,
        TerminalMode::MouseMotion => ModeBits::MOUSE_MOTION,
        TerminalMode::LineWrap => ModeBits::LINE_WRAP,
        TerminalMode::LineFeedNewLine => ModeBits::LINE_FEED_NEW_LINE,
        TerminalMode::Origin => ModeBits::ORIGIN,
        TerminalMode::Insert => ModeBits::INSERT,
        TerminalMode::FocusInOut => ModeBits::FOCUS_IN_OUT,
        TerminalMode::AltScreen => ModeBits::ALT_SCREEN,
        TerminalMode::MouseDrag => ModeBits::MOUSE_DRAG,
        TerminalMode::Utf8Mouse => ModeBits::UTF8_MOUSE,
        TerminalMode::AlternateScroll => ModeBits::ALTERNATE_SCROLL,
        TerminalMode::Vi => ModeBits::VI,
        TerminalMode::UrgencyHints => ModeBits::URGENCY_HINTS,
        TerminalMode::DisambiguateEscCodes => ModeBits::DISAMBIGUATE_ESC_CODES,
        TerminalMode::ReportEventTypes => ModeBits::REPORT_EVENT_TYPES,
        TerminalMode::ReportAlternateKeys => ModeBits::REPORT_ALTERNATE_KEYS,
        TerminalMode::ReportAllKeysAsEsc => ModeBits::REPORT_ALL_KEYS_AS_ESC,
        TerminalMode::ReportAssociatedText => ModeBits::REPORT_ASSOCIATED_TEXT,
    }
}

pub fn normalize_color(color: Color) -> NormalizedColor {
    match color {
        Color::Named(named) => NormalizedColor::Named(named_value(named)),
        Color::Indexed(index) => NormalizedColor::Indexed(index),
        Color::Spec(Rgb { r, g, b }) => NormalizedColor::Rgb([r, g, b]),
    }
}

fn named_value(color: NamedColor) -> NamedColorValue {
    match color {
        NamedColor::Black => NamedColorValue::Black,
        NamedColor::Red => NamedColorValue::Red,
        NamedColor::Green => NamedColorValue::Green,
        NamedColor::Yellow => NamedColorValue::Yellow,
        NamedColor::Blue => NamedColorValue::Blue,
        NamedColor::Magenta => NamedColorValue::Magenta,
        NamedColor::Cyan => NamedColorValue::Cyan,
        NamedColor::White => NamedColorValue::White,
        NamedColor::BrightBlack => NamedColorValue::BrightBlack,
        NamedColor::BrightRed => NamedColorValue::BrightRed,
        NamedColor::BrightGreen => NamedColorValue::BrightGreen,
        NamedColor::BrightYellow => NamedColorValue::BrightYellow,
        NamedColor::BrightBlue => NamedColorValue::BrightBlue,
        NamedColor::BrightMagenta => NamedColorValue::BrightMagenta,
        NamedColor::BrightCyan => NamedColorValue::BrightCyan,
        NamedColor::BrightWhite => NamedColorValue::BrightWhite,
        NamedColor::Foreground => NamedColorValue::Foreground,
        NamedColor::Background => NamedColorValue::Background,
        NamedColor::Cursor => NamedColorValue::Cursor,
        NamedColor::DimBlack => NamedColorValue::DimBlack,
        NamedColor::DimRed => NamedColorValue::DimRed,
        NamedColor::DimGreen => NamedColorValue::DimGreen,
        NamedColor::DimYellow => NamedColorValue::DimYellow,
        NamedColor::DimBlue => NamedColorValue::DimBlue,
        NamedColor::DimMagenta => NamedColorValue::DimMagenta,
        NamedColor::DimCyan => NamedColorValue::DimCyan,
        NamedColor::DimWhite => NamedColorValue::DimWhite,
        NamedColor::BrightForeground => NamedColorValue::BrightForeground,
        NamedColor::DimForeground => NamedColorValue::DimForeground,
    }
}

pub fn map_line_drawing(c: char) -> char {
    match c {
        '_' => ' ',
        '`' => '◆',
        'a' => '▒',
        'b' => '\u{2409}',
        'c' => '\u{240C}',
        'd' => '\u{240D}',
        'e' => '\u{240A}',
        'f' => '°',
        'g' => '±',
        'h' => '\u{2424}',
        'i' => '\u{240B}',
        'j' => '┘',
        'k' => '┐',
        'l' => '┌',
        'm' => '└',
        'n' => '┼',
        'o' => '⎺',
        'p' => '⎻',
        'q' => '─',
        'r' => '⎼',
        's' => '⎽',
        't' => '├',
        'u' => '┤',
        'v' => '┴',
        'w' => '┬',
        'x' => '│',
        'y' => '≤',
        'z' => '≥',
        '{' => 'π',
        '|' => '≠',
        '}' => '£',
        '~' => '·',
        other => other,
    }
}

pub fn map_charset(set: StandardCharset, c: char) -> char {
    match set {
        StandardCharset::Ascii => c,
        StandardCharset::SpecialCharacterAndLineDrawing => map_line_drawing(c),
    }
}

pub fn clamp_scroll_region(top: usize, bottom: usize, rows: u16) -> Option<Range<u16>> {
    if top >= bottom {
        return None;
    }
    let rows = usize::from(rows);
    let start = top.min(rows) as u16;
    let end = bottom.min(rows) as u16;
    if start >= end { None } else { Some(start..end) }
}

pub fn default_cursor_style(blink: bool) -> CursorStyle {
    CursorStyle {
        shape: CursorShape::Block,
        blinking: blink,
    }
}

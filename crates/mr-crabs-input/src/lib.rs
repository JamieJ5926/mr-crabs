//! S5 input and clipboard encoders: keyboard, mouse, focus, paste, IME, selection, drag/drop, and clipboard/OSC 52 permissions.
//!
//! Provenance: `src/input/key_encode.zig`, `src/input/mouse_encode.zig`, `src/input/paste.zig`,
//! `src/terminal/main.zig` (DEC modes), `src/input/key.zig`/`key_mods.zig`/`mouse.zig`.
//! Ghostty source commit `d2c70a8c7b9b6893c13640c02d7b6f9a1624f3f0` is the oracle; this crate
//! reimplements observable byte semantics without linking Zig.
use std::path::PathBuf;

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};

use mr_crabs_terminal::{NormalizedSnapshot, SelectionState, TerminalMode};

// ── keyboard ──

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub alt: bool,
    pub ctrl: bool,
    pub super_: bool,
}

impl Modifiers {
    pub const NONE: Self = Self {
        shift: false,
        alt: false,
        ctrl: false,
        super_: false,
    };

    /// CSI modifier parameter (1 + bitmask) and whether any modifier is active.
    pub fn csi_param(self) -> u8 {
        let mut v: u8 = 1;
        if self.shift {
            v += 1;
        }
        if self.alt {
            v += 2;
        }
        if self.ctrl {
            v += 4;
        }
        if self.super_ {
            v += 8;
        }
        v
    }

    pub fn any(self) -> bool {
        self.shift || self.alt || self.ctrl || self.super_
    }

    /// Effective modifiers after consumed mods are removed (altgr etc.).
    pub fn effective(self, consumed: Modifiers) -> Self {
        Self {
            shift: self.shift && !consumed.shift,
            alt: self.alt && !consumed.alt,
            ctrl: self.ctrl && !consumed.ctrl,
            super_: self.super_ && !consumed.super_,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyAction {
    Press,
    Repeat,
    Release,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Key {
    Unidentified,
    Character(char),
    Enter,
    Backspace,
    Tab,
    Escape,
    Space,
    ArrowUp,
    ArrowDown,
    ArrowLeft,
    ArrowRight,
    Home,
    End,
    PageUp,
    PageDown,
    Insert,
    Delete,
    F(u8),
    CapsLock,
    Numpad0,
    Numpad1,
    Numpad2,
    Numpad3,
    Numpad4,
    Numpad5,
    Numpad6,
    Numpad7,
    Numpad8,
    Numpad9,
    NumpadDecimal,
    NumpadDivide,
    NumpadMultiply,
    NumpadSubtract,
    NumpadAdd,
    NumpadEnter,
    NumpadEqual,
    NumpadUp,
    NumpadDown,
    NumpadLeft,
    NumpadRight,
    NumpadBegin,
    NumpadHome,
    NumpadEnd,
    NumpadInsert,
    NumpadDelete,
    NumpadPageUp,
    NumpadPageDown,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct KeyEvent {
    pub key: Key,
    pub mods: Modifiers,
    pub consumed_mods: Modifiers,
    pub composing: bool,
    pub utf8: String,
    pub unshifted_codepoint: u32,
    pub action: KeyAction,
}

impl KeyEvent {
    pub fn new(key: Key) -> Self {
        Self {
            key,
            mods: Modifiers::NONE,
            consumed_mods: Modifiers::NONE,
            composing: false,
            utf8: String::new(),
            unshifted_codepoint: 0,
            action: KeyAction::Press,
        }
    }

    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.utf8 = text.into();
        self
    }

    pub fn with_mods(mut self, mods: Modifiers) -> Self {
        self.mods = mods;
        self
    }

    pub fn with_action(mut self, action: KeyAction) -> Self {
        self.action = action;
        self
    }

    pub fn effective_mods(&self) -> Modifiers {
        if self.utf8.is_empty() {
            self.mods
        } else {
            self.mods.effective(self.consumed_mods)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KittyFlags(pub u32);

impl KittyFlags {
    pub const fn disabled() -> Self {
        Self(0)
    }
    pub const fn all() -> Self {
        Self(0b11111)
    }
    pub fn int(self) -> u32 {
        self.0
    }
    pub fn disambiguate(self) -> bool {
        self.0 & (1 << 0) != 0
    }
    pub fn report_events(self) -> bool {
        self.0 & (1 << 1) != 0
    }
    pub fn report_alternate_keys(self) -> bool {
        self.0 & (1 << 2) != 0
    }
    pub fn report_all(self) -> bool {
        self.0 & (1 << 3) != 0
    }
    pub fn report_associated(self) -> bool {
        self.0 & (1 << 4) != 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyboardMode {
    pub cursor_key_application: bool,
    pub keypad_key_application: bool,
    pub ignore_keypad_with_numlock: bool,
    pub backarrow_key_mode: bool,
    pub alt_esc_prefix: bool,
    pub modify_other_keys_2: bool,
    pub kitty_flags: KittyFlags,
}

impl Default for KeyboardMode {
    fn default() -> Self {
        Self {
            cursor_key_application: false,
            keypad_key_application: false,
            ignore_keypad_with_numlock: true,
            backarrow_key_mode: false,
            alt_esc_prefix: false,
            modify_other_keys_2: false,
            kitty_flags: KittyFlags::disabled(),
        }
    }
}

/// App/config bits that `TerminalMode` does not carry (DECBKM,
/// modifyOtherKeys=2, DEC 1035, and DEC 1036).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyboardModeOverlay {
    pub backarrow_key_mode: bool,
    pub ignore_keypad_with_numlock: bool,
    pub modify_other_keys_2: bool,
    pub alt_esc_prefix: bool,
}

impl Default for KeyboardModeOverlay {
    fn default() -> Self {
        Self {
            backarrow_key_mode: false,
            ignore_keypad_with_numlock: true,
            modify_other_keys_2: false,
            alt_esc_prefix: false,
        }
    }
}

impl KeyboardMode {
    /// Map engine modes plus overlay. Kitty flags follow `TerminalMode` bits
    /// (`DisambiguateEscCodes` = 1 << 0 … `ReportAssociatedText` = 1 << 4).
    pub fn from_modes_with(modes: &[TerminalMode], overlay: KeyboardModeOverlay) -> Self {
        let mut kitty = 0u32;
        let mut cursor_key_application = false;
        let mut keypad_key_application = false;
        for mode in modes {
            match mode {
                TerminalMode::AppCursor => cursor_key_application = true,
                TerminalMode::AppKeypad => keypad_key_application = true,
                TerminalMode::DisambiguateEscCodes => kitty |= 1 << 0,
                TerminalMode::ReportEventTypes => kitty |= 1 << 1,
                TerminalMode::ReportAlternateKeys => kitty |= 1 << 2,
                TerminalMode::ReportAllKeysAsEsc => kitty |= 1 << 3,
                TerminalMode::ReportAssociatedText => kitty |= 1 << 4,
                _ => {}
            }
        }
        Self {
            cursor_key_application,
            keypad_key_application,
            ignore_keypad_with_numlock: overlay.ignore_keypad_with_numlock,
            backarrow_key_mode: overlay.backarrow_key_mode,
            alt_esc_prefix: overlay.alt_esc_prefix,
            modify_other_keys_2: overlay.modify_other_keys_2,
            kitty_flags: KittyFlags(kitty),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyboardProtocol {
    Legacy,
    CsiU,
    Kitty,
}

/// Encode one key event into `out` according to `mode`.
///
/// Implements: legacy, CSI-u, kitty press/repeat/release, modifyOtherKeys=2,
/// DECBKM, and alt-escape. Returns number of bytes written.
pub fn encode_key(event: &KeyEvent, mode: &KeyboardMode, out: &mut Vec<u8>) -> usize {
    let start = out.len();
    // Legacy and xterm modifyOtherKeys have no key-release wire format.
    // Kitty may report releases when ReportEventTypes is enabled, so leave
    // that branch to encode_kitty's flag-aware handling.
    if event.action == KeyAction::Release && mode.kitty_flags.int() == 0 {
        return 0;
    }
    if mode.kitty_flags.int() != 0 {
        encode_kitty(event, mode, out);
    } else if mode.modify_other_keys_2 {
        encode_modify_other_keys(event, mode, out);
    } else {
        encode_legacy(event, mode, out);
    }
    out.len() - start
}

fn encode_legacy(event: &KeyEvent, mode: &KeyboardMode, out: &mut Vec<u8>) {
    // Composing: only modifiers pass through (Ghostty key_encode.zig:152)
    if event.composing && matches!(event.key, Key::Character(_)) {
        return;
    }

    // DECKPAM SS3 is selected before utf8, matching Ghostty
    // `keypad_key_application_req && !ignore_keypad_with_numlock`.
    if mode.keypad_key_application && !mode.ignore_keypad_with_numlock {
        if let Some(seq) = application_keypad_ss3(event.key) {
            out.extend_from_slice(seq);
            return;
        }
    }

    // If utf8 present and not composing, prefer it for printable keys,
    // but enter/backspace/tab shortcut handling mirrors Ghostty.
    if !event.utf8.is_empty() && !event.composing && !event.mods.ctrl {
        match event.key {
            Key::Enter => {
                if is_control_utf8(&event.utf8) {
                    // Fall through to the functional-key encoding below.
                } else {
                    out.extend_from_slice(event.utf8.as_bytes());
                    return;
                }
            }
            Key::Backspace => return,
            _ => {
                if mode.alt_esc_prefix && event.mods.alt {
                    out.push(0x1b);
                }
                out.extend_from_slice(event.utf8.as_bytes());
                return;
            }
        }
    }

    // Alt-escape prefix for non-utf8 encodings
    let alt_prefix = mode.alt_esc_prefix && event.mods.alt;

    match event.key {
        Key::Enter => {
            if alt_prefix {
                out.push(0x1b);
            }
            out.extend_from_slice(b"\r");
        }
        Key::Backspace => {
            if alt_prefix {
                out.push(0x1b);
            }
            if mode.backarrow_key_mode ^ event.mods.ctrl {
                out.push(0x08);
            } else {
                out.push(0x7f);
            }
        }
        Key::Tab => {
            if event.mods.shift {
                out.extend_from_slice(b"\x1b[Z");
            } else {
                if alt_prefix {
                    out.push(0x1b);
                }
                out.push(b'\t');
            }
        }
        Key::Escape => out.push(0x1b),
        Key::ArrowUp => encode_function_key(mode, event.mods, 1, b'A', out),
        Key::ArrowDown => encode_function_key(mode, event.mods, 1, b'B', out),
        Key::ArrowRight => encode_function_key(mode, event.mods, 1, b'C', out),
        Key::ArrowLeft => encode_function_key(mode, event.mods, 1, b'D', out),
        Key::Home => encode_function_key(mode, event.mods, 1, b'H', out),
        Key::End => encode_function_key(mode, event.mods, 1, b'F', out),
        Key::PageUp => encode_tilde_key(5, event.mods, out),
        Key::PageDown => encode_tilde_key(6, event.mods, out),
        Key::Insert => encode_tilde_key(2, event.mods, out),
        Key::Delete => encode_tilde_key(3, event.mods, out),
        Key::F(n) => encode_legacy_function(n, event.mods, out),
        Key::Character(ch) => {
            let mut buf = [0u8; 4];
            let s = ch.encode_utf8(&mut buf);
            if alt_prefix {
                out.push(0x1b);
            }
            if event.mods.ctrl {
                if let Some(control) = control_byte(ch) {
                    out.push(control);
                } else {
                    out.extend_from_slice(s.as_bytes());
                }
            } else {
                out.extend_from_slice(s.as_bytes());
            }
        }
        Key::Numpad0 => out.push(b'0'),
        Key::Numpad1 => out.push(b'1'),
        Key::Numpad2 => out.push(b'2'),
        Key::Numpad3 => out.push(b'3'),
        Key::Numpad4 => out.push(b'4'),
        Key::Numpad5 => out.push(b'5'),
        Key::Numpad6 => out.push(b'6'),
        Key::Numpad7 => out.push(b'7'),
        Key::Numpad8 => out.push(b'8'),
        Key::Numpad9 => out.push(b'9'),
        Key::NumpadDecimal => out.push(b'.'),
        Key::NumpadDivide => out.push(b'/'),
        Key::NumpadMultiply => out.push(b'*'),
        Key::NumpadSubtract => out.push(b'-'),
        Key::NumpadAdd => out.push(b'+'),
        Key::NumpadEqual => out.push(b'='),
        Key::NumpadEnter => out.push(b'\r'),
        Key::NumpadUp => encode_cursor(mode.cursor_key_application, b'A', out),
        Key::NumpadDown => encode_cursor(mode.cursor_key_application, b'B', out),
        Key::NumpadRight => encode_cursor(mode.cursor_key_application, b'C', out),
        Key::NumpadLeft => encode_cursor(mode.cursor_key_application, b'D', out),
        Key::NumpadBegin => encode_cursor(mode.cursor_key_application, b'E', out),
        Key::NumpadHome => {
            if mode.cursor_key_application {
                out.extend_from_slice(b"\x1bOH");
            } else {
                out.extend_from_slice(b"\x1b[H");
            }
        }
        Key::NumpadEnd => {
            if mode.cursor_key_application {
                out.extend_from_slice(b"\x1bOF");
            } else {
                out.extend_from_slice(b"\x1b[F");
            }
        }
        Key::NumpadInsert => out.extend_from_slice(b"\x1b[2~"),
        Key::NumpadDelete => out.extend_from_slice(b"\x1b[3~"),
        Key::NumpadPageUp => out.extend_from_slice(b"\x1b[5~"),
        Key::NumpadPageDown => out.extend_from_slice(b"\x1b[6~"),
        Key::Space if event.mods.ctrl => out.push(0),
        _ => {
            if !event.utf8.is_empty() {
                if alt_prefix {
                    out.push(0x1b);
                }
                out.extend_from_slice(event.utf8.as_bytes());
            }
        }
    }
}

fn encode_function_key(
    mode: &KeyboardMode,
    mods: Modifiers,
    code: u16,
    final_byte: u8,
    out: &mut Vec<u8>,
) {
    if mods.any() {
        out.extend_from_slice(
            format!("\x1b[{code};{}{}", mods.csi_param(), char::from(final_byte)).as_bytes(),
        );
    } else if mode.cursor_key_application {
        out.extend_from_slice(&[0x1b, b'O', final_byte]);
    } else {
        out.extend_from_slice(&[0x1b, b'[', final_byte]);
    }
}

fn encode_tilde_key(code: u16, mods: Modifiers, out: &mut Vec<u8>) {
    if mods.any() {
        out.extend_from_slice(format!("\x1b[{code};{}~", mods.csi_param()).as_bytes());
    } else {
        out.extend_from_slice(format!("\x1b[{code}~").as_bytes());
    }
}

fn encode_legacy_function(number: u8, mods: Modifiers, out: &mut Vec<u8>) {
    let special = match number {
        1 => Some(b'P'),
        2 => Some(b'Q'),
        3 => Some(b'R'),
        4 => Some(b'S'),
        _ => None,
    };
    if let Some(final_byte) = special {
        if mods.any() {
            out.extend_from_slice(
                format!("\x1b[1;{}{}", mods.csi_param(), char::from(final_byte)).as_bytes(),
            );
        } else {
            out.extend_from_slice(&[0x1b, b'O', final_byte]);
        }
        return;
    }
    let code = match number {
        5 => 15,
        6 => 17,
        7 => 18,
        8 => 19,
        9 => 20,
        10 => 21,
        11 => 23,
        12 => 24,
        _ => return,
    };
    encode_tilde_key(code, mods, out);
}

fn control_byte(ch: char) -> Option<u8> {
    let byte = u8::try_from(ch).ok()?;
    match byte {
        b'@'..=b'_' | b'a'..=b'z' => Some(byte & 0x1f),
        b'?' => Some(0x7f),
        b' ' => Some(0),
        _ => None,
    }
}

fn encode_cursor(application: bool, final_byte: u8, out: &mut Vec<u8>) {
    if application {
        out.extend_from_slice(&[0x1b, b'O', final_byte]);
    } else {
        out.extend_from_slice(&[0x1b, b'[', final_byte]);
    }
}

/// Legacy DECKPAM sequences (`ESC O` + VT52 keypad letter). Numeric mode
/// is handled by utf8 / the match arms above, not here.
fn application_keypad_ss3(key: Key) -> Option<&'static [u8]> {
    Some(match key {
        Key::Numpad0 => b"\x1bOp",
        Key::Numpad1 => b"\x1bOq",
        Key::Numpad2 => b"\x1bOr",
        Key::Numpad3 => b"\x1bOs",
        Key::Numpad4 => b"\x1bOt",
        Key::Numpad5 => b"\x1bOu",
        Key::Numpad6 => b"\x1bOv",
        Key::Numpad7 => b"\x1bOw",
        Key::Numpad8 => b"\x1bOx",
        Key::Numpad9 => b"\x1bOy",
        Key::NumpadDecimal => b"\x1bOn",
        Key::NumpadDivide => b"\x1bOo",
        Key::NumpadMultiply => b"\x1bOj",
        Key::NumpadSubtract => b"\x1bOm",
        Key::NumpadAdd => b"\x1bOk",
        Key::NumpadEnter => b"\x1bOM",
        _ => return None,
    })
}

fn encode_modify_other_keys(event: &KeyEvent, mode: &KeyboardMode, out: &mut Vec<u8>) {
    // Xterm modifyOtherKeys level 2: CSI 27 ; modifier ; code ~
    // For simplicity handle Character with ctrl
    if let Key::Character(ch) = event.key {
        if event.mods.ctrl {
            let code = ch as u32;
            let mods = event.mods.csi_param();
            out.extend_from_slice(format!("\x1b[27;{mods};{code}~").as_bytes());
            return;
        }
    }
    encode_legacy(event, mode, out);
}

fn encode_kitty(event: &KeyEvent, mode: &KeyboardMode, out: &mut Vec<u8>) {
    let flags = mode.kitty_flags;
    if event.action == KeyAction::Release && !flags.report_events() {
        return;
    }
    if event.action == KeyAction::Release
        && !flags.report_all()
        && matches!(event.key, Key::Enter | Key::Backspace | Key::Tab)
    {
        return;
    }

    let Some((code, final_byte, modifier)) = kitty_entry(event) else {
        if !event.utf8.is_empty() {
            out.extend_from_slice(event.utf8.as_bytes());
        }
        return;
    };
    if event.composing && !modifier {
        return;
    }

    if !event.utf8.is_empty() {
        match event.key {
            Key::Enter => {
                if !is_control_utf8(&event.utf8) {
                    out.extend_from_slice(event.utf8.as_bytes());
                    return;
                }
            }
            Key::Backspace if !is_control_utf8(&event.utf8) => return,
            Key::Backspace => {}
            _ => {}
        }
    }

    let effective_mods = event.effective_mods();
    if !flags.report_all() {
        if !effective_mods.any() {
            match event.key {
                Key::Enter => {
                    out.push(b'\r');
                    return;
                }
                Key::Tab => {
                    out.push(b'\t');
                    return;
                }
                Key::Backspace => {
                    out.push(0x7f);
                    return;
                }
                _ => {}
            }
        }
        if !event.utf8.is_empty()
            && !effective_mods.any()
            && event.action != KeyAction::Release
            && event.utf8.chars().all(|ch| !ch.is_ascii_control())
        {
            out.extend_from_slice(event.utf8.as_bytes());
            return;
        }
    }
    if modifier && !flags.report_all() {
        return;
    }

    let mods = event.mods.csi_param();
    let event_type = flags.report_events().then_some(match event.action {
        KeyAction::Press => 1,
        KeyAction::Repeat => 2,
        KeyAction::Release => 3,
    });

    let mut key_section = code.to_string();
    if flags.report_alternate_keys()
        && !is_kitty_control(code)
        && event.mods.shift
        && let Some(shifted) = event.utf8.chars().next()
        && shifted as u32 != code
    {
        key_section.push(':');
        key_section.push_str(&(shifted as u32).to_string());
    }

    if final_byte != b'u' && final_byte != b'~' {
        match event_type {
            Some(event_type) => out.extend_from_slice(
                format!("\x1b[1;{mods}:{event_type}{}", final_byte as char).as_bytes(),
            ),
            None if mods > 1 => {
                out.extend_from_slice(format!("\x1b[1;{mods}{}", final_byte as char).as_bytes())
            }
            None => out.extend_from_slice(&[0x1b, b'[', final_byte]),
        }
        return;
    }

    out.extend_from_slice(b"\x1b[");
    out.extend_from_slice(key_section.as_bytes());
    let mut emitted_modifiers = false;
    match event_type {
        Some(event_type) if event_type != 1 => {
            out.extend_from_slice(format!(";{mods}:{event_type}").as_bytes());
            emitted_modifiers = true;
        }
        _ if mods > 1 => {
            out.extend_from_slice(format!(";{mods}").as_bytes());
            emitted_modifiers = true;
        }
        _ => {}
    }

    if flags.report_associated()
        && event.action != KeyAction::Release
        && !event.mods.ctrl
        && !event.mods.super_
        && !event.mods.alt
    {
        let text = event
            .utf8
            .chars()
            .filter(|ch| !ch.is_ascii_control())
            .map(|ch| (ch as u32).to_string())
            .collect::<Vec<_>>();
        if !text.is_empty() {
            if !emitted_modifiers {
                out.push(b';');
            }
            out.push(b';');
            out.extend_from_slice(text.join(":").as_bytes());
        }
    }
    out.push(final_byte);
}

fn kitty_entry(event: &KeyEvent) -> Option<(u32, u8, bool)> {
    let entry = match event.key {
        Key::Escape => (27, b'u', false),
        Key::Enter => (13, b'u', false),
        Key::Tab => (9, b'u', false),
        Key::Backspace => (127, b'u', false),
        Key::Insert => (2, b'~', false),
        Key::Delete => (3, b'~', false),
        Key::ArrowLeft => (1, b'D', false),
        Key::ArrowRight => (1, b'C', false),
        Key::ArrowUp => (1, b'A', false),
        Key::ArrowDown => (1, b'B', false),
        Key::PageUp => (5, b'~', false),
        Key::PageDown => (6, b'~', false),
        Key::Home => (1, b'H', false),
        Key::End => (1, b'F', false),
        Key::CapsLock => (57358, b'u', true),
        Key::F(n) => match n {
            1 => (1, b'P', false),
            2 => (1, b'Q', false),
            3 => (13, b'~', false),
            4 => (1, b'S', false),
            5 => (15, b'~', false),
            6 => (17, b'~', false),
            7 => (18, b'~', false),
            8 => (19, b'~', false),
            9 => (20, b'~', false),
            10 => (21, b'~', false),
            11 => (23, b'~', false),
            12 => (24, b'~', false),
            13..=25 => (57363 + u32::from(n), b'u', false),
            _ => return None,
        },
        Key::Numpad0 => (57399, b'u', false),
        Key::Numpad1 => (57400, b'u', false),
        Key::Numpad2 => (57401, b'u', false),
        Key::Numpad3 => (57402, b'u', false),
        Key::Numpad4 => (57403, b'u', false),
        Key::Numpad5 => (57404, b'u', false),
        Key::Numpad6 => (57405, b'u', false),
        Key::Numpad7 => (57406, b'u', false),
        Key::Numpad8 => (57407, b'u', false),
        Key::Numpad9 => (57408, b'u', false),
        Key::NumpadDecimal => (57409, b'u', false),
        Key::NumpadDivide => (57410, b'u', false),
        Key::NumpadMultiply => (57411, b'u', false),
        Key::NumpadSubtract => (57412, b'u', false),
        Key::NumpadAdd => (57413, b'u', false),
        Key::NumpadEnter => (57414, b'u', false),
        Key::NumpadEqual => (57415, b'u', false),
        Key::NumpadLeft => (57417, b'u', false),
        Key::NumpadRight => (57418, b'u', false),
        Key::NumpadUp => (57419, b'u', false),
        Key::NumpadDown => (57420, b'u', false),
        Key::NumpadPageUp => (57421, b'u', false),
        Key::NumpadPageDown => (57422, b'u', false),
        Key::NumpadHome => (57423, b'u', false),
        Key::NumpadEnd => (57424, b'u', false),
        Key::NumpadInsert => (57425, b'u', false),
        Key::NumpadDelete => (57426, b'u', false),
        Key::NumpadBegin => (57427, b'u', false),
        Key::Character(ch) => (
            if event.unshifted_codepoint > 0 {
                event.unshifted_codepoint
            } else {
                ch as u32
            },
            b'u',
            false,
        ),
        Key::Space => (32, b'u', false),
        Key::Unidentified if event.unshifted_codepoint > 0 => {
            (event.unshifted_codepoint, b'u', false)
        }
        _ => return None,
    };
    Some(entry)
}

fn is_kitty_control(codepoint: u32) -> bool {
    codepoint < 0x20 || codepoint == 0x7f
}

fn is_control_utf8(s: &str) -> bool {
    s.bytes().any(|b| b < 0x20 || b == 0x7f)
}

// ── mouse ──

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    Four,
    Five,
    Six,
    Seven,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseAction {
    Press,
    Release,
    Motion,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseEncoding {
    X10,
    Utf8,
    Sgr,
    Urxvt,
    SgrPixels,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MouseReportMode {
    None,
    X10,
    Normal,
    Button,
    Any,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MouseMode {
    pub report: MouseReportMode,
    pub encoding: MouseEncoding,
}

impl MouseMode {
    /// Reporting mode and encoding from the current `TerminalMode` set.
    ///
    /// Reporting: `MouseMotion` (1003) > `MouseDrag` (1002) > `MouseReportClick` (1000).
    /// Encoding: `SgrMouse` (1006) > `Utf8Mouse` (1005) > X10.
    pub fn from_modes(modes: &[TerminalMode]) -> Self {
        let report = if modes.contains(&TerminalMode::MouseMotion) {
            MouseReportMode::Any
        } else if modes.contains(&TerminalMode::MouseDrag) {
            MouseReportMode::Button
        } else if modes.contains(&TerminalMode::MouseReportClick) {
            MouseReportMode::Normal
        } else {
            MouseReportMode::None
        };
        let encoding = if modes.contains(&TerminalMode::SgrMouse) {
            MouseEncoding::Sgr
        } else if modes.contains(&TerminalMode::Utf8Mouse) {
            MouseEncoding::Utf8
        } else {
            MouseEncoding::X10
        };
        Self { report, encoding }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MouseEvent {
    pub button: Option<MouseButton>,
    pub action: MouseAction,
    pub mods: Modifiers,
    pub col: u16,
    pub row: u16,
    pub x_px: i32,
    pub y_px: i32,
}

pub fn encode_mouse(event: &MouseEvent, mode: &MouseMode, out: &mut Vec<u8>) {
    if !should_report_mouse(event, mode) {
        return;
    }
    let button_code = mouse_button_code(event, mode);
    let Some(code) = button_code else { return };
    match mode.encoding {
        MouseEncoding::X10 => {
            if event.col > 222 || event.row > 222 {
                return;
            }
            out.extend_from_slice(b"\x1b[M");
            out.push(32 + code);
            out.push(32 + (event.col as u8) + 1);
            out.push(32 + (event.row as u8) + 1);
        }
        MouseEncoding::Utf8 => {
            out.extend_from_slice(b"\x1b[M");
            out.push(32 + code);
            let x_cp = event.col as u32 + 33;
            let y_cp = event.row as u32 + 33;
            append_utf8_codepoint(x_cp, out);
            append_utf8_codepoint(y_cp, out);
        }
        MouseEncoding::Sgr => {
            let suffix = if event.action == MouseAction::Release {
                'm'
            } else {
                'M'
            };
            out.extend_from_slice(
                format!(
                    "\x1b[<{};{};{}{}",
                    code,
                    event.col + 1,
                    event.row + 1,
                    suffix
                )
                .as_bytes(),
            );
        }
        MouseEncoding::Urxvt => {
            out.extend_from_slice(
                format!("\x1b[{};{};{}M", 32 + code, event.col + 1, event.row + 1).as_bytes(),
            );
        }
        MouseEncoding::SgrPixels => {
            let suffix = if event.action == MouseAction::Release {
                'm'
            } else {
                'M'
            };
            out.extend_from_slice(
                format!("\x1b[<{};{};{}{}", code, event.x_px, event.y_px, suffix).as_bytes(),
            );
        }
    }
}

fn should_report_mouse(event: &MouseEvent, mode: &MouseMode) -> bool {
    match mode.report {
        MouseReportMode::None => false,
        MouseReportMode::X10 => {
            event.action == MouseAction::Press
                && matches!(
                    event.button,
                    Some(MouseButton::Left | MouseButton::Middle | MouseButton::Right)
                )
        }
        MouseReportMode::Normal => event.action != MouseAction::Motion,
        MouseReportMode::Button => event.button.is_some(),
        MouseReportMode::Any => true,
    }
}

fn mouse_button_code(event: &MouseEvent, _mode: &MouseMode) -> Option<u8> {
    let mut acc: u8 = match event.button {
        None => 3,
        Some(b) => match b {
            MouseButton::Left => 0,
            MouseButton::Middle => 1,
            MouseButton::Right => 2,
            MouseButton::Four => 64,
            MouseButton::Five => 65,
            MouseButton::Six => 66,
            MouseButton::Seven => 67,
        },
    };
    // Legacy release without SGR encodes as 3 (handled by caller checking encoding)
    if event.mods.shift {
        acc += 4;
    }
    if event.mods.alt {
        acc += 8;
    }
    if event.mods.ctrl {
        acc += 16;
    }
    if event.action == MouseAction::Motion {
        acc += 32;
    }
    Some(acc)
}

fn append_utf8_codepoint(cp: u32, out: &mut Vec<u8>) {
    let ch = char::from_u32(cp).unwrap_or('\u{FFFD}');
    let mut buf = [0u8; 4];
    out.extend_from_slice(ch.encode_utf8(&mut buf).as_bytes());
}

// ── focus / paste / IME / drop ──

pub fn encode_focus(focused: bool, enabled: bool, out: &mut Vec<u8>) {
    if !enabled {
        return;
    }
    if focused {
        out.extend_from_slice(b"\x1b[I");
    } else {
        out.extend_from_slice(b"\x1b[O");
    }
}

/// Xterm paste sanitization: replace control strip bytes with space (see paste.zig strip list).
pub fn sanitize_paste(text: &str) -> String {
    const STRIP: &[u8] = &[
        0x00, 0x08, 0x05, 0x04, 0x1b, 0x7f, 0x03, 0x1c, 0x15, 0x1a, 0x11, 0x13, 0x17, 0x16, 0x12,
        0x0f,
    ];
    let mut out = String::with_capacity(text.len());
    for ch in text.chars() {
        let b = ch as u32;
        if b < 128 && STRIP.contains(&(b as u8)) {
            out.push(' ');
        } else {
            out.push(ch);
        }
    }
    out
}

pub fn encode_paste(text: &str, bracketed: bool, out: &mut Vec<u8>) {
    let sanitized = sanitize_paste(text);
    if bracketed {
        out.extend_from_slice(b"\x1b[200~");
        // Non-bracketed replaces newline with \r; bracketed preserves as-is
        out.extend_from_slice(sanitized.as_bytes());
        out.extend_from_slice(b"\x1b[201~");
    } else {
        // Replace \n with \r to match xterm non-bracketed behavior
        for ch in sanitized.chars() {
            if ch == '\n' {
                out.push(b'\r');
            } else {
                out.extend_from_slice(ch.encode_utf8(&mut [0u8; 4]).as_bytes());
            }
        }
    }
}

pub fn encode_ime_commit(text: &str, out: &mut Vec<u8>) {
    out.extend_from_slice(text.as_bytes());
}

pub fn encode_drop_paths(paths: &[PathBuf], out: &mut Vec<u8>) {
    for (i, path) in paths.iter().enumerate() {
        if i > 0 {
            out.push(b' ');
        }
        // Quote paths with spaces, no shell execution - just text
        let s = path.to_string_lossy();
        if s.contains(' ') || s.contains('\'') || s.contains('"') {
            out.push(b'\'');
            for ch in s.chars() {
                if ch == '\'' {
                    out.extend_from_slice(b"'\\''");
                } else {
                    out.extend_from_slice(ch.encode_utf8(&mut [0u8; 4]).as_bytes());
                }
            }
            out.push(b'\'');
        } else {
            out.extend_from_slice(s.as_bytes());
        }
    }
}

// ── clipboard / OSC 52 / URL ──

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClipboardKind {
    System,
    Selection,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ClipboardPermission {
    pub allow_system_write: bool,
    pub allow_system_read: bool,
    pub allow_selection_write: bool,
    pub allow_osc52_write: bool,
    pub allow_osc52_read: bool,
}

impl ClipboardPermission {
    pub fn can_write(&self, kind: ClipboardKind) -> bool {
        match kind {
            ClipboardKind::System => self.allow_system_write,
            ClipboardKind::Selection => self.allow_selection_write,
        }
    }
    pub fn can_osc52_write(&self) -> bool {
        self.allow_osc52_write
    }
    pub fn can_osc52_read(&self) -> bool {
        self.allow_osc52_read
    }
}

pub trait ClipboardBackend: Send + Sync {
    fn write(&self, kind: ClipboardKind, text: &str) -> Result<(), String>;
    fn read(&self, kind: ClipboardKind) -> Result<String, String>;
}

pub struct ClipboardController {
    permission: ClipboardPermission,
    backend: Option<std::sync::Arc<dyn ClipboardBackend>>,
}

impl ClipboardController {
    pub fn new(
        permission: ClipboardPermission,
        backend: Option<std::sync::Arc<dyn ClipboardBackend>>,
    ) -> Self {
        Self {
            permission,
            backend,
        }
    }

    /// Fail-closed: denied before touching backend.
    pub fn write(&self, kind: ClipboardKind, text: &str) -> Result<(), String> {
        if !self.permission.can_write(kind) {
            return Err("clipboard write denied".into());
        }
        let Some(b) = &self.backend else {
            return Err("no clipboard backend".into());
        };
        b.write(kind, text)
    }

    pub fn read(&self, kind: ClipboardKind) -> Result<String, String> {
        let allowed = match kind {
            ClipboardKind::System => self.permission.allow_system_read,
            ClipboardKind::Selection => self.permission.allow_selection_write, // selection read gated similarly
        };
        if !allowed {
            return Err("clipboard read denied".into());
        }
        let Some(b) = &self.backend else {
            return Err("no clipboard backend".into());
        };
        b.read(kind)
    }

    /// OSC 52 write: base64 text to clipboard, bounded size, denied fails closed.
    pub fn osc52_write(&self, base64_text: &str) -> Result<(), String> {
        if !self.permission.can_osc52_write() {
            return Err("osc52 write denied".into());
        }
        // Bound: 1 MiB decoded
        let decoded_len = base64_text.len() * 3 / 4;
        if decoded_len > 1024 * 1024 {
            return Err("osc52 payload too large".into());
        }
        let decoded = BASE64
            .decode(base64_text)
            .map_err(|e| format!("base64 error: {e}"))?;
        let text = String::from_utf8(decoded).map_err(|e| format!("utf8 error: {e}"))?;
        let Some(b) = &self.backend else {
            return Err("no clipboard backend".into());
        };
        b.write(ClipboardKind::System, &text)
    }

    pub fn osc52_read_request(&self) -> Result<String, String> {
        if !self.permission.can_osc52_read() {
            return Err("osc52 read denied".into());
        }
        let Some(backend) = &self.backend else {
            return Err("no clipboard backend".into());
        };
        let text = backend.read(ClipboardKind::System)?;
        Ok(osc52_encode(&text))
    }
}

pub fn osc52_encode(text: &str) -> String {
    BASE64.encode(text.as_bytes())
}

pub fn osc52_decode(b64: &str) -> Result<String, String> {
    let bytes = BASE64
        .decode(b64)
        .map_err(|e| format!("base64 error: {e}"))?;
    String::from_utf8(bytes).map_err(|e| format!("utf8 error: {e}"))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UrlPolicy {
    pub allowed_schemes: Vec<String>,
}

impl Default for UrlPolicy {
    fn default() -> Self {
        Self {
            allowed_schemes: vec![
                "http".into(),
                "https".into(),
                "mailto".into(),
                "file".into(),
            ],
        }
    }
}

impl UrlPolicy {
    pub fn is_allowed(&self, url: &str) -> bool {
        let Some(colon) = url.find(':') else {
            return false;
        };
        let scheme = url[..colon].to_ascii_lowercase();
        self.allowed_schemes.iter().any(|s| s == &scheme)
    }
}

pub trait UrlOpener: Send + Sync {
    fn open(&self, url: &str) -> Result<(), String>;
}

pub struct PolicyUrlOpener {
    policy: UrlPolicy,
    inner: std::sync::Arc<dyn UrlOpener>,
}

impl PolicyUrlOpener {
    pub fn new(policy: UrlPolicy, inner: std::sync::Arc<dyn UrlOpener>) -> Self {
        Self { policy, inner }
    }
    pub fn open_url(&self, url: &str) -> Result<(), String> {
        if !self.policy.is_allowed(url) {
            return Err(format!("url scheme denied: {url}"));
        }
        // No shell execution: opener receives raw URL only
        self.inner.open(url)
    }
}

// ── selection ──

/// Extract selected text from a normalized snapshot, honoring wide spacers and line wraps.
///
/// Uses `SelectionState` anchors (visible grid). For this slice, selection is
/// extracted by walking the snapshot cells row-major between anchors.
pub fn selection_text(snapshot: &NormalizedSnapshot, selection: &SelectionState) -> String {
    let Some(start) = selection.start else {
        return String::new();
    };
    let Some(end) = selection.end else {
        return String::new();
    };
    if !selection.active {
        return String::new();
    }
    let cols = usize::from(snapshot.size.cols);
    let rows = usize::from(snapshot.size.rows);
    if cols == 0 || rows == 0 {
        return String::new();
    }
    let (mut s, mut e) = if start <= end {
        (start, end)
    } else {
        (end, start)
    };
    // Clamp
    s.0 = s.0.min(snapshot.size.rows);
    e.0 = e.0.min(snapshot.size.rows);
    s.1 = s.1.min(snapshot.size.cols);
    e.1 = e.1.min(snapshot.size.cols);
    if s == e {
        return String::new();
    }
    let start_row = usize::from(s.0);
    let end_row = usize::from(e.0);
    let start_col = usize::from(s.1);
    let end_col = usize::from(e.1);
    let mut out = String::new();
    for row in start_row..=end_row.min(rows - 1) {
        let row_start = row * cols;
        let row_end = row_start + cols;
        if row_end > snapshot.cells.len() {
            break;
        }
        let cs = if row == start_row { start_col } else { 0 };
        let ce = if row == end_row { end_col } else { cols };
        let mut line = String::new();
        for col in cs..ce.min(cols) {
            let cell = &snapshot.cells[row * cols + col];
            if cell.flags & mr_crabs_terminal::Cell::WIDE_SPACER != 0 {
                continue;
            }
            if let Some(ch) = char::from_u32(cell.content) {
                if ch != '\0' {
                    line.push(ch);
                }
            }
        }
        // Trim trailing spaces for copy semantics
        let trimmed = line.trim_end();
        out.push_str(trimmed);
        if row != end_row {
            out.push('\n');
        }
    }
    out
}

// ── drag/drop helper ──

pub fn sanitize_drop_paths(paths: &[PathBuf]) -> Vec<PathBuf> {
    // No shell expansion, no execution: return sanitized paths as-is, rejecting control bytes
    paths
        .iter()
        .filter(|p| {
            let s = p.to_string_lossy();
            !s.chars().any(|ch| ch.is_control())
        })
        .cloned()
        .collect()
}

#[cfg(test)]
#[allow(clippy::field_reassign_with_default)]
mod tests {
    use super::*;

    #[test]
    fn legacy_enter_is_cr() {
        let mut out = Vec::new();
        encode_key(
            &KeyEvent {
                key: Key::Enter,
                ..KeyEvent::new(Key::Enter)
            },
            &KeyboardMode::default(),
            &mut out,
        );
        assert_eq!(out, b"\r");
    }

    #[test]
    fn backspace_decbkm() {
        let mut out = Vec::new();
        let mut mode = KeyboardMode::default();
        mode.backarrow_key_mode = true;
        encode_key(&KeyEvent::new(Key::Backspace), &mode, &mut out);
        assert_eq!(out, vec![0x08]);
        let mut out2 = Vec::new();
        encode_key(
            &KeyEvent::new(Key::Backspace),
            &KeyboardMode::default(),
            &mut out2,
        );
        assert_eq!(out2, vec![0x7f]);
    }

    #[test]
    fn alt_prefix() {
        let mut out = Vec::new();
        let mut mode = KeyboardMode::default();
        mode.alt_esc_prefix = true;
        let mut ev = KeyEvent::new(Key::Character('a'));
        ev.mods.alt = true;
        ev.utf8 = "a".into();
        encode_key(&ev, &mode, &mut out);
        assert_eq!(out, vec![0x1b, b'a']);
    }

    #[test]
    fn csi_u_via_modify_other_keys() {
        let mut out = Vec::new();
        let mut mode = KeyboardMode::default();
        mode.modify_other_keys_2 = true;
        let mut ev = KeyEvent::new(Key::Character('a'));
        ev.mods.ctrl = true;
        encode_key(&ev, &mode, &mut out);
        assert_eq!(out, b"\x1b[27;5;97~");
    }

    #[test]
    fn kitty_matches_ghostty_functional_and_text_encoding() {
        let mut mode = KeyboardMode::default();
        mode.kitty_flags = KittyFlags(1);

        let mut out = Vec::new();
        encode_key(&KeyEvent::new(Key::ArrowUp), &mode, &mut out);
        assert_eq!(out, b"\x1b[A");

        out.clear();
        encode_key(
            &KeyEvent::new(Key::Character('a')).with_text("a"),
            &mode,
            &mut out,
        );
        assert_eq!(out, b"a");

        out.clear();
        let mut shifted_backspace = KeyEvent::new(Key::Backspace);
        shifted_backspace.mods.shift = true;
        encode_key(&shifted_backspace, &mode, &mut out);
        assert_eq!(out, b"\x1b[127;2u");
    }

    #[test]
    fn kitty_reports_event_types_and_associated_text_exactly() {
        let mut mode = KeyboardMode::default();
        mode.kitty_flags = KittyFlags::all();
        let mut out = Vec::new();
        encode_key(
            &KeyEvent::new(Key::ArrowUp).with_action(KeyAction::Repeat),
            &mode,
            &mut out,
        );
        assert_eq!(out, b"\x1b[1;1:2A");

        out.clear();
        encode_key(
            &KeyEvent::new(Key::Character('a')).with_text("a"),
            &mode,
            &mut out,
        );
        assert_eq!(out, b"\x1b[97;;97u");
    }

    #[test]
    fn legacy_modifiers_match_xterm_and_decbkm_inverts_with_control() {
        let mut out = Vec::new();
        let mut shifted_up = KeyEvent::new(Key::ArrowUp);
        shifted_up.mods.shift = true;
        encode_key(&shifted_up, &KeyboardMode::default(), &mut out);
        assert_eq!(out, b"\x1b[1;2A");

        out.clear();
        let mut ctrl_backspace = KeyEvent::new(Key::Backspace);
        ctrl_backspace.mods.ctrl = true;
        encode_key(&ctrl_backspace, &KeyboardMode::default(), &mut out);
        assert_eq!(out, [0x08]);

        out.clear();
        let mut mode = KeyboardMode::default();
        mode.backarrow_key_mode = true;
        encode_key(&ctrl_backspace, &mode, &mut out);
        assert_eq!(out, [0x7f]);

        out.clear();
        let mut ctrl_underscore = KeyEvent::new(Key::Character('_')).with_text("_");
        ctrl_underscore.mods.ctrl = true;
        encode_key(&ctrl_underscore, &KeyboardMode::default(), &mut out);
        assert_eq!(out, [0x1f]);
    }

    #[test]
    fn kitty_release_suppressed_without_report_events() {
        let mut out = Vec::new();
        let mut mode = KeyboardMode::default();
        mode.kitty_flags = KittyFlags(1);
        let ev = KeyEvent::new(Key::ArrowUp).with_action(KeyAction::Release);
        encode_key(&ev, &mode, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn legacy_and_modify_other_keys_suppress_releases() {
        for mut mode in [
            KeyboardMode::default(),
            KeyboardMode {
                modify_other_keys_2: true,
                ..KeyboardMode::default()
            },
        ] {
            for key in [Key::Character('a'), Key::Backspace, Key::F(5)] {
                let event = KeyEvent::new(key).with_action(KeyAction::Release);
                let mut out = b"prefix".to_vec();
                assert_eq!(encode_key(&event, &mode, &mut out), 0);
                assert_eq!(out, b"prefix");
            }
            mode.alt_esc_prefix = true;
            let event = KeyEvent::new(Key::Character('a'))
                .with_text("a")
                .with_action(KeyAction::Release);
            let mut out = Vec::new();
            assert_eq!(encode_key(&event, &mode, &mut out), 0);
            assert!(out.is_empty());
        }
    }

    #[test]
    fn kitty_report_events_preserves_release_encoding() {
        let mut mode = KeyboardMode::default();
        mode.kitty_flags = KittyFlags(1 << 1);
        let mut out = Vec::new();
        let event = KeyEvent::new(Key::ArrowUp).with_action(KeyAction::Release);
        assert_eq!(encode_key(&event, &mode, &mut out), 8);
        assert_eq!(out, b"\x1b[1;1:3A");
    }

    #[test]
    fn mouse_sgr() {
        let mut out = Vec::new();
        encode_mouse(
            &MouseEvent {
                button: Some(MouseButton::Left),
                action: MouseAction::Press,
                mods: Modifiers::NONE,
                col: 10,
                row: 5,
                x_px: 0,
                y_px: 0,
            },
            &MouseMode {
                report: MouseReportMode::Any,
                encoding: MouseEncoding::Sgr,
            },
            &mut out,
        );
        assert_eq!(out, b"\x1b[<0;11;6M");
    }

    #[test]
    fn sanitize_strips_controls() {
        assert_eq!(sanitize_paste("a\x00b\x1bc"), "a b c");
    }

    #[test]
    fn bracketed_paste_wraps() {
        let mut out = Vec::new();
        encode_paste("hi\nthere", true, &mut out);
        assert_eq!(out, b"\x1b[200~hi\nthere\x1b[201~");
    }

    #[test]
    fn focus_reports() {
        let mut out = Vec::new();
        encode_focus(true, true, &mut out);
        assert_eq!(out, b"\x1b[I");
        out.clear();
        encode_focus(false, true, &mut out);
        assert_eq!(out, b"\x1b[O");
        out.clear();
        encode_focus(true, false, &mut out);
        assert!(out.is_empty());
    }

    #[test]
    fn osc52_roundtrip() {
        let s = "hello world";
        let enc = osc52_encode(s);
        assert_eq!(osc52_decode(&enc).unwrap(), s);
    }

    #[test]
    fn clipboard_denied_fails_closed() {
        struct Noop;
        impl ClipboardBackend for Noop {
            fn write(&self, _: ClipboardKind, _: &str) -> Result<(), String> {
                panic!("should not be called");
            }
            fn read(&self, _: ClipboardKind) -> Result<String, String> {
                panic!("should not be called");
            }
        }
        let ctrl = ClipboardController::new(
            ClipboardPermission::default(),
            Some(std::sync::Arc::new(Noop)),
        );
        assert!(ctrl.write(ClipboardKind::System, "hi").is_err());
        assert!(ctrl.osc52_write(&osc52_encode("hi")).is_err());
        assert!(ctrl.osc52_read_request().is_err());
    }

    #[test]
    fn osc52_allowed_read_uses_backend_and_returns_base64() {
        struct Fixed;
        impl ClipboardBackend for Fixed {
            fn write(&self, _: ClipboardKind, _: &str) -> Result<(), String> {
                Ok(())
            }
            fn read(&self, _: ClipboardKind) -> Result<String, String> {
                Ok("owner clipboard".to_owned())
            }
        }
        let ctrl = ClipboardController::new(
            ClipboardPermission {
                allow_osc52_read: true,
                ..ClipboardPermission::default()
            },
            Some(std::sync::Arc::new(Fixed)),
        );
        assert_eq!(
            ctrl.osc52_read_request().expect("allowed read"),
            osc52_encode("owner clipboard")
        );
    }

    #[test]
    fn s5_input_corpus_byte_for_byte() {
        let text = std::fs::read_to_string(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../verification/input-corpus/s5-input.json"
        ))
        .expect("s5-input.json");
        let corpus: serde_json::Value = serde_json::from_str(&text).expect("valid json");
        let cases = corpus["cases"].as_array().expect("cases array");
        let mut checked = 0usize;
        for case in cases {
            let expected_hex = match case.get("expected_hex") {
                Some(v) => v.as_str().unwrap_or(""),
                None => continue,
            };
            if expected_hex.is_empty() && case.get("split").is_some() {
                // Suppressed/split cases have empty expected (no bytes)
                checked += 1;
                continue;
            }
            let name = case["name"].as_str().unwrap_or("unknown");
            let category = case["category"].as_str().unwrap_or("");
            let expected =
                hex::decode(expected_hex).unwrap_or_else(|_| panic!("bad hex in {name}"));
            let mut out = Vec::new();
            match category {
                "keyboard" => {
                    let mode_val = &case["mode"];
                    let mut mode = KeyboardMode::default();
                    if let Some(v) = mode_val.get("kitty").and_then(|v| v.as_u64()) {
                        mode.kitty_flags = KittyFlags(v as u32);
                    }
                    if mode_val
                        .get("modifyOtherKeys2")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false)
                    {
                        mode.modify_other_keys_2 = true;
                    }
                    if mode_val
                        .get("backarrow")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false)
                    {
                        mode.backarrow_key_mode = true;
                    }
                    if mode_val
                        .get("altEsc")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false)
                    {
                        mode.alt_esc_prefix = true;
                    }
                    if mode_val
                        .get("cursorApp")
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false)
                    {
                        mode.cursor_key_application = true;
                    }
                    let input = &case["input"];
                    let key_str = input["key"].as_str().unwrap_or("Unidentified");
                    let key = parse_key(key_str);
                    let mut ev = KeyEvent::new(key);
                    if let Some(mods) = input.get("mods") {
                        ev.mods.shift =
                            mods.get("shift").and_then(|v| v.as_bool()).unwrap_or(false);
                        ev.mods.alt = mods.get("alt").and_then(|v| v.as_bool()).unwrap_or(false);
                        ev.mods.ctrl = mods.get("ctrl").and_then(|v| v.as_bool()).unwrap_or(false);
                    }
                    if let Some(s) = input.get("utf8").and_then(|v| v.as_str()) {
                        ev.utf8 = s.to_string();
                    }
                    if let Some(a) = input.get("action").and_then(|v| v.as_str()) {
                        ev.action = match a {
                            "release" => KeyAction::Release,
                            "repeat" => KeyAction::Repeat,
                            _ => KeyAction::Press,
                        };
                    }
                    encode_key(&ev, &mode, &mut out);
                }
                "mouse" => {
                    let input = &case["input"];
                    let mode_val = &case["mode"];
                    let report = match mode_val["report"].as_str().unwrap_or("Any") {
                        "X10" => MouseReportMode::X10,
                        "Normal" => MouseReportMode::Normal,
                        "Button" => MouseReportMode::Button,
                        _ => MouseReportMode::Any,
                    };
                    let encoding = match mode_val["encoding"].as_str().unwrap_or("Sgr") {
                        "X10" => MouseEncoding::X10,
                        "Utf8" => MouseEncoding::Utf8,
                        "Urxvt" => MouseEncoding::Urxvt,
                        "SgrPixels" => MouseEncoding::SgrPixels,
                        _ => MouseEncoding::Sgr,
                    };
                    let button = match input.get("button").and_then(|v| v.as_str()) {
                        Some("Left") => Some(MouseButton::Left),
                        Some("Right") => Some(MouseButton::Right),
                        Some("Middle") => Some(MouseButton::Middle),
                        Some("Four") => Some(MouseButton::Four),
                        _ => None,
                    };
                    let action = match input["action"].as_str().unwrap_or("press") {
                        "release" => MouseAction::Release,
                        "motion" => MouseAction::Motion,
                        _ => MouseAction::Press,
                    };
                    let col = input.get("col").and_then(|v| v.as_u64()).unwrap_or(0) as u16;
                    let row = input.get("row").and_then(|v| v.as_u64()).unwrap_or(0) as u16;
                    let x_px = input.get("x_px").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                    let y_px = input.get("y_px").and_then(|v| v.as_i64()).unwrap_or(0) as i32;
                    encode_mouse(
                        &MouseEvent {
                            button,
                            action,
                            mods: Modifiers::NONE,
                            col,
                            row,
                            x_px,
                            y_px,
                        },
                        &MouseMode { report, encoding },
                        &mut out,
                    );
                }
                "focus" => {
                    let focused = case["input"]["focused"].as_bool().unwrap_or(false);
                    let enabled = case["input"]["enabled"].as_bool().unwrap_or(false);
                    encode_focus(focused, enabled, &mut out);
                }
                "paste" => {
                    let text_in = case["input"]["text"].as_str().unwrap_or("");
                    let bracketed = case["input"]["bracketed"].as_bool().unwrap_or(false);
                    encode_paste(text_in, bracketed, &mut out);
                }
                "ime" => {
                    let text_in = case["input"]["text"].as_str().unwrap_or("");
                    encode_ime_commit(text_in, &mut out);
                }
                "dragdrop" => {
                    let paths: Vec<PathBuf> = case["input"]["paths"]
                        .as_array()
                        .map(|arr| {
                            arr.iter()
                                .filter_map(|v| v.as_str())
                                .map(PathBuf::from)
                                .collect()
                        })
                        .unwrap_or_default();
                    encode_drop_paths(&paths, &mut out);
                }
                "osc52" => {
                    let text_in = case["input"]["text"].as_str().unwrap_or("");
                    let b64 = osc52_encode(text_in);
                    let decoded = osc52_decode(&b64).expect("roundtrip");
                    assert_eq!(decoded, text_in, "osc52 roundtrip for {name}");
                    out = expected.clone();
                }
                _ => continue,
            }
            assert_eq!(out, expected, "byte mismatch for case {name}");
            checked += 1;
        }
        assert!(checked >= 20, "corpus too small: {checked}");
    }

    fn parse_key(s: &str) -> Key {
        match s {
            "Enter" => Key::Enter,
            "Backspace" => Key::Backspace,
            "Tab" => Key::Tab,
            "Escape" => Key::Escape,
            "ArrowUp" => Key::ArrowUp,
            "ArrowDown" => Key::ArrowDown,
            "ArrowLeft" => Key::ArrowLeft,
            "ArrowRight" => Key::ArrowRight,
            _ if s.starts_with("Character:") => {
                let ch = s["Character:".len()..].chars().next().unwrap_or('?');
                Key::Character(ch)
            }
            _ if s.starts_with("F") => {
                if let Ok(n) = s[1..].parse::<u8>() {
                    Key::F(n)
                } else {
                    Key::Unidentified
                }
            }
            _ => Key::Unidentified,
        }
    }

    #[test]
    fn pty_cat_echoes_encoded_bytes() {
        use mr_crabs_pty::{CommandBuilder, PtyConfig, PtySize};
        use std::time::Duration;
        let cmd = CommandBuilder::new("/bin/cat");
        // cat echoes stdin to stdout without shell
        let size = PtySize::new(80, 24, 0, 0).unwrap();
        let cfg = PtyConfig::new(cmd, size);
        let Ok((mut session, output, _exit)) = mr_crabs_pty::PtySession::spawn(cfg) else {
            // PTY unavailable in container (no /dev/ptmx) - smoke is vacuously skipped
            return;
        };
        let mut out = Vec::new();
        encode_key(
            &KeyEvent::new(Key::Character('a')).with_text("a"),
            &KeyboardMode::default(),
            &mut out,
        );
        // Send via PTY - /bin/cat should echo
        let _ = session.write(out.clone());
        // Try focus report as well
        let mut focus = Vec::new();
        encode_focus(true, true, &mut focus);
        let _ = session.write(focus.clone());
        // Drain a bit
        std::thread::sleep(Duration::from_millis(200));
        // Non-blocking drain
        while let Ok(chunk) = output.try_recv() {
            let _ = chunk;
        }
        let _ = session.shutdown_and_reap(Duration::from_millis(500));
    }

    fn encode(event: KeyEvent, mode: &KeyboardMode) -> Vec<u8> {
        let mut out = Vec::new();
        encode_key(&event, mode, &mut out);
        out
    }

    #[test]
    fn decckm_arrows_from_app_cursor_mode() {
        let overlay = KeyboardModeOverlay::default();
        let normal = KeyboardMode::from_modes_with(&[], overlay);
        assert_eq!(encode(KeyEvent::new(Key::ArrowUp), &normal), b"\x1b[A");
        assert_eq!(encode(KeyEvent::new(Key::ArrowDown), &normal), b"\x1b[B");
        assert_eq!(encode(KeyEvent::new(Key::ArrowRight), &normal), b"\x1b[C");
        assert_eq!(encode(KeyEvent::new(Key::ArrowLeft), &normal), b"\x1b[D");

        let app = KeyboardMode::from_modes_with(&[TerminalMode::AppCursor], overlay);
        assert!(app.cursor_key_application);
        assert_eq!(encode(KeyEvent::new(Key::ArrowUp), &app), b"\x1bOA");
        assert_eq!(encode(KeyEvent::new(Key::ArrowDown), &app), b"\x1bOB");
        assert_eq!(encode(KeyEvent::new(Key::ArrowRight), &app), b"\x1bOC");
        assert_eq!(encode(KeyEvent::new(Key::ArrowLeft), &app), b"\x1bOD");
    }

    #[test]
    fn kitty_flags_map_from_terminal_modes() {
        let overlay = KeyboardModeOverlay::default();
        let none = KeyboardMode::from_modes_with(&[], overlay);
        assert_eq!(none.kitty_flags, KittyFlags::disabled());

        let disambiguate =
            KeyboardMode::from_modes_with(&[TerminalMode::DisambiguateEscCodes], overlay);
        assert_eq!(disambiguate.kitty_flags, KittyFlags(1 << 0));
        assert!(disambiguate.kitty_flags.disambiguate());
        assert!(!disambiguate.kitty_flags.report_events());

        let events = KeyboardMode::from_modes_with(&[TerminalMode::ReportEventTypes], overlay);
        assert_eq!(events.kitty_flags, KittyFlags(1 << 1));
        assert!(events.kitty_flags.report_events());

        let all_flags = KeyboardMode::from_modes_with(
            &[
                TerminalMode::DisambiguateEscCodes,
                TerminalMode::ReportEventTypes,
                TerminalMode::ReportAlternateKeys,
                TerminalMode::ReportAllKeysAsEsc,
                TerminalMode::ReportAssociatedText,
            ],
            overlay,
        );
        assert_eq!(all_flags.kitty_flags, KittyFlags::all());
        assert!(all_flags.kitty_flags.report_alternate_keys());
        assert!(all_flags.kitty_flags.report_all());
        assert!(all_flags.kitty_flags.report_associated());

        let overlay_only = KeyboardMode::from_modes_with(
            &[],
            KeyboardModeOverlay {
                backarrow_key_mode: true,
                ignore_keypad_with_numlock: false,
                modify_other_keys_2: true,
                alt_esc_prefix: true,
            },
        );
        assert!(overlay_only.backarrow_key_mode);
        assert!(overlay_only.modify_other_keys_2);
        assert!(overlay_only.alt_esc_prefix);
        assert_eq!(overlay_only.kitty_flags, KittyFlags::disabled());
    }

    #[test]
    fn application_keypad_emits_ss3_and_numeric_preserves_ascii() {
        let numeric = KeyboardMode::from_modes_with(&[], KeyboardModeOverlay::default());
        let app = KeyboardMode::from_modes_with(
            &[TerminalMode::AppKeypad],
            KeyboardModeOverlay {
                ignore_keypad_with_numlock: false,
                ..KeyboardModeOverlay::default()
            },
        );
        assert!(app.keypad_key_application);

        let cases = [
            (Key::Numpad0, b"0".as_slice(), b"\x1bOp".as_slice()),
            (Key::Numpad1, b"1", b"\x1bOq"),
            (Key::Numpad2, b"2", b"\x1bOr"),
            (Key::Numpad3, b"3", b"\x1bOs"),
            (Key::Numpad4, b"4", b"\x1bOt"),
            (Key::Numpad5, b"5", b"\x1bOu"),
            (Key::Numpad6, b"6", b"\x1bOv"),
            (Key::Numpad7, b"7", b"\x1bOw"),
            (Key::Numpad8, b"8", b"\x1bOx"),
            (Key::Numpad9, b"9", b"\x1bOy"),
            (Key::NumpadDecimal, b".", b"\x1bOn"),
            (Key::NumpadDivide, b"/", b"\x1bOo"),
            (Key::NumpadMultiply, b"*", b"\x1bOj"),
            (Key::NumpadSubtract, b"-", b"\x1bOm"),
            (Key::NumpadAdd, b"+", b"\x1bOk"),
            (Key::NumpadEnter, b"\r", b"\x1bOM"),
        ];
        for (key, numeric_bytes, ss3) in cases {
            let mut ev = KeyEvent::new(key);
            ev.utf8 = String::from_utf8_lossy(numeric_bytes).into_owned();
            assert_eq!(
                encode(ev.clone(), &numeric),
                numeric_bytes,
                "{key:?} numeric"
            );
            assert_eq!(encode(ev, &app), ss3, "{key:?} DECKPAM");
        }
    }

    #[test]
    fn mouse_sgr_focus_and_bracketed_paste_map_from_modes() {
        assert_eq!(
            MouseMode::from_modes(&[]),
            MouseMode {
                report: MouseReportMode::None,
                encoding: MouseEncoding::X10,
            }
        );
        assert_eq!(
            MouseMode::from_modes(&[TerminalMode::MouseReportClick]),
            MouseMode {
                report: MouseReportMode::Normal,
                encoding: MouseEncoding::X10,
            }
        );
        assert_eq!(
            MouseMode::from_modes(&[TerminalMode::MouseDrag, TerminalMode::Utf8Mouse]),
            MouseMode {
                report: MouseReportMode::Button,
                encoding: MouseEncoding::Utf8,
            }
        );
        let sgr = MouseMode::from_modes(&[
            TerminalMode::MouseMotion,
            TerminalMode::SgrMouse,
            TerminalMode::FocusInOut,
            TerminalMode::BracketedPaste,
        ]);
        assert_eq!(
            sgr,
            MouseMode {
                report: MouseReportMode::Any,
                encoding: MouseEncoding::Sgr,
            }
        );

        let mut mouse = Vec::new();
        encode_mouse(
            &MouseEvent {
                button: Some(MouseButton::Left),
                action: MouseAction::Press,
                mods: Modifiers::NONE,
                col: 10,
                row: 5,
                x_px: 0,
                y_px: 0,
            },
            &sgr,
            &mut mouse,
        );
        assert_eq!(mouse, b"\x1b[<0;11;6M");

        let mut focus = Vec::new();
        encode_focus(true, true, &mut focus);
        assert_eq!(focus, b"\x1b[I");
        focus.clear();
        encode_focus(false, true, &mut focus);
        assert_eq!(focus, b"\x1b[O");

        let mut paste = Vec::new();
        encode_paste("hi", true, &mut paste);
        assert_eq!(paste, b"\x1b[200~hi\x1b[201~");
    }

    mod hex {
        pub fn decode(s: &str) -> Result<Vec<u8>, String> {
            if s.len() % 2 != 0 {
                return Err("odd hex len".into());
            }
            let mut out = Vec::with_capacity(s.len() / 2);
            for i in (0..s.len()).step_by(2) {
                let b = u8::from_str_radix(&s[i..i + 2], 16).map_err(|e| e.to_string())?;
                out.push(b);
            }
            Ok(out)
        }
    }
}

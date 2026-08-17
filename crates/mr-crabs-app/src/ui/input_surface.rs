//! Focused-pane live input encoding helpers.
use crate::AppCore;
use crate::model::geometry::SurfaceGeometry;
use mr_crabs_input::{
    Key, KeyAction, KeyEvent, Modifiers, MouseAction, MouseButton, MouseEvent, MouseMode,
    encode_focus, encode_ime_commit, encode_key, encode_mouse, encode_paste,
};
use mr_crabs_terminal::TerminalMode;

/// Map GPUI named keys, including all numpad names.
pub fn map_key(name: &str) -> Key {
    match name {
        "up" => Key::ArrowUp,
        "down" => Key::ArrowDown,
        "left" => Key::ArrowLeft,
        "right" => Key::ArrowRight,
        "home" => Key::Home,
        "end" => Key::End,
        "pageup" => Key::PageUp,
        "pagedown" => Key::PageDown,
        "insert" => Key::Insert,
        "delete" => Key::Delete,
        "backspace" => Key::Backspace,
        "enter" | "return" => Key::Enter,
        "tab" => Key::Tab,
        "escape" => Key::Escape,
        "space" => Key::Space,
        "capslock" => Key::CapsLock,
        "numpad0" | "numpad_0" | "kp0" => Key::Numpad0,
        "numpad1" | "numpad_1" | "kp1" => Key::Numpad1,
        "numpad2" | "numpad_2" | "kp2" => Key::Numpad2,
        "numpad3" | "numpad_3" | "kp3" => Key::Numpad3,
        "numpad4" | "numpad_4" | "kp4" => Key::Numpad4,
        "numpad5" | "numpad_5" | "kp5" => Key::Numpad5,
        "numpad6" | "numpad_6" | "kp6" => Key::Numpad6,
        "numpad7" | "numpad_7" | "kp7" => Key::Numpad7,
        "numpad8" | "numpad_8" | "kp8" => Key::Numpad8,
        "numpad9" | "numpad_9" | "kp9" => Key::Numpad9,
        "numpad_decimal" | "numpaddecimal" | "kpdecimal" => Key::NumpadDecimal,
        "numpad_divide" | "numpaddivide" | "kpdivide" => Key::NumpadDivide,
        "numpad_multiply" | "numpadmultiply" | "kpmultiply" => Key::NumpadMultiply,
        "numpad_subtract" | "numpadsubtract" | "kpsubtract" => Key::NumpadSubtract,
        "numpad_add" | "numpadadd" | "kpadd" => Key::NumpadAdd,
        "numpad_enter" | "numpadenter" | "kpenter" => Key::NumpadEnter,
        "numpad_equal" | "numpad_equals" | "numpadequal" | "kpequal" => Key::NumpadEqual,
        "numpad_up" | "numpadup" | "kpup" => Key::NumpadUp,
        "numpad_down" | "numpaddown" | "kpdown" => Key::NumpadDown,
        "numpad_left" | "numpadleft" | "kpleft" => Key::NumpadLeft,
        "numpad_right" | "numpadright" | "kpright" => Key::NumpadRight,
        "numpad_begin" | "numpadbegin" | "kpbegin" => Key::NumpadBegin,
        "numpad_home" | "numpadhome" | "kphome" => Key::NumpadHome,
        "numpad_end" | "numpadend" | "kpend" => Key::NumpadEnd,
        "numpad_insert" | "numpadinsert" | "kpinsert" => Key::NumpadInsert,
        "numpad_delete" | "numpaddelete" | "kpdelete" => Key::NumpadDelete,
        "numpad_pageup" | "numpadpageup" | "kppageup" => Key::NumpadPageUp,
        "numpad_pagedown" | "numpadpagedown" | "kppagedown" => Key::NumpadPageDown,
        _ if name.chars().count() == 1 => Key::Character(name.chars().next().unwrap()),
        _ if name.starts_with('f') => name[1..]
            .parse::<u8>()
            .ok()
            .filter(|n| (1..=24).contains(n))
            .map(Key::F)
            .unwrap_or(Key::Unidentified),
        _ => Key::Unidentified,
    }
}

pub fn key_event(name: &str, text: impl Into<String>, mods: Modifiers, held: bool) -> KeyEvent {
    KeyEvent {
        key: map_key(name),
        mods,
        consumed_mods: Modifiers::NONE,
        composing: false,
        utf8: text.into(),
        unshifted_codepoint: name.chars().next().map(|c| c as u32).unwrap_or(0),
        action: if held {
            KeyAction::Repeat
        } else {
            KeyAction::Press
        },
    }
}

pub fn encode_live_key(core: &AppCore, event: &KeyEvent) -> Vec<u8> {
    let mut out = Vec::new();
    encode_key(event, &core.keyboard_mode(), &mut out);
    out
}

pub fn surface_cell(geometry: &SurfaceGeometry, x: f32, y: f32) -> (u16, u16) {
    let col = (x / geometry.metrics.width.max(1.0))
        .floor()
        .clamp(0.0, f32::from(geometry.grid.cols.saturating_sub(1))) as u16;
    let row = (y / geometry.metrics.height.max(1.0))
        .floor()
        .clamp(0.0, f32::from(geometry.grid.rows.saturating_sub(1))) as u16;
    (col, row)
}

pub fn encode_live_mouse(
    core: &AppCore,
    geometry: &SurfaceGeometry,
    x: f32,
    y: f32,
    button: Option<MouseButton>,
    action: MouseAction,
    mods: Modifiers,
) -> Vec<u8> {
    let (col, row) = surface_cell(geometry, x, y);
    let mut out = Vec::new();
    encode_mouse(
        &MouseEvent {
            button,
            action,
            mods,
            col,
            row,
            x_px: x as i32,
            y_px: y as i32,
        },
        &MouseMode::from_modes(&core.modes()),
        &mut out,
    );
    out
}
pub fn encode_live_focus(core: &AppCore, focused: bool) -> Vec<u8> {
    let mut out = Vec::new();
    encode_focus(focused, core.has_mode(TerminalMode::FocusInOut), &mut out);
    out
}
pub fn encode_live_paste(core: &AppCore, text: &str) -> Vec<u8> {
    let mut out = Vec::new();
    encode_paste(text, core.has_mode(TerminalMode::BracketedPaste), &mut out);
    out
}
/// Composition updates produce zero bytes; only committed text is encoded.
/// Lone CR/LF is owned by the key path (terminals send CR for Return).
pub fn encode_ime(text: &str, composing: bool) -> Vec<u8> {
    if composing || text.is_empty() || text == "\n" || text == "\r" {
        return Vec::new();
    }
    let mut out = Vec::new();
    encode_ime_commit(text, &mut out);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn numpad_names_map() {
        assert_eq!(map_key("numpad0"), Key::Numpad0);
        assert_eq!(map_key("numpad_decimal"), Key::NumpadDecimal);
        assert_eq!(map_key("numpad_enter"), Key::NumpadEnter);
        assert_eq!(map_key("numpad_pagedown"), Key::NumpadPageDown);
    }
    #[test]
    fn ime_composition_is_empty() {
        assert!(encode_ime("x", true).is_empty());
        assert!(encode_ime("\n", false).is_empty());
        assert!(encode_ime("\r", false).is_empty());
        assert_eq!(encode_ime("x", false), b"x");
    }
}

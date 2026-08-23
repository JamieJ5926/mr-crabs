//! Shell actions registered as GPUI actions, plus the keybinding
//! conversion for the real keymap.
//!
//! Each [`AppAction`] has exactly one GPUI action struct here
//! (`mr_crabs::NewTab`, ...), so the GPUI keymap/menu pipeline dispatches
//! through the same single shell action set as the palette.

use gpui::{Action, KeyBinding};

use crate::action::AppAction;
use crate::keymap::KeyBindingDef;

gpui::actions!(
    mr_crabs,
    [
        NewWindow,
        CloseWindow,
        NewTab,
        CloseTab,
        NextTab,
        PreviousTab,
        NewSplitRight,
        NewSplitDown,
        ClosePane,
        NextPane,
        PreviousPane,
        GotoSplitUp,
        GotoSplitDown,
        GotoSplitLeft,
        GotoSplitRight,
        TogglePalette,
        ToggleQuickTerminal,
        ToggleSecureInput,
        ReloadConfig,
        CheckForUpdates,
        SearchNext,
        SearchPrevious,
        Quit,
        SetTextAnimationNone,
        SetTextAnimationStreaming,
        SetTextAnimationTypewriter,
        ToggleCursorTrail,
    ]
);

/// The GPUI action name for a shell action.
pub fn gpui_action_name(action: AppAction) -> &'static str {
    match action {
        AppAction::NewWindow => "mr_crabs::NewWindow",
        AppAction::CloseWindow => "mr_crabs::CloseWindow",
        AppAction::NewTab => "mr_crabs::NewTab",
        AppAction::CloseTab => "mr_crabs::CloseTab",
        AppAction::NextTab => "mr_crabs::NextTab",
        AppAction::PreviousTab => "mr_crabs::PreviousTab",
        AppAction::NewSplitRight => "mr_crabs::NewSplitRight",
        AppAction::NewSplitDown => "mr_crabs::NewSplitDown",
        AppAction::ClosePane => "mr_crabs::ClosePane",
        AppAction::NextPane => "mr_crabs::NextPane",
        AppAction::PreviousPane => "mr_crabs::PreviousPane",
        AppAction::GotoSplitUp => "mr_crabs::GotoSplitUp",
        AppAction::GotoSplitDown => "mr_crabs::GotoSplitDown",
        AppAction::GotoSplitLeft => "mr_crabs::GotoSplitLeft",
        AppAction::GotoSplitRight => "mr_crabs::GotoSplitRight",
        AppAction::TogglePalette => "mr_crabs::TogglePalette",
        AppAction::ToggleQuickTerminal => "mr_crabs::ToggleQuickTerminal",
        AppAction::ToggleSecureInput => "mr_crabs::ToggleSecureInput",
        AppAction::ReloadConfig => "mr_crabs::ReloadConfig",
        AppAction::CheckForUpdates => "mr_crabs::CheckForUpdates",
        AppAction::SearchNext => "mr_crabs::SearchNext",
        AppAction::SearchPrevious => "mr_crabs::SearchPrevious",
        AppAction::Quit => "mr_crabs::Quit",
        AppAction::SetTextAnimationNone => "mr_crabs::SetTextAnimationNone",
        AppAction::SetTextAnimationStreaming => "mr_crabs::SetTextAnimationStreaming",
        AppAction::SetTextAnimationTypewriter => "mr_crabs::SetTextAnimationTypewriter",
        AppAction::ToggleCursorTrail => "mr_crabs::ToggleCursorTrail",
    }
}

/// Map a GPUI action back to its shell action by name.
pub fn to_shell_action(action: &dyn Action) -> Option<AppAction> {
    let name = action.name().strip_prefix("mr_crabs::")?;
    // GPUI names are CamelCase; shell names are snake_case.
    let mut out = String::new();
    for (index, ch) in name.chars().enumerate() {
        if ch.is_uppercase() && index > 0 {
            out.push('_');
        }
        out.push(ch.to_ascii_lowercase());
    }
    AppAction::from_name(&out)
}

/// Build the concrete GPUI action for a shell action.
pub fn action_struct(action: AppAction) -> Box<dyn Action> {
    match action {
        AppAction::NewWindow => Box::new(NewWindow),
        AppAction::CloseWindow => Box::new(CloseWindow),
        AppAction::NewTab => Box::new(NewTab),
        AppAction::CloseTab => Box::new(CloseTab),
        AppAction::NextTab => Box::new(NextTab),
        AppAction::PreviousTab => Box::new(PreviousTab),
        AppAction::NewSplitRight => Box::new(NewSplitRight),
        AppAction::NewSplitDown => Box::new(NewSplitDown),
        AppAction::ClosePane => Box::new(ClosePane),
        AppAction::NextPane => Box::new(NextPane),
        AppAction::PreviousPane => Box::new(PreviousPane),
        AppAction::GotoSplitUp => Box::new(GotoSplitUp),
        AppAction::GotoSplitDown => Box::new(GotoSplitDown),
        AppAction::GotoSplitLeft => Box::new(GotoSplitLeft),
        AppAction::GotoSplitRight => Box::new(GotoSplitRight),
        AppAction::TogglePalette => Box::new(TogglePalette),
        AppAction::ToggleQuickTerminal => Box::new(ToggleQuickTerminal),
        AppAction::ToggleSecureInput => Box::new(ToggleSecureInput),
        AppAction::ReloadConfig => Box::new(ReloadConfig),
        AppAction::CheckForUpdates => Box::new(CheckForUpdates),
        AppAction::SearchNext => Box::new(SearchNext),
        AppAction::SearchPrevious => Box::new(SearchPrevious),
        AppAction::Quit => Box::new(Quit),
        AppAction::SetTextAnimationNone => Box::new(SetTextAnimationNone),
        AppAction::SetTextAnimationStreaming => Box::new(SetTextAnimationStreaming),
        AppAction::SetTextAnimationTypewriter => Box::new(SetTextAnimationTypewriter),
        AppAction::ToggleCursorTrail => Box::new(ToggleCursorTrail),
    }
}

/// Convert Ghostty-style `+` keystrokes to GPUI `-` keystrokes.
pub fn shell_keys_to_gpui(keys: &str) -> String {
    keys.replace('+', "-")
}

/// Build the GPUI keymap from shell binding definitions. Invalid bindings
/// are skipped (the shell resolver surfaces them in [`KeymapResolver`]).
pub fn key_bindings(defs: &[KeyBindingDef]) -> Vec<KeyBinding> {
    defs.iter()
        .filter_map(|def| {
            let keys = shell_keys_to_gpui(&def.keys);
            // KeyBinding::new panics on parse errors; the shell resolver
            // already validated the same syntax, but guard the boundary.
            if crate::keymap::ShellKeystroke::parse(&def.keys).is_err() {
                return None;
            }
            Some(binding_for(&keys, def.action))
        })
        .collect()
}

fn binding_for(keys: &str, action: AppAction) -> KeyBinding {
    match action {
        AppAction::NewWindow => KeyBinding::new(keys, NewWindow, None),
        AppAction::CloseWindow => KeyBinding::new(keys, CloseWindow, None),
        AppAction::NewTab => KeyBinding::new(keys, NewTab, None),
        AppAction::CloseTab => KeyBinding::new(keys, CloseTab, None),
        AppAction::NextTab => KeyBinding::new(keys, NextTab, None),
        AppAction::PreviousTab => KeyBinding::new(keys, PreviousTab, None),
        AppAction::NewSplitRight => KeyBinding::new(keys, NewSplitRight, None),
        AppAction::NewSplitDown => KeyBinding::new(keys, NewSplitDown, None),
        AppAction::ClosePane => KeyBinding::new(keys, ClosePane, None),
        AppAction::NextPane => KeyBinding::new(keys, NextPane, None),
        AppAction::PreviousPane => KeyBinding::new(keys, PreviousPane, None),
        AppAction::GotoSplitUp => KeyBinding::new(keys, GotoSplitUp, None),
        AppAction::GotoSplitDown => KeyBinding::new(keys, GotoSplitDown, None),
        AppAction::GotoSplitLeft => KeyBinding::new(keys, GotoSplitLeft, None),
        AppAction::GotoSplitRight => KeyBinding::new(keys, GotoSplitRight, None),
        AppAction::TogglePalette => KeyBinding::new(keys, TogglePalette, None),
        AppAction::ToggleQuickTerminal => KeyBinding::new(keys, ToggleQuickTerminal, None),
        AppAction::ToggleSecureInput => KeyBinding::new(keys, ToggleSecureInput, None),
        AppAction::ReloadConfig => KeyBinding::new(keys, ReloadConfig, None),
        AppAction::CheckForUpdates => KeyBinding::new(keys, CheckForUpdates, None),
        AppAction::SearchNext => KeyBinding::new(keys, SearchNext, None),
        AppAction::SearchPrevious => KeyBinding::new(keys, SearchPrevious, None),
        AppAction::Quit => KeyBinding::new(keys, Quit, None),
        AppAction::SetTextAnimationNone => KeyBinding::new(keys, SetTextAnimationNone, None),
        AppAction::SetTextAnimationStreaming => KeyBinding::new(keys, SetTextAnimationStreaming, None),
        AppAction::SetTextAnimationTypewriter => KeyBinding::new(keys, SetTextAnimationTypewriter, None),
        AppAction::ToggleCursorTrail => KeyBinding::new(keys, ToggleCursorTrail, None),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_shell_action_maps_to_exactly_one_gpui_action() {
        for action in AppAction::ALL {
            let name = gpui_action_name(action);
            assert!(name.starts_with("mr_crabs::"), "namespace prefix");
            let boxed = action_struct(action);
            assert_eq!(boxed.name(), name, "action_struct name matches");
        }
    }

    #[test]
    fn gpui_names_round_trip_to_shell_actions() {
        for action in AppAction::ALL {
            let boxed = action_struct(action);
            assert_eq!(to_shell_action(boxed.as_ref()), Some(action));
        }
    }

    #[test]
    fn keystroke_syntax_conversion() {
        assert_eq!(shell_keys_to_gpui("cmd+t"), "cmd-t");
        assert_eq!(shell_keys_to_gpui("ctrl+cmd+up"), "ctrl-cmd-up");
        assert_eq!(shell_keys_to_gpui("ctrl+`"), "ctrl-`");
    }

    #[test]
    fn key_bindings_cover_the_default_map() {
        let defs = crate::keymap::default_keybindings();
        let bindings = key_bindings(&defs);
        assert_eq!(
            bindings.len(),
            defs.len(),
            "every default binding definition must convert to a GPUI binding"
        );
        for def in &defs {
            assert!(
                bindings
                    .iter()
                    .any(|binding| to_shell_action(binding.action()) == Some(def.action)),
                "no GPUI binding for {:?}",
                def.action
            );
        }
    }
}

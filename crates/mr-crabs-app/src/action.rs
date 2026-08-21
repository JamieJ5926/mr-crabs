//! The shell action set.
//!
//! Every variant maps to exactly one S10 callable-parity ID:
//!
//! | Parity ID              | Action(s)                                       |
//! |------------------------|-------------------------------------------------|
//! | `windows`              | `NewWindow`, `CloseWindow`                      |
//! | `tabs`                 | `NewTab`, `CloseTab`, `NextTab`, `PreviousTab`  |
//! | `splits`               | `NewSplitRight`, `NewSplitDown`, `ClosePane`    |
//! | `pane-focus`           | `NextPane`, `PreviousPane`                      |
//! | `pane-navigation`      | `GotoSplitUp`, `GotoSplitDown`, `GotoSplitLeft`, `GotoSplitRight` |
//! | `command-palette`      | `TogglePalette`                                 |
//! | `config-reload`        | `ReloadConfig`                                  |
//! | `quick-terminal`       | `ToggleQuickTerminal`                           |
//! | `secure-input`         | `ToggleSecureInput`                             |
//! | `updates`              | `CheckForUpdates`                               |
//! | `keyboard-only-operation` | every action is keyboard-dispatachable     |
//! | `menus` / `dock-behavior` / `app-intents` / `accessibility` | consumed by their modules |
//!
//! `AppAction` is deliberately payload-free so it can be serialized into
//! keymaps, menus, palette entries, and restore state. Payload actions
//! (focus a specific pane, open a URL) are separate methods on `AppModel`.

use serde::{Deserialize, Serialize};

/// The complete product-shell action set.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AppAction {
    /// Open a new window with one tab.
    NewWindow,
    /// Close the active window (shutting down its sessions).
    CloseWindow,
    /// Open a new tab in the active window.
    NewTab,
    /// Close the active tab (or the window when it was the last tab).
    CloseTab,
    /// Focus the next tab.
    NextTab,
    /// Focus the previous tab.
    PreviousTab,
    /// Split the focused pane to the right.
    NewSplitRight,
    /// Split the focused pane below.
    NewSplitDown,
    /// Close the focused pane (cascading to tab/window when last).
    ClosePane,
    /// Cycle focus to the next pane in focus order.
    NextPane,
    /// Cycle focus to the previous pane in focus order.
    PreviousPane,
    /// Move focus up.
    GotoSplitUp,
    /// Move focus down.
    GotoSplitDown,
    /// Move focus left.
    GotoSplitLeft,
    /// Move focus right.
    GotoSplitRight,
    /// Toggle the command palette.
    TogglePalette,
    /// Toggle the quick terminal.
    ToggleQuickTerminal,
    /// Toggle secure input.
    ToggleSecureInput,
    /// Reload configuration from the configured source.
    ReloadConfig,
    /// Check for updates (never performs network I/O).
    CheckForUpdates,
    /// Search forward for the active search query (S8).
    SearchNext,
    /// Search backward for the active search query (S8).
    SearchPrevious,
    /// Quit the application, shutting down every session deterministically.
    Quit,
    /// Set text animation to none for this process.
    SetTextAnimationNone,
    /// Set text animation to streaming for this process.
    SetTextAnimationStreaming,
    /// Set text animation to typewriter for this process.
    SetTextAnimationTypewriter,
    /// Toggle the cursor trail for this process.
    ToggleCursorTrail,
}

impl AppAction {
    /// Every shell action in declaration order.
    pub const ALL: [AppAction; 27] = [
        AppAction::NewWindow,
        AppAction::CloseWindow,
        AppAction::NewTab,
        AppAction::CloseTab,
        AppAction::NextTab,
        AppAction::PreviousTab,
        AppAction::NewSplitRight,
        AppAction::NewSplitDown,
        AppAction::ClosePane,
        AppAction::NextPane,
        AppAction::PreviousPane,
        AppAction::GotoSplitUp,
        AppAction::GotoSplitDown,
        AppAction::GotoSplitLeft,
        AppAction::GotoSplitRight,
        AppAction::TogglePalette,
        AppAction::ToggleQuickTerminal,
        AppAction::ToggleSecureInput,
        AppAction::ReloadConfig,
        AppAction::CheckForUpdates,
        AppAction::SearchNext,
        AppAction::SearchPrevious,
        AppAction::Quit,
        AppAction::SetTextAnimationNone,
        AppAction::SetTextAnimationStreaming,
        AppAction::SetTextAnimationTypewriter,
        AppAction::ToggleCursorTrail,
    ];

    /// Stable machine name (used for palette command ids and serialization).
    pub fn name(self) -> &'static str {
        match self {
            AppAction::NewWindow => "new_window",
            AppAction::CloseWindow => "close_window",
            AppAction::NewTab => "new_tab",
            AppAction::CloseTab => "close_tab",
            AppAction::NextTab => "next_tab",
            AppAction::PreviousTab => "previous_tab",
            AppAction::NewSplitRight => "new_split_right",
            AppAction::NewSplitDown => "new_split_down",
            AppAction::ClosePane => "close_pane",
            AppAction::NextPane => "next_pane",
            AppAction::PreviousPane => "previous_pane",
            AppAction::GotoSplitUp => "goto_split_up",
            AppAction::GotoSplitDown => "goto_split_down",
            AppAction::GotoSplitLeft => "goto_split_left",
            AppAction::GotoSplitRight => "goto_split_right",
            AppAction::TogglePalette => "toggle_palette",
            AppAction::ToggleQuickTerminal => "toggle_quick_terminal",
            AppAction::ToggleSecureInput => "toggle_secure_input",
            AppAction::ReloadConfig => "reload_config",
            AppAction::CheckForUpdates => "check_for_updates",
            AppAction::SearchNext => "search_next",
            AppAction::SearchPrevious => "search_previous",
            AppAction::Quit => "quit",
            AppAction::SetTextAnimationNone => "set_text_animation_none",
            AppAction::SetTextAnimationStreaming => "set_text_animation_streaming",
            AppAction::SetTextAnimationTypewriter => "set_text_animation_typewriter",
            AppAction::ToggleCursorTrail => "toggle_cursor_trail",
        }
    }

    /// Human title shown in the palette and menus.
    pub fn title(self) -> &'static str {
        match self {
            AppAction::NewWindow => "New Window",
            AppAction::CloseWindow => "Close Window",
            AppAction::NewTab => "New Tab",
            AppAction::CloseTab => "Close Tab",
            AppAction::NextTab => "Next Tab",
            AppAction::PreviousTab => "Previous Tab",
            AppAction::NewSplitRight => "New Split Right",
            AppAction::NewSplitDown => "New Split Down",
            AppAction::ClosePane => "Close Pane",
            AppAction::NextPane => "Next Pane",
            AppAction::PreviousPane => "Previous Pane",
            AppAction::GotoSplitUp => "Move Focus Up",
            AppAction::GotoSplitDown => "Move Focus Down",
            AppAction::GotoSplitLeft => "Move Focus Left",
            AppAction::GotoSplitRight => "Move Focus Right",
            AppAction::TogglePalette => "Command Palette",
            AppAction::ToggleQuickTerminal => "Quick Terminal",
            AppAction::ToggleSecureInput => "Secure Input",
            AppAction::ReloadConfig => "Reload Configuration",
            AppAction::CheckForUpdates => "Check for Updates",
            AppAction::SearchNext => "Search Next",
            AppAction::SearchPrevious => "Search Previous",
            AppAction::Quit => "Quit",
            AppAction::SetTextAnimationNone => "Text Animation: None",
            AppAction::SetTextAnimationStreaming => "Text Animation: Streaming",
            AppAction::SetTextAnimationTypewriter => "Text Animation: Typewriter",
            AppAction::ToggleCursorTrail => "Toggle Cursor Trail",
        }
    }

    /// Coarse grouping used by the palette and menus.
    pub fn category(self) -> &'static str {
        match self {
            AppAction::NewWindow | AppAction::CloseWindow => "Window",
            AppAction::NewTab
            | AppAction::CloseTab
            | AppAction::NextTab
            | AppAction::PreviousTab => "Tab",
            AppAction::NewSplitRight | AppAction::NewSplitDown | AppAction::ClosePane => "Split",
            AppAction::NextPane
            | AppAction::PreviousPane
            | AppAction::GotoSplitUp
            | AppAction::GotoSplitDown
            | AppAction::GotoSplitLeft
            | AppAction::GotoSplitRight => "Pane",
            AppAction::TogglePalette
            | AppAction::ToggleQuickTerminal
            | AppAction::ToggleSecureInput
            | AppAction::ReloadConfig
            | AppAction::SearchNext
            | AppAction::SearchPrevious
            | AppAction::SetTextAnimationNone
            | AppAction::SetTextAnimationStreaming
            | AppAction::SetTextAnimationTypewriter
            | AppAction::ToggleCursorTrail => "View",
            AppAction::CheckForUpdates | AppAction::Quit => "App",
        }
    }

    /// Resolve a machine name back to an action.
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|a| a.name() == name)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_covers_every_parity_id_exactly_once() {
        // One entry per variant, no duplicates, and every variant is in ALL.
        assert_eq!(AppAction::ALL.len(), 27);
        let mut names: Vec<&str> = AppAction::ALL.iter().map(|a| a.name()).collect();
        names.sort_unstable();
        let mut dedup = names.clone();
        dedup.dedup();
        assert_eq!(names, dedup, "action names must be unique");
    }

    #[test]
    fn name_title_and_category_are_stable_and_nonempty() {
        for action in AppAction::ALL {
            assert!(!action.name().is_empty());
            assert!(!action.title().is_empty());
            assert!(!action.category().is_empty());
            assert_eq!(AppAction::from_name(action.name()), Some(action));
        }
        assert_eq!(AppAction::from_name("nope"), None);
    }

    #[test]
    fn serde_round_trip_uses_snake_case_names() {
        for action in AppAction::ALL {
            let json = serde_json::to_string(&action).unwrap();
            let back: AppAction = serde_json::from_str(&json).unwrap();
            assert_eq!(back, action);
        }
        let json = serde_json::to_string(&AppAction::NewSplitRight).unwrap();
        assert_eq!(json, "\"new_split_right\"");
    }
}

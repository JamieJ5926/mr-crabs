//! Shell keybindings and the headless keymap resolver (keyboard-only
//! operation).
//!
//! Bindings use Ghostty-style strings (`"cmd+t"`, `"ctrl+shift+space"`,
//! `"ctrl+`"`) and map to [`AppAction`]s. The resolver is a pure function of
//! a keystroke string and the binding list, so the whole keyboard-only
//! surface is testable without a window. The GPUI binary converts the same
//! definitions into `gpui::KeyBinding`s.

use serde::{Deserialize, Serialize};

use crate::action::AppAction;

/// One keybinding definition. `keys` uses Ghostty syntax: modifiers joined
/// with `+` (`ctrl`, `alt`, `shift`, `cmd`, `super`, `win`) followed by the
/// key (`a`, `1`, `up`, `f1`, `` ` ``, `.`, ...).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct KeyBindingDef {
    pub keys: String,
    #[serde(rename = "action")]
    pub action: AppAction,
    /// Reserved for per-surface contexts (e.g. `"palette"`); `None` applies
    /// everywhere.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<String>,
}

impl KeyBindingDef {
    pub fn new(keys: &str, action: AppAction) -> Self {
        Self {
            keys: keys.to_string(),
            action,
            context: None,
        }
    }
}

/// A parsed shell keystroke: a modifier mask plus the key.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub struct ModifierMask {
    pub ctrl: bool,
    pub alt: bool,
    pub shift: bool,
    pub cmd: bool,
}

impl ModifierMask {
    pub const NONE: Self = Self {
        ctrl: false,
        alt: false,
        shift: false,
        cmd: false,
    };

    fn parse(part: &str) -> Option<Self> {
        Some(match part {
            "ctrl" | "control" => Self {
                ctrl: true,
                ..Self::NONE
            },
            "alt" | "option" => Self {
                alt: true,
                ..Self::NONE
            },
            "shift" => Self {
                shift: true,
                ..Self::NONE
            },
            "cmd" | "command" | "super" | "meta" | "win" => Self {
                cmd: true,
                ..Self::NONE
            },
            _ => return None,
        })
    }

    pub fn any(self) -> bool {
        self.ctrl || self.alt || self.shift || self.cmd
    }
}

/// A parsed keystroke.
#[derive(Clone, Debug, Eq, PartialEq, Hash)]
pub struct ShellKeystroke {
    pub mods: ModifierMask,
    pub key: String,
}

impl ShellKeystroke {
    /// Parse Ghostty-style `"ctrl+shift+up"`. The key may itself contain no
    /// `+`; modifiers may appear in any order.
    pub fn parse(source: &str) -> Result<Self, String> {
        let mut mods = ModifierMask::NONE;
        let mut key: Option<&str> = None;
        for part in source.split('+') {
            if part.is_empty() {
                return Err(format!("empty component in keystroke {source:?}"));
            }
            if let Some(parsed) = ModifierMask::parse(part) {
                mods.ctrl |= parsed.ctrl;
                mods.alt |= parsed.alt;
                mods.shift |= parsed.shift;
                mods.cmd |= parsed.cmd;
            } else if key.is_some() {
                return Err(format!("multiple keys in keystroke {source:?}"));
            } else {
                key = Some(part);
            }
        }
        let key = key.ok_or_else(|| format!("missing key in keystroke {source:?}"))?;
        Ok(Self {
            mods,
            key: key.to_string(),
        })
    }
}

/// Ghostty-compatible default shell keybindings. These mirror the product
/// defaults for the shell actions; the authoritative Ghostty key/default
/// parity remains owned by the config slice.
pub fn default_keybindings() -> Vec<KeyBindingDef> {
    vec![
        KeyBindingDef::new("cmd+shift+n", AppAction::NewWindow),
        KeyBindingDef::new("cmd+t", AppAction::NewTab),
        // close_surface semantics: closes the focused pane; the last pane
        // closes the tab, and the last tab closes the window.
        KeyBindingDef::new("cmd+w", AppAction::ClosePane),
        KeyBindingDef::new("cmd+shift+w", AppAction::CloseWindow),
        KeyBindingDef::new("cmd+]", AppAction::NextTab),
        KeyBindingDef::new("cmd+[", AppAction::PreviousTab),
        KeyBindingDef::new("cmd+d", AppAction::NewSplitRight),
        KeyBindingDef::new("cmd+shift+d", AppAction::NewSplitDown),
        KeyBindingDef::new("ctrl+cmd+up", AppAction::GotoSplitUp),
        KeyBindingDef::new("ctrl+cmd+down", AppAction::GotoSplitDown),
        KeyBindingDef::new("ctrl+cmd+left", AppAction::GotoSplitLeft),
        KeyBindingDef::new("ctrl+cmd+right", AppAction::GotoSplitRight),
        KeyBindingDef::new("cmd+shift+p", AppAction::TogglePalette),
        KeyBindingDef::new("cmd+shift+j", AppAction::ToggleChatPresentation),
        KeyBindingDef::new("ctrl+`", AppAction::ToggleQuickTerminal),
        KeyBindingDef::new("ctrl+cmd+o", AppAction::ToggleSecureInput),
        KeyBindingDef::new("cmd+shift+r", AppAction::ReloadConfig),
        KeyBindingDef::new("cmd+shift+g", AppAction::SearchNext),
        KeyBindingDef::new("cmd+shift+h", AppAction::SearchPrevious),
        KeyBindingDef::new("cmd+q", AppAction::Quit),
    ]
}

/// Resolves keystrokes to actions. Later bindings for the same keystroke
/// override earlier ones (user config wins over defaults).
pub struct KeymapResolver {
    bindings: Vec<(ShellKeystroke, KeyBindingDef)>,
    /// Binding sources that failed to parse (user config errors, surfaced
    /// rather than silently dropped).
    pub invalid: Vec<String>,
}

impl KeymapResolver {
    pub fn new(bindings: Vec<KeyBindingDef>) -> Self {
        let mut resolved = Vec::new();
        let mut invalid = Vec::new();
        for def in bindings {
            match ShellKeystroke::parse(&def.keys) {
                Ok(keystroke) => resolved.push((keystroke, def)),
                Err(e) => invalid.push(format!("{}: {e}", def.keys)),
            }
        }
        Self {
            bindings: resolved,
            invalid,
        }
    }

    /// Resolve one keystroke string to an action. The context is accepted
    /// for future per-surface bindings; shell bindings currently ignore it.
    pub fn resolve(&self, keystroke: &str, _context: &str) -> Option<AppAction> {
        let parsed = ShellKeystroke::parse(keystroke).ok()?;
        // Last match wins.
        self.bindings
            .iter()
            .rev()
            .find(|(binding, _)| *binding == parsed)
            .map(|(_, def)| def.action)
    }

    /// The definitions bound to an action, in binding order.
    pub fn bindings_for_action(&self, action: AppAction) -> Vec<&KeyBindingDef> {
        self.bindings
            .iter()
            .filter(|(_, def)| def.action == action)
            .map(|(_, def)| def)
            .collect()
    }

    pub fn is_empty(&self) -> bool {
        self.bindings.is_empty()
    }

    pub fn len(&self) -> usize {
        self.bindings.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_plain_and_modified_keystrokes() {
        let plain = ShellKeystroke::parse("t").expect("plain key");
        assert_eq!(plain.mods, ModifierMask::NONE);
        assert_eq!(plain.key, "t");

        let chord = ShellKeystroke::parse("ctrl+shift+up").expect("chord");
        assert!(chord.mods.ctrl && chord.mods.shift && !chord.mods.alt && !chord.mods.cmd);
        assert_eq!(chord.key, "up");

        let cmd = ShellKeystroke::parse("cmd+t").expect("cmd chord");
        assert!(cmd.mods.cmd && cmd.mods.any() && cmd.key == "t");

        let tick = ShellKeystroke::parse("ctrl+`").expect("tick chord");
        assert!(tick.mods.ctrl);
        assert_eq!(tick.key, "`");

        let dot = ShellKeystroke::parse("cmd+shift+.").expect("dot chord");
        assert!(dot.mods.cmd && dot.mods.shift);
        assert_eq!(dot.key, ".");
    }

    #[test]
    fn parse_rejects_malformed_keystrokes() {
        assert!(ShellKeystroke::parse("").is_err());
        assert!(ShellKeystroke::parse("+t").is_err());
        assert!(ShellKeystroke::parse("cmd+").is_err());
        assert!(ShellKeystroke::parse("cmd+t+u").is_err());
        assert!(
            ShellKeystroke::parse("frobnicate+t").is_err(),
            "unknown modifier"
        );
    }

    #[test]
    fn defaults_cover_the_shell_action_surface() {
        let bindings = default_keybindings();
        assert_eq!(bindings.len(), 20);
        let resolver = KeymapResolver::new(bindings);
        assert!(resolver.invalid.is_empty());
        assert_eq!(resolver.resolve("cmd+t", ""), Some(AppAction::NewTab));
        assert_eq!(
            resolver.resolve("cmd+d", ""),
            Some(AppAction::NewSplitRight)
        );
        assert_eq!(
            resolver.resolve("cmd+shift+d", ""),
            Some(AppAction::NewSplitDown)
        );
        assert_eq!(
            resolver.resolve("cmd+shift+p", ""),
            Some(AppAction::TogglePalette)
        );
        assert_eq!(resolver.resolve("cmd+q", ""), Some(AppAction::Quit));
        assert_eq!(
            resolver.resolve("ctrl+cmd+right", ""),
            Some(AppAction::GotoSplitRight)
        );
        assert_eq!(
            resolver.resolve("ctrl+`", ""),
            Some(AppAction::ToggleQuickTerminal)
        );
        assert_eq!(
            resolver.resolve("cmd+shift+g", ""),
            Some(AppAction::SearchNext)
        );
        assert_eq!(
            resolver.resolve("cmd+shift+h", ""),
            Some(AppAction::SearchPrevious)
        );
    }

    #[test]
    fn resolver_rejects_unbound_keystrokes() {
        let resolver = KeymapResolver::new(default_keybindings());
        assert_eq!(resolver.resolve("cmd+alt+9", ""), None);
        assert_eq!(resolver.resolve("not+a+chord", ""), None);
    }

    #[test]
    fn later_bindings_override_earlier_ones() {
        let bindings = vec![
            KeyBindingDef::new("cmd+t", AppAction::NewTab),
            KeyBindingDef::new("cmd+t", AppAction::CloseTab),
        ];
        let resolver = KeymapResolver::new(bindings);
        assert_eq!(resolver.resolve("cmd+t", ""), Some(AppAction::CloseTab));
    }

    #[test]
    fn invalid_binding_sources_are_surfaced_not_dropped() {
        let bindings = vec![
            KeyBindingDef::new("cmd+t", AppAction::NewTab),
            KeyBindingDef::new("bogus+key", AppAction::Quit),
        ];
        let resolver = KeymapResolver::new(bindings);
        assert_eq!(resolver.len(), 1);
        assert_eq!(resolver.invalid.len(), 1);
        assert_eq!(resolver.resolve("cmd+t", ""), Some(AppAction::NewTab));
    }

    #[test]
    fn modifier_mask_parses_aliases() {
        for alias in ["cmd", "command", "super", "meta", "win"] {
            let keystroke = ShellKeystroke::parse(&format!("{alias}+t")).expect("alias");
            assert!(keystroke.mods.cmd, "{alias} must map to cmd");
        }
        for alias in ["ctrl", "control"] {
            let keystroke = ShellKeystroke::parse(&format!("{alias}+t")).expect("alias");
            assert!(keystroke.mods.ctrl, "{alias} must map to ctrl");
        }
    }
}

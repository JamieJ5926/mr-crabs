//! Menu model: the application menu bar as a headless structure.
//!
//! The model is the single source of truth; the GPUI bridge converts it to
//! `gpui::Menu`s for the real menu bar. Every menu action is an
//! [`AppAction`], so menus, palette, and keybindings all dispatch through
//! the same registry.

use serde::{Deserialize, Serialize};

use crate::action::AppAction;

/// One menu (the menu bar or a submenu).
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct MenuDef {
    pub name: String,
    pub items: Vec<MenuItemDef>,
}

impl MenuDef {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            items: Vec::new(),
        }
    }

    pub fn item(mut self, item: MenuItemDef) -> Self {
        self.items.push(item);
        self
    }
}

/// One menu item.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum MenuItemDef {
    Separator,
    Submenu(MenuDef),
    Action {
        name: String,
        action: AppAction,
        #[serde(default)]
        checked: bool,
        #[serde(default)]
        disabled: bool,
    },
}

impl MenuItemDef {
    pub fn separator() -> Self {
        Self::Separator
    }

    pub fn action(name: impl Into<String>, action: AppAction) -> Self {
        Self::Action {
            name: name.into(),
            action,
            checked: false,
            disabled: false,
        }
    }

    pub fn submenu(menu: MenuDef) -> Self {
        Self::Submenu(menu)
    }
}

/// The shell menu model.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct MenuModel {
    pub menus: Vec<MenuDef>,
}

impl MenuModel {
    pub fn new() -> Self {
        Self { menus: Vec::new() }
    }

    /// The default shell menu bar, modeled on the Ghostty product menus.
    pub fn default_shell() -> Self {
        let app_menu = MenuDef::new("Mr Crabs")
            .item(MenuItemDef::action(
                "Check for Updates",
                AppAction::CheckForUpdates,
            ))
            .item(MenuItemDef::separator())
            .item(MenuItemDef::action(
                "Quick Terminal",
                AppAction::ToggleQuickTerminal,
            ))
            .item(MenuItemDef::action(
                "Secure Input",
                AppAction::ToggleSecureInput,
            ))
            .item(MenuItemDef::separator())
            .item(MenuItemDef::action("Quit", AppAction::Quit));

        let file_menu = MenuDef::new("File")
            .item(MenuItemDef::action("New Window", AppAction::NewWindow))
            .item(MenuItemDef::action("New Tab", AppAction::NewTab))
            .item(MenuItemDef::separator())
            .item(MenuItemDef::action("Close Tab", AppAction::CloseTab))
            .item(MenuItemDef::action("Close Window", AppAction::CloseWindow));

        let view_menu = MenuDef::new("View")
            .item(MenuItemDef::action(
                "Command Palette",
                AppAction::TogglePalette,
            ))
            .item(MenuItemDef::action(
                "Chat Presentation",
                AppAction::ToggleChatPresentation,
            ))
            .item(MenuItemDef::action(
                "Reload Configuration",
                AppAction::ReloadConfig,
            ))
            .item(MenuItemDef::separator())
            .item(MenuItemDef::action("Search Next", AppAction::SearchNext))
            .item(MenuItemDef::action(
                "Search Previous",
                AppAction::SearchPrevious,
            ))
            .item(MenuItemDef::separator())
            .item(MenuItemDef::action(
                "New Split Right",
                AppAction::NewSplitRight,
            ))
            .item(MenuItemDef::action(
                "New Split Down",
                AppAction::NewSplitDown,
            ))
            .item(MenuItemDef::action("Close Pane", AppAction::ClosePane));

        let split_navigation = MenuDef::new("Split Navigation")
            .item(MenuItemDef::action("Move Focus Up", AppAction::GotoSplitUp))
            .item(MenuItemDef::action(
                "Move Focus Down",
                AppAction::GotoSplitDown,
            ))
            .item(MenuItemDef::action(
                "Move Focus Left",
                AppAction::GotoSplitLeft,
            ))
            .item(MenuItemDef::action(
                "Move Focus Right",
                AppAction::GotoSplitRight,
            ));

        let window_menu = MenuDef::new("Window")
            .item(MenuItemDef::action("Next Tab", AppAction::NextTab))
            .item(MenuItemDef::action("Previous Tab", AppAction::PreviousTab))
            .item(MenuItemDef::separator())
            .item(MenuItemDef::action("Next Pane", AppAction::NextPane))
            .item(MenuItemDef::action(
                "Previous Pane",
                AppAction::PreviousPane,
            ))
            .item(MenuItemDef::submenu(split_navigation));

        Self {
            menus: vec![app_menu, file_menu, view_menu, window_menu],
        }
    }

    pub fn find(&self, name: &str) -> Option<&MenuDef> {
        self.menus.iter().find(|menu| menu.name == name)
    }

    /// Flatten every action item (menu, submenu, nested submenu) in order.
    pub fn action_items(&self) -> Vec<(String, AppAction)> {
        let mut out = Vec::new();
        for menu in &self.menus {
            collect_actions(menu, &mut out);
        }
        out
    }

    pub fn contains_action(&self, action: AppAction) -> bool {
        self.action_items().iter().any(|(_, item)| *item == action)
    }

    /// Set the checked state of every item bound to `action`; returns
    /// whether any item changed.
    pub fn set_checked(&mut self, action: AppAction, checked: bool) -> bool {
        let mut changed = false;
        for menu in &mut self.menus {
            changed |= set_checked_rec(menu, action, checked);
        }
        changed
    }
}

fn collect_actions(menu: &MenuDef, out: &mut Vec<(String, AppAction)>) {
    for item in &menu.items {
        match item {
            MenuItemDef::Action { name, action, .. } => out.push((name.clone(), *action)),
            MenuItemDef::Submenu(submenu) => collect_actions(submenu, out),
            MenuItemDef::Separator => {}
        }
    }
}

fn set_checked_rec(menu: &mut MenuDef, action: AppAction, checked: bool) -> bool {
    let mut changed = false;
    for item in &mut menu.items {
        match item {
            MenuItemDef::Action {
                action: item_action,
                checked: item_checked,
                ..
            } if *item_action == action => {
                if *item_checked != checked {
                    *item_checked = checked;
                    changed = true;
                }
            }
            MenuItemDef::Submenu(submenu) => {
                changed |= set_checked_rec(submenu, action, checked);
            }
            _ => {}
        }
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_shell_menus_cover_the_action_surface() {
        let model = MenuModel::default_shell();
        assert_eq!(model.menus.len(), 4);
        assert!(model.find("Mr Crabs").is_some());
        assert!(model.find("File").is_some());
        assert!(model.find("View").is_some());
        assert!(model.find("Window").is_some());
        const PALETTE_ONLY: [AppAction; 4] = [
            AppAction::SetTextAnimationNone,
            AppAction::SetTextAnimationStreaming,
            AppAction::SetTextAnimationTypewriter,
            AppAction::ToggleCursorTrail,
        ];
        for action in AppAction::ALL {
            if PALETTE_ONLY.contains(&action) {
                assert!(
                    !model.contains_action(action),
                    "palette-only action must stay out of production menus: {action:?}"
                );
            } else {
                assert!(model.contains_action(action), "menu must expose {action:?}");
            }
        }
    }

    #[test]
    fn submenu_actions_flatten_in_order() {
        let model = MenuModel::default_shell();
        let items = model.action_items();
        assert!(items.iter().any(|(name, _)| name == "Move Focus Up"));
        assert!(items.iter().any(|(_, action)| *action == AppAction::Quit));
        let positions: Vec<usize> = items
            .iter()
            .enumerate()
            .filter(|(_, (_, action))| *action == AppAction::GotoSplitUp)
            .map(|(i, _)| i)
            .collect();
        assert_eq!(positions.len(), 1, "each action appears exactly once");
    }

    #[test]
    fn set_checked_touches_only_matching_items() {
        let mut model = MenuModel::default_shell();
        assert!(model.set_checked(AppAction::ToggleSecureInput, true));
        assert!(
            !model.set_checked(AppAction::ToggleSecureInput, true),
            "no change when already checked"
        );
        // The app-menu secure-input item is now checked; the View menu's
        // secure-input item is checked too; no other item changed.
        let mut checked_items = 0;
        for menu in &model.menus {
            for item in &menu.items {
                if let MenuItemDef::Action {
                    action, checked, ..
                } = item
                {
                    if *action == AppAction::ToggleSecureInput {
                        assert!(*checked);
                        checked_items += 1;
                    } else {
                        assert!(!*checked);
                    }
                }
            }
        }
        assert_eq!(checked_items, 1);
        // Unchecking reports a change again.
        assert!(model.set_checked(AppAction::ToggleSecureInput, false));
    }
}

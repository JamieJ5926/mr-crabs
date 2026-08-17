//! Menu-model to GPUI menu conversion.

use gpui::Menu;

use crate::menu::{MenuDef, MenuItemDef};

use super::actions::action_struct;

/// Convert the shell menu model into `gpui::Menu`s for the real menu bar.
pub fn gpui_menus(model: &crate::menu::MenuModel) -> Vec<Menu> {
    model.menus.iter().map(convert_menu).collect()
}

fn convert_menu(def: &MenuDef) -> Menu {
    let items = def
        .items
        .iter()
        .map(|item| match item {
            MenuItemDef::Separator => gpui::MenuItem::separator(),
            MenuItemDef::Submenu(submenu) => gpui::MenuItem::submenu(convert_menu(submenu)),
            MenuItemDef::Action {
                name,
                action,
                checked,
                disabled,
            } => gpui::MenuItem::Action {
                name: name.clone().into(),
                action: action_struct(*action),
                os_action: None,
                checked: *checked,
                disabled: *disabled,
            },
        })
        .collect();
    let mut menu = Menu::new(def.name.clone());
    menu.items = items;
    menu
}

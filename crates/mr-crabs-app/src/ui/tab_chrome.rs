//! Visible tab chrome: titles, active indication, click select/close.
//!
//! The shell model already owns tabs (`WindowModel.tab_order`, `active_tab`,
//! `TabModel.title`) and the keymap already binds `cmd+t`/`cmd+w`/`cmd+]/[`
//! to `NewTab`/`CloseTab`/`NextTab`/`PreviousTab`. What was missing is purely
//! visual: `WindowView::render` composes only panes from
//! `window_model.active_tab()` (`ui/workspace.rs:436-468`) and never draws a
//! tab strip. This module is that strip, standalone so it integrates through
//! one leased call site in `WindowView::render` (see `SEAM` below) without
//! touching `workspace.rs` or any `AppModel` file.
//!
//! `SEAM` (for the AppsDeep lease; not applied here):
//! ```text
//! file: crates/mr-crabs-app/src/ui/workspace.rs
//! anchor: after the native-title sync block (render step 3, ~line 434),
//!   before "// 4. Compose every pane in the active tab"
//! hunk:
//!   +        // Tab strip (CrabsTabs seam): titles + active state + click.
//!   +        let tab_bar = self.model.read(cx).window(self.window_id).map(
//!   +            |window_model| {
//!   +                crate::ui::tab_chrome::render_tab_bar(
//!   +                    &crate::ui::tab_chrome::tab_entries(window_model),
use gpui::{
    App, Div, ElementId, Entity, InteractiveElement as _, MouseButton, ParentElement as _, Styled
    as _, div, px, rgb,
};

use crate::model::app_model::AppModel;
use crate::model::tab::TabId;
use crate::model::window::{WindowId, WindowModel};

/// Reserved tab-strip height in pixels. `WindowView::render` subtracts this
/// from the viewport before committing surface geometry and offsets every
/// window-space `top` by it, so panes, dock, and hit areas stay consistent.
pub const TAB_ROW_HEIGHT_PX: f32 = 28.0;

/// One visible tab: stable identity, human title, selection state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TabEntry {
    pub id: TabId,
    pub title: String,
    pub active: bool,
}

/// Project a window's tab order into visible entries. Titles come from the
/// existing tab model; an empty title falls back to `"shell"` (the
/// `TabModel::new` default) so a tab never renders blank.
pub fn tab_entries(window: &WindowModel) -> Vec<TabEntry> {
    window
        .tab_order
        .iter()
        .filter_map(|id| {
            window.tab(*id).map(|tab| TabEntry {
                id: *id,
                title: if tab.title.is_empty() {
                    "shell".to_string()
                } else {
                    tab.title.clone()
                },
                active: window.active_tab == Some(*id),
            })
        })
        .collect()
}

/// Stable GPUI element id for a tab pill (click selects).
pub fn tab_element_id(id: TabId) -> ElementId {
    ElementId::NamedInteger("crabs-tab".into(), id.as_u64())
}

/// Stable GPUI element id for a tab's close affordance (click closes).
pub fn tab_close_element_id(id: TabId) -> ElementId {
    ElementId::NamedInteger("crabs-tab-close".into(), id.as_u64())
}

/// Render the tab strip: one pill per entry in window order, the active pill
/// visibly indicated (brighter text + underline bar), each pill carrying a
/// `x` close affordance. Clicking a pill selects its tab; clicking `x`
/// closes it and the model moves focus to another tab
/// (`WindowModel::close_tab` keeps `active_tab` on the newest remaining tab).
/// Keyboard select/close keep working through the existing keymap bindings.
pub fn render_tab_bar(
    entries: &[TabEntry],
    window_id: WindowId,
    model: Entity<AppModel>,
) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(px(4.0))
        .px(px(8.0))
        .h(px(TAB_ROW_HEIGHT_PX))
        .overflow_hidden()
        .bg(rgb(0x1C1A17))
        .children(entries.iter().map(|entry| {
            let tab_id = entry.id;
            let select_model = model.clone();
            let close_model = model.clone();
            let (fg, underline) = if entry.active {
                (rgb(0xF7F4EE), rgb(0xD4A24A))
            } else {
                (rgb(0x8A867E), rgb(0x1C1A17))
            };
            div()
                .id(tab_element_id(tab_id))
                .flex()
                .flex_row()
                .items_center()
                .gap(px(6.0))
                .px(px(10.0))
                .py(px(2.0))
                .text_color(fg)
                .border_b(px(if entry.active { 2.0 } else { 1.0 }))
                .border_color(underline)
                .cursor_pointer()
                .on_mouse_down(MouseButton::Left, move |_, _, cx: &mut App| {
                    select_model.update(cx, |shell, _| {
                        shell.activate_tab(tab_id);
                    });
                })
                .child(entry.title.clone())
                .child(
                    div()
                        .id(tab_close_element_id(tab_id))
                        .px(px(4.0))
                        .text_color(rgb(0x8A867E))
                        .cursor_pointer()
                        .on_mouse_down(MouseButton::Left, move |_, _, cx: &mut App| {
                            close_model.update(cx, |shell, _| {
                                shell.close_tab_anywhere(window_id, tab_id);
                            });
                        })
                        .child("x"),
                )
        }))
}

/// Focus target after closing `closed` out of `order`: the newest remaining
/// tab (matches `WindowModel::close_tab`, which keeps the last of
/// `tab_order`). Pure helper so close-return behavior is pinned without GPUI.
pub fn focus_after_close(order: &[TabId], closed: TabId) -> Option<TabId> {
    let remaining: Vec<TabId> = order.iter().copied().filter(|id| *id != closed).collect();
    remaining.last().copied()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::AppAction;
    use mr_crabs_terminal::GridSize;

    fn window_with_two_tabs() -> (AppModel, WindowId, TabId, TabId) {
        let mut shell = AppModel::new();
        let window_id = shell.new_window().expect("window");
        let first = shell
            .window(window_id)
            .and_then(|window| window.active_tab)
            .expect("first tab");
        shell
            .window_mut(window_id)
            .expect("window")
            .tab_mut(first)
            .expect("tab")
            .title = "alpha".to_string();
        let second = shell.new_tab(window_id).expect("second tab");
        shell
            .window_mut(window_id)
            .expect("window")
            .tab_mut(second)
            .expect("tab")
            .title = "beta".to_string();
        assert_eq!(GridSize::new(80, 24).cols, 80);
        (shell, window_id, first, second)
    }

    #[test]
    fn two_tabs_render_distinct_titles_with_selected_indicator() {
        let (shell, window_id, first, second) = window_with_two_tabs();
        let window = shell.window(window_id).expect("window");
        assert_eq!(window.active_tab, Some(second));
        let entries = tab_entries(window);
        assert_eq!(
            entries,
            vec![
                TabEntry {
                    id: first,
                    title: "alpha".to_string(),
                    active: false,
                },
                TabEntry {
                    id: second,
                    title: "beta".to_string(),
                    active: true,
                },
            ]
        );
    }

    #[test]
    fn empty_title_never_renders_blank() {
        let (mut shell, window_id, first, _) = window_with_two_tabs();
        shell
            .window_mut(window_id)
            .expect("window")
            .tab_mut(first)
            .expect("tab")
            .title = String::new();
        let entries = tab_entries(shell.window(window_id).expect("window"));
        assert_eq!(entries[0].title, "shell");
    }

    #[test]
    fn keyboard_select_moves_active_flag() {
        let (mut shell, window_id, first, second) = window_with_two_tabs();
        shell.dispatch(AppAction::PreviousTab);
        assert_eq!(
            shell.window(window_id).expect("window").active_tab,
            Some(first)
        );
        let entries = tab_entries(shell.window(window_id).expect("window"));
        assert!(entries[0].active);
        assert!(!entries[1].active);
        shell.dispatch(AppAction::NextTab);
        assert_eq!(
            shell.window(window_id).expect("window").active_tab,
            Some(second)
        );
    }

    #[test]
    fn keyboard_close_returns_focus_to_other_tab() {
        let (mut shell, window_id, first, second) = window_with_two_tabs();
        shell.dispatch(AppAction::CloseTab);
        let window = shell.window(window_id).expect("window");
        assert_eq!(window.active_tab, Some(first));
        assert!(window.tab(second).is_none());
        let entries = tab_entries(window);
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0].title, "alpha");
        assert!(entries[0].active);
        assert_eq!(focus_after_close(&[first, second], second), Some(first));
    }

    #[test]
    fn select_and_close_helpers_agree_with_model() {
        let (mut shell, window_id, first, second) = window_with_two_tabs();
        assert!(shell.activate_tab(first));
        assert_eq!(
            shell.window(window_id).expect("window").active_tab,
            Some(first)
        );
        assert!(shell.close_tab_anywhere(window_id, first));
        assert_eq!(
            shell.window(window_id).expect("window").active_tab,
            Some(second)
        );
        assert_eq!(focus_after_close(&[first, second], first), Some(second));
        assert!(!shell.activate_tab(first));
    }
}

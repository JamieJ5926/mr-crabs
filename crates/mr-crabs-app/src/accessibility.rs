//! Accessibility: a headless snapshot of the shell's accessibility tree
//! plus the actions clients can perform on it.
//!
//! The snapshot is rebuilt from the model on demand (the model is the
//! source of truth), so tests can assert the tree structure and drive
//! focus/activate/close actions without a window.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::model::app_model::AppModel;
use crate::model::pane::PaneId;
use crate::model::tab::TabId;
use crate::model::window::WindowId;

/// Roles in the shell accessibility tree.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessibilityRole {
    Window,
    Tab,
    Pane,
    TerminalGrid,
    Popover,
    List,
    ListItem,
    TextInput,
}

/// Actions a client can perform on a node.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessibleAction {
    Focus,
    Activate,
    Close,
    ScrollUp,
    ScrollDown,
}

/// One node of the accessibility tree.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AccessibilityNode {
    pub id: usize,
    pub role: AccessibilityRole,
    pub label: String,
    pub value: Option<String>,
    pub actions: Vec<AccessibleAction>,
    pub children: Vec<AccessibilityNode>,
}

impl AccessibilityNode {
    pub fn new(id: usize, role: AccessibilityRole, label: impl Into<String>) -> Self {
        Self {
            id,
            role,
            label: label.into(),
            value: None,
            actions: Vec::new(),
            children: Vec::new(),
        }
    }

    pub fn with_value(mut self, value: impl Into<String>) -> Self {
        self.value = Some(value.into());
        self
    }

    pub fn with_actions(mut self, actions: Vec<AccessibleAction>) -> Self {
        self.actions = actions;
        self
    }

    pub fn find(&self, id: usize) -> Option<&AccessibilityNode> {
        if self.id == id {
            return Some(self);
        }
        self.children.iter().find_map(|child| child.find(id))
    }

    pub fn count(&self) -> usize {
        1 + self
            .children
            .iter()
            .map(|child| child.count())
            .sum::<usize>()
    }
}

/// Where a node lives in the model, so actions can be applied.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NodePath {
    Root,
    Window(WindowId),
    Tab(TabId),
    Pane(PaneId),
    PaletteItem(usize),
}

/// A point-in-time accessibility snapshot.
pub struct AccessibilitySnapshot {
    pub root: AccessibilityNode,
    pub generated_at: u64,
    paths: HashMap<usize, NodePath>,
}

impl AccessibilitySnapshot {
    /// Rebuild the snapshot from the model. Structure: root → window →
    /// tab strip (tabs) → focused tab's panes → terminal grid; the command
    /// palette appears as a popover when open.
    pub fn from_model(model: &AppModel) -> Self {
        let mut next_id = 0usize;
        let mut paths = HashMap::new();
        let mut root = AccessibilityNode::new(next_id, AccessibilityRole::Window, "Mr Crabs");
        paths.insert(next_id, NodePath::Root);
        next_id += 1;

        for window_id in &model.window_order {
            let Some(window) = model.windows.get(window_id) else {
                continue;
            };
            if !window.visible {
                continue;
            }
            let mut window_node =
                AccessibilityNode::new(next_id, AccessibilityRole::Window, window.window_title())
                    .with_actions(vec![AccessibleAction::Focus, AccessibleAction::Close]);
            paths.insert(next_id, NodePath::Window(*window_id));
            next_id += 1;

            for tab_id in &window.tab_order {
                let Some(tab) = window.tabs.get(tab_id) else {
                    continue;
                };
                let mut tab_node =
                    AccessibilityNode::new(next_id, AccessibilityRole::Tab, tab.title.clone())
                        .with_actions(vec![AccessibleAction::Focus, AccessibleAction::Activate]);
                paths.insert(next_id, NodePath::Tab(*tab_id));
                next_id += 1;

                if window.active_tab == Some(*tab_id) {
                    for pane_id in tab.tree.panes() {
                        let Some(pane) = tab.panes.get(&pane_id) else {
                            continue;
                        };
                        let grid = format!("{}x{}", pane.last_size.cols, pane.last_size.rows);
                        let dock_value = pane.input_dock().and_then(|snap| {
                            if snap.state
                                != crate::model::input_dock::InputDockState::ShellInputActive
                            {
                                return None;
                            }
                            let mut text = String::new();
                            for cell in snap.cells.iter() {
                                if let Some(ch) = char::from_u32(cell.content) {
                                    text.push(ch);
                                }
                            }
                            let trimmed = text.trim_end();
                            if trimmed.is_empty() {
                                None
                            } else {
                                Some(trimmed.to_string())
                            }
                        });
                        let mut pane_node = AccessibilityNode::new(
                            next_id,
                            AccessibilityRole::Pane,
                            pane.title.clone(),
                        )
                        .with_value(grid.clone())
                        .with_actions(vec![
                            AccessibleAction::Focus,
                            AccessibleAction::Activate,
                            AccessibleAction::Close,
                        ]);
                        paths.insert(next_id, NodePath::Pane(pane_id));
                        next_id += 1;
                        let grid_node = AccessibilityNode::new(
                            next_id,
                            AccessibilityRole::TerminalGrid,
                            "Terminal",
                        )
                        .with_value(dock_value.unwrap_or(grid))
                        .with_actions(vec![
                            AccessibleAction::Focus,
                            AccessibleAction::ScrollUp,
                            AccessibleAction::ScrollDown,
                        ]);
                        paths.insert(next_id, NodePath::Pane(pane_id));
                        next_id += 1;
                        pane_node.children.push(grid_node);
                        tab_node.children.push(pane_node);
                    }
                }
                window_node.children.push(tab_node);
            }
            root.children.push(window_node);
        }

        if model.palette.is_open() {
            let mut popover =
                AccessibilityNode::new(next_id, AccessibilityRole::Popover, "Command Palette");
            paths.insert(next_id, NodePath::Root);
            next_id += 1;
            let mut list = AccessibilityNode::new(next_id, AccessibilityRole::List, "Commands");
            paths.insert(next_id, NodePath::Root);
            next_id += 1;
            for (index, item) in model.palette.results.iter().enumerate() {
                let node = AccessibilityNode::new(
                    next_id,
                    AccessibilityRole::ListItem,
                    item.title.clone(),
                )
                .with_actions(vec![AccessibleAction::Activate]);
                paths.insert(next_id, NodePath::PaletteItem(index));
                next_id += 1;
                list.children.push(node);
            }
            popover.children.push(list);
            root.children.push(popover);
        }

        // Chat presentation overlay when any pane has effective Chat mode.
        if let Some(pid) = model.focused_pane_id() {
            if let Some((window_id, tab_id)) = model.locate_pane(pid) {
                if let Some(pane) = model
                    .window(window_id)
                    .and_then(|w| w.tabs.get(&tab_id))
                    .and_then(|t| t.pane(pid))
                {
                    if pane.effective_mode(model.palette.is_open(), false)
                        == crate::model::presentation::SurfaceMode::Chat
                    {
                        let events = pane.conversation_events(model.palette.is_open(), false);
                        let mut chat =
                            AccessibilityNode::new(next_id, AccessibilityRole::Popover, "Chat");
                        paths.insert(next_id, NodePath::Root);
                        next_id += 1;
                        let mut list = AccessibilityNode::new(
                            next_id,
                            AccessibilityRole::List,
                            "Conversation",
                        );
                        paths.insert(next_id, NodePath::Root);
                        next_id += 1;
                        for ev in &events {
                            let label = format!("{:?}: {}", ev.kind, ev.text);
                            let node =
                                AccessibilityNode::new(next_id, AccessibilityRole::ListItem, label)
                                    .with_actions(vec![]);
                            paths.insert(next_id, NodePath::Root);
                            next_id += 1;
                            list.children.push(node);
                        }
                        if list.children.is_empty() {
                            let node = AccessibilityNode::new(
                                next_id,
                                AccessibilityRole::ListItem,
                                "No conversation yet",
                            );
                            paths.insert(next_id, NodePath::Root);
                            next_id += 1;
                            list.children.push(node);
                        }
                        chat.children.push(list);
                        let composer = AccessibilityNode::new(
                            next_id,
                            AccessibilityRole::TextInput,
                            "Message",
                        )
                        .with_value(pane.chat_draft().to_owned());
                        paths.insert(next_id, NodePath::Root);
                        chat.children.push(composer);
                        root.children.push(chat);
                    }
                }
            }
        }

        root.actions = vec![AccessibleAction::Focus];
        Self {
            root,
            generated_at: model.generation,
            paths,
        }
    }

    pub fn node(&self, id: usize) -> Option<&AccessibilityNode> {
        self.root.find(id)
    }

    /// Perform an action on a node. Returns whether the shell performed it.
    pub fn perform(&self, model: &mut AppModel, node_id: usize, action: AccessibleAction) -> bool {
        let Some(path) = self.paths.get(&node_id) else {
            return false;
        };
        match (*path, action) {
            (NodePath::Pane(pane), AccessibleAction::Focus | AccessibleAction::Activate) => {
                model.focus_pane(pane)
            }
            (NodePath::Pane(pane), AccessibleAction::Close) => model.close_pane_anywhere(pane),
            (NodePath::Window(window), AccessibleAction::Focus | AccessibleAction::Activate) => {
                model.set_active_window(window)
            }
            (NodePath::Window(window), AccessibleAction::Close) => {
                model.close_window(window);
                true
            }
            (NodePath::Tab(tab), AccessibleAction::Focus | AccessibleAction::Activate) => {
                model.activate_tab(tab)
            }
            (NodePath::PaletteItem(index), AccessibleAction::Activate) => {
                model.palette.select_index(index);
                model.activate_palette_selection().is_some()
            }
            (
                NodePath::Pane(pane_id),
                action @ (AccessibleAction::ScrollUp | AccessibleAction::ScrollDown),
            ) => {
                let Some((window_id, tab_id)) = model.locate_pane(pane_id) else {
                    return false;
                };
                let Some(pane) = model
                    .window_mut(window_id)
                    .and_then(|window| window.tabs.get_mut(&tab_id))
                    .and_then(|tab| tab.pane_mut(pane_id))
                else {
                    return false;
                };
                let lines = usize::from(pane.last_size.rows).saturating_sub(1).max(1);
                match action {
                    AccessibleAction::ScrollUp => pane.scroll_viewport_up(lines),
                    AccessibleAction::ScrollDown => pane.scroll_viewport_down(lines),
                    _ => unreachable!(),
                }
                true
            }
            _ => false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::AppAction;
    use crate::model::split::PaneId;

    #[test]
    fn snapshot_mirrors_windows_tabs_panes_and_grids() {
        let mut model = AppModel::headless();
        model.dispatch(AppAction::NewSplitRight);
        let snapshot = model.accessibility_snapshot();
        // root + 1 window + 1 tab + 2 panes + 2 grids
        assert_eq!(snapshot.root.count(), 7);
        let window = &snapshot.root.children[0];
        assert_eq!(window.role, AccessibilityRole::Window);
        let tab = &window.children[0];
        assert_eq!(tab.role, AccessibilityRole::Tab);
        assert_eq!(tab.children.len(), 2, "one pane node per pane");
        let pane = &tab.children[0];
        assert_eq!(pane.role, AccessibilityRole::Pane);
        assert_eq!(pane.value.as_deref(), Some("80x24"));
        assert_eq!(pane.children.len(), 1);
        assert_eq!(pane.children[0].role, AccessibilityRole::TerminalGrid);
    }

    #[test]
    fn hidden_quick_terminal_is_not_in_the_tree() {
        let mut model = AppModel::headless();
        model.toggle_quick_terminal();
        model.toggle_quick_terminal(); // hidden again
        let snapshot = model.accessibility_snapshot();
        assert_eq!(snapshot.root.count(), 5, "only the main window remains");
    }

    #[test]
    fn palette_popover_appears_when_open() {
        let mut model = AppModel::headless();
        model.dispatch(AppAction::TogglePalette);
        let snapshot = model.accessibility_snapshot();
        assert!(
            snapshot
                .root
                .children
                .iter()
                .any(|node| node.role == AccessibilityRole::Popover)
        );
        let popover = snapshot
            .root
            .children
            .iter()
            .find(|node| node.role == AccessibilityRole::Popover)
            .expect("popover");
        assert_eq!(popover.children[0].role, AccessibilityRole::List);
        assert!(popover.children[0].children.len() >= 2);
        assert!(
            popover.children[0]
                .children
                .iter()
                .all(|item| item.role == AccessibilityRole::ListItem)
        );
    }

    #[test]
    fn activating_a_pane_node_focuses_it() {
        let mut model = AppModel::headless();
        model.dispatch(AppAction::NewSplitRight);
        let pane_ids = model.active_tab().unwrap().pane_ids();
        let first = pane_ids[0];
        let second = pane_ids[1];
        // Focus the first pane through the tree.
        let snapshot = model.accessibility_snapshot();
        let window = &snapshot.root.children[0];
        let tab = &window.children[0];
        let first_pane_node = tab.children.iter().find(|node| {
            node.role == AccessibilityRole::Pane
                && snapshot
                    .paths
                    .get(&node.id)
                    .is_some_and(|path| *path == NodePath::Pane(first))
        });
        let node_id = first_pane_node.expect("first pane node").id;
        assert!(snapshot.perform(&mut model, node_id, AccessibleAction::Activate));
        assert_eq!(model.focused_pane_id(), Some(first));
        assert_ne!(first, second);
    }

    #[test]
    fn closing_a_pane_node_cascades() {
        let mut model = AppModel::headless();
        model.dispatch(AppAction::NewSplitRight);
        let snapshot = model.accessibility_snapshot();
        let window = &snapshot.root.children[0];
        let tab = &window.children[0];
        let second_pane_node = tab.children.iter().find(|node| {
            node.role == AccessibilityRole::Pane
                && snapshot
                    .paths
                    .get(&node.id)
                    .is_some_and(|path| *path == NodePath::Pane(PaneId::new(2)))
        });
        let node_id = second_pane_node.expect("second pane node").id;
        assert!(snapshot.perform(&mut model, node_id, AccessibleAction::Close));
        assert_eq!(model.active_tab().unwrap().pane_count(), 1);
    }

    #[test]
    fn pane_scroll_actions_move_the_terminal_viewport() {
        let mut model = AppModel::headless();
        let pane_id = model.focused_pane_id().expect("focused pane");
        let mut output = Vec::new();
        for line in 0..40 {
            output.extend_from_slice(format!("line-{line}\r\n").as_bytes());
        }
        model
            .focused_pane_mut()
            .expect("pane")
            .feed_test_output(&output)
            .expect("accessibility fixture feed should succeed");
        let snapshot = model.accessibility_snapshot();
        let pane_node = snapshot.root.children[0].children[0]
            .children
            .iter()
            .find(|node| snapshot.paths.get(&node.id) == Some(&NodePath::Pane(pane_id)))
            .expect("pane node");

        assert!(snapshot.perform(&mut model, pane_node.id, AccessibleAction::ScrollUp));
        assert!(model.focused_pane().expect("pane").viewport_offset() > 0);
        assert!(snapshot.perform(&mut model, pane_node.id, AccessibleAction::ScrollDown));
        assert_eq!(model.focused_pane().expect("pane").viewport_offset(), 0);
    }

    #[test]
    fn unknown_nodes_and_scroll_actions_are_refused() {
        let mut model = AppModel::headless();
        let snapshot = model.accessibility_snapshot();
        assert!(!snapshot.perform(&mut model, 9999, AccessibleAction::Focus));
        assert!(
            !snapshot.perform(&mut model, 0, AccessibleAction::ScrollDown),
            "shell does not claim grid scrolling"
        );
    }

    #[test]
    fn palette_item_activation_dispatches() {
        let mut model = AppModel::headless();
        model.dispatch(AppAction::TogglePalette);
        model.palette.set_query("new tab", &model.commands);
        let snapshot = model.accessibility_snapshot();
        let popover = snapshot
            .root
            .children
            .iter()
            .find(|node| node.role == AccessibilityRole::Popover)
            .expect("popover");
        let item = &popover.children[0].children[0];
        let tabs_before = model.active_window().unwrap().tabs.len();
        assert!(snapshot.perform(&mut model, item.id, AccessibleAction::Activate));
        assert_eq!(model.active_window().unwrap().tabs.len(), tabs_before + 1);
        assert!(!model.palette.is_open(), "activation closes the palette");
    }
}

#[test]
fn chat_overlay_appears_when_effective_chat() {
    let mut model = crate::model::app_model::AppModel::headless();
    // Make pane eligible: feed OSC133 A and put cursor at bottom row with prompt
    model
        .focused_pane_mut()
        .expect("pane")
        .feed_test_output(b"\x1b]133;A\x07\x1b]133;B\x07hello")
        .expect("feed");
    // Toggle chat
    let res = model.dispatch(crate::action::AppAction::ToggleChatPresentation);
    assert!(
        res.performed,
        "chat toggle should succeed when eligible: {}",
        res.note
    );
    let pane_id = model.focused_pane_id().expect("pane");
    assert!(model.insert_chat_text(pane_id, "hello agent"));
    let snapshot = model.accessibility_snapshot();
    assert!(
        snapshot.root.children.iter().any(|n| n.label == "Chat"),
        "chat popover should appear in a11y tree"
    );
    let chat = snapshot
        .root
        .children
        .iter()
        .find(|node| node.label == "Chat")
        .expect("chat");
    let composer = chat
        .children
        .iter()
        .find(|node| node.role == AccessibilityRole::TextInput)
        .expect("composer");
    assert_eq!(composer.value.as_deref(), Some("hello agent"));
    // Toggle back
    let res2 = model.dispatch(crate::action::AppAction::ToggleChatPresentation);
    assert!(res2.performed);
    let snapshot2 = model.accessibility_snapshot();
    assert!(
        !snapshot2.root.children.iter().any(|n| n.label == "Chat"),
        "chat should hide after second toggle"
    );
}

#[test]
fn empty_chat_placeholder_and_composer_have_unique_ids() {
    let mut model = crate::model::app_model::AppModel::headless();
    model
        .focused_pane_mut()
        .expect("pane")
        .feed_test_output(b"\x1b]133;A\x07\x1b]133;B\x07")
        .expect("feed");
    assert!(
        model
            .dispatch(crate::action::AppAction::ToggleChatPresentation)
            .performed
    );
    let snapshot = model.accessibility_snapshot();
    let chat = snapshot
        .root
        .children
        .iter()
        .find(|node| node.label == "Chat")
        .expect("chat");
    let placeholder = &chat.children[0].children[0];
    let composer = chat
        .children
        .iter()
        .find(|node| node.role == AccessibilityRole::TextInput)
        .expect("composer");
    assert_ne!(placeholder.id, composer.id);
}

#[test]
fn chat_toggle_fails_closed_when_not_eligible() {
    let mut model = crate::model::app_model::AppModel::headless();
    // No OSC133, so not eligible
    let res = model.dispatch(crate::action::AppAction::ToggleChatPresentation);
    assert!(
        !res.performed,
        "chat toggle must fail closed when not eligible"
    );
}

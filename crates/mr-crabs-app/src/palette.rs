//! Command palette: an action registry plus the palette UI state.
//!
//! Every shell action is registered as a palette command
//! (`shell.<action_name>`) by [`CommandRegistry::install_shell_commands`],
//! so the palette, menus, keybindings, and accessibility all dispatch
//! through the same single registry — exactly once per action.
//!
//! The registry is pure and headless; dispatch runs a stored closure with
//! the `AppModel`, which keeps palette activation testable without GPUI.

use std::collections::BTreeMap;
use std::sync::Arc;
#[cfg(test)]
use std::sync::atomic::{AtomicBool, Ordering};

use crate::action::AppAction;
use crate::model::app_model::AppModel;

/// Maximum palette results shown for a query.
pub const PALETTE_RESULT_LIMIT: usize = 50;

/// A palette command: a stable id, a human title, an optional key
/// equivalent, and the runnable.
#[derive(Clone)]
pub struct Command {
    pub id: String,
    pub title: String,
    pub keys: Option<String>,
    run: Arc<dyn Fn(&mut AppModel)>,
}

impl Command {
    pub fn new(
        id: impl Into<String>,
        title: impl Into<String>,
        run: impl Fn(&mut AppModel) + 'static,
    ) -> Self {
        Self {
            id: id.into(),
            title: title.into(),
            keys: None,
            run: Arc::new(run),
        }
    }

    pub fn with_keys(mut self, keys: Option<String>) -> Self {
        self.keys = keys;
        self
    }

    /// Run this command against the model.
    pub fn run(&self, model: &mut AppModel) {
        (self.run)(model);
    }
}

/// A search result: the command plus its deterministic score.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct CommandMatch {
    pub id: String,
    pub title: String,
    pub keys: Option<String>,
    /// Higher is a better match.
    pub score: u32,
    /// Registration order, used to break ties deterministically.
    pub order: usize,
}

/// Ordered action registry.
pub struct CommandRegistry {
    commands: BTreeMap<String, Command>,
    order: Vec<String>,
}

impl Default for CommandRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl CommandRegistry {
    pub fn new() -> Self {
        Self {
            commands: BTreeMap::new(),
            order: Vec::new(),
        }
    }

    pub fn register(&mut self, command: Command) {
        if !self.commands.contains_key(&command.id) {
            self.order.push(command.id.clone());
        }
        self.commands.insert(command.id.clone(), command);
    }

    /// Remove a command; returns whether it existed.
    pub fn unregister(&mut self, id: &str) -> bool {
        let existed = self.commands.remove(id).is_some();
        self.order.retain(|existing| existing != id);
        existed
    }

    pub fn get(&self, id: &str) -> Option<&Command> {
        self.commands.get(id)
    }

    pub fn contains(&self, id: &str) -> bool {
        self.commands.contains_key(id)
    }

    pub fn len(&self) -> usize {
        self.commands.len()
    }

    pub fn is_empty(&self) -> bool {
        self.commands.is_empty()
    }

    /// Dispatch by id; returns whether a command ran.
    pub fn dispatch(&self, id: &str, model: &mut AppModel) -> bool {
        let Some(command) = self.commands.get(id) else {
            return false;
        };
        command.run(model);
        true
    }

    /// Deterministic search: exact id > exact title > title prefix > title
    /// contains > id contains; ties break by registration order.
    pub fn search(&self, query: &str, limit: usize) -> Vec<CommandMatch> {
        let query = query.trim().to_lowercase();
        let mut matches: Vec<CommandMatch> = self
            .order
            .iter()
            .enumerate()
            .filter_map(|(order, id)| {
                let command = self.commands.get(id)?;
                let title_lower = command.title.to_lowercase();
                let score = if command.id.to_lowercase() == query {
                    1000
                } else if title_lower == query {
                    900
                } else if title_lower.starts_with(&query) {
                    800
                } else if title_lower.contains(&query) {
                    500
                } else if command.id.to_lowercase().contains(&query) {
                    300
                } else {
                    return None;
                };
                Some(CommandMatch {
                    id: command.id.clone(),
                    title: command.title.clone(),
                    keys: command.keys.clone(),
                    score,
                    order,
                })
            })
            .collect();
        matches.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.order.cmp(&b.order)));
        matches.truncate(limit);
        matches
    }

    /// Register one palette command per shell action plus the update-check
    /// action (already part of the action set). Ids are `shell.<name>`.
    pub fn install_shell_commands(&mut self) {
        let bindings = crate::keymap::default_keybindings();
        for action in AppAction::ALL {
            let keys = bindings
                .iter()
                .find(|binding| binding.action == action)
                .map(|binding| binding.keys.clone());
            let id = format!("shell.{}", action.name());
            let title = action.title().to_string();
            self.register(
                Command::new(id, title, move |model| {
                    model.dispatch(action);
                })
                .with_keys(keys),
            );
        }
    }
}

/// Palette UI state. Kept separate from the registry so the registry is
/// immutable during a search.
#[derive(Clone, Debug)]
pub struct PaletteState {
    pub open: bool,
    pub query: String,
    pub selection: usize,
    pub results: Vec<CommandMatch>,
    pub last_dispatched: Option<String>,
}

impl Default for PaletteState {
    fn default() -> Self {
        Self::new()
    }
}

impl PaletteState {
    pub fn new() -> Self {
        Self {
            open: false,
            query: String::new(),
            selection: 0,
            results: Vec::new(),
            last_dispatched: None,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Open the palette and list every command.
    pub fn open(&mut self, registry: &CommandRegistry) {
        self.open = true;
        self.selection = 0;
        let query = self.query.clone();
        self.set_query(&query, registry);
    }

    pub fn close(&mut self) {
        self.open = false;
        self.query.clear();
        self.selection = 0;
        self.results.clear();
    }

    /// Returns the new open state.
    pub fn toggle(&mut self, registry: &CommandRegistry) -> bool {
        if self.open {
            self.close();
        } else {
            self.open(registry);
        }
        self.open
    }

    /// Re-run the search for a query and keep the selection in bounds.
    pub fn set_query(&mut self, query: &str, registry: &CommandRegistry) {
        self.query = query.to_string();
        self.results = registry.search(query, PALETTE_RESULT_LIMIT);
        self.clamp_selection();
    }

    pub fn type_char(&mut self, ch: char, registry: &CommandRegistry) {
        let mut query = self.query.clone();
        query.push(ch);
        self.set_query(&query, registry);
    }

    pub fn backspace(&mut self, registry: &CommandRegistry) {
        self.query.pop();
        let query = self.query.clone();
        self.set_query(&query, registry);
    }

    pub fn move_selection(&mut self, delta: isize) {
        if self.results.is_empty() {
            self.selection = 0;
            return;
        }
        let len = self.results.len() as isize;
        let next = (self.selection as isize + delta).rem_euclid(len);
        self.selection = next as usize;
    }

    pub fn select_index(&mut self, index: usize) {
        self.selection = index.min(self.results.len().saturating_sub(1));
    }

    pub fn selected(&self) -> Option<&CommandMatch> {
        self.results.get(self.selection)
    }

    fn clamp_selection(&mut self) {
        if self.selection >= self.results.len() {
            self.selection = self.results.len().saturating_sub(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::app_model::AppModel;

    #[test]
    fn shell_commands_cover_every_action_exactly_once() {
        let mut registry = CommandRegistry::new();
        registry.install_shell_commands();
        assert_eq!(registry.len(), AppAction::ALL.len());
        for action in AppAction::ALL {
            let id = format!("shell.{}", action.name());
            assert!(registry.contains(&id), "missing command for {action:?}");
        }
        // Unregistering removes the id exactly once.
        assert!(registry.unregister("shell.quit"));
        assert!(!registry.unregister("shell.quit"));
        assert_eq!(registry.len(), AppAction::ALL.len() - 1);
    }

    #[test]
    fn search_ranks_exact_over_prefix_over_substring() {
        let mut registry = CommandRegistry::new();
        registry.register(Command::new("shell.new_tab", "New Tab", |_| {}));
        registry.register(Command::new("shell.next_tab", "New Tab in Window", |_| {}));
        registry.register(Command::new("shell.tabulate", "Create New Tab", |_| {}));
        let results = registry.search("new tab", 10);
        assert_eq!(results[0].id, "shell.new_tab", "exact title wins");
        assert_eq!(
            results[1].id, "shell.next_tab",
            "title prefix beats substring"
        );
        assert_eq!(results[2].id, "shell.tabulate", "substring follows prefix");
        assert!(registry.search("zzz", 10).is_empty());
    }

    #[test]
    fn empty_query_lists_all_commands_in_registration_order() {
        let mut registry = CommandRegistry::new();
        registry.register(Command::new("a", "Alpha", |_| {}));
        registry.register(Command::new("b", "Beta", |_| {}));
        registry.register(Command::new("c", "Gamma", |_| {}));
        let results = registry.search("", 10);
        let ids: Vec<&str> = results.iter().map(|m| m.id.as_str()).collect();
        assert_eq!(ids, vec!["a", "b", "c"]);
    }

    #[test]
    fn dispatch_runs_the_registered_closure() {
        let mut registry = CommandRegistry::new();
        let flag = Arc::new(AtomicBool::new(false));
        let command_flag = flag.clone();
        registry.register(Command::new("flag", "Flag", move |_| {
            command_flag.store(true, Ordering::Relaxed)
        }));
        let mut model = AppModel::headless();
        assert!(registry.dispatch("flag", &mut model));
        assert!(
            flag.load(Ordering::Relaxed),
            "the command closure must have run"
        );
        assert!(!registry.dispatch("missing", &mut model));
    }

    #[test]
    fn palette_open_query_navigate_and_activate() {
        let mut registry = CommandRegistry::new();
        registry.register(Command::new("one", "First Command", |_| {}));
        registry.register(Command::new("two", "Second Command", |_| {}));
        let mut palette = PaletteState::new();
        assert!(!palette.is_open());

        assert!(palette.toggle(&registry));
        assert!(palette.is_open());
        assert_eq!(palette.results.len(), 2);

        palette.set_query("second", &registry);
        assert_eq!(palette.results.len(), 1);
        assert_eq!(palette.results[0].id, "two");
        palette.select_index(0);

        palette.set_query("", &registry);
        assert_eq!(palette.results.len(), 2);
        palette.move_selection(1);
        assert_eq!(palette.selected().unwrap().id, "two");
        palette.move_selection(-1);
        assert_eq!(palette.selected().unwrap().id, "one");
        // Selection wraps.
        palette.move_selection(-1);
        assert_eq!(palette.selected().unwrap().id, "two");

        palette.type_char('f', &registry);
        assert_eq!(palette.query, "f");
        assert_eq!(palette.results[0].id, "one");
        palette.backspace(&registry);
        assert_eq!(palette.query, "");
        assert_eq!(palette.results.len(), 2);

        assert!(!palette.toggle(&registry));
        assert!(!palette.is_open());
        assert!(palette.results.is_empty(), "closing clears results");
    }

    #[test]
    fn palette_activation_dispatches_shell_commands() {
        let mut model = AppModel::headless();
        assert_eq!(model.windows.len(), 1);
        let tabs_before = model.active_window().unwrap().tabs.len();
        model.palette.toggle(&model.commands);
        model.palette.set_query("new tab", &model.commands);
        assert_eq!(model.palette.selected().unwrap().id, "shell.new_tab");
        let dispatched = model.activate_palette_selection();
        assert_eq!(dispatched.as_deref(), Some("shell.new_tab"));
        assert_eq!(model.active_window().unwrap().tabs.len(), tabs_before + 1);
        assert!(!model.palette.is_open(), "activation closes the palette");
        assert_eq!(
            model.palette.last_dispatched.as_deref(),
            Some("shell.new_tab")
        );
    }
    #[test]
    fn animation_commands_are_discoverable_with_frozen_ids() {
        let mut registry = CommandRegistry::new();
        registry.install_shell_commands();

        let matches = registry.search("Text Animation", 10);
        let ids: Vec<&str> = matches.iter().map(|result| result.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                "shell.set_text_animation_none",
                "shell.set_text_animation_streaming",
                "shell.set_text_animation_typewriter",
            ]
        );

        let trail = registry.search("Toggle Cursor Trail", 10);
        assert_eq!(trail.len(), 1);
        assert_eq!(trail[0].id, "shell.toggle_cursor_trail");
    }

    #[test]
    fn palette_activation_applies_typewriter_through_real_dispatch() {
        use mr_crabs_config::TextAnimation;

        let mut model = AppModel::headless();
        model.palette.toggle(&model.commands);
        model
            .palette
            .set_query("Text Animation: Typewriter", &model.commands);
        assert_eq!(
            model.palette.selected().map(|result| result.id.as_str()),
            Some("shell.set_text_animation_typewriter")
        );

        let dispatched = model.activate_palette_selection();
        assert_eq!(
            dispatched.as_deref(),
            Some("shell.set_text_animation_typewriter")
        );
        assert!(!model.palette.is_open());
        assert_eq!(
            model.palette.last_dispatched.as_deref(),
            Some("shell.set_text_animation_typewriter")
        );
        assert_eq!(model.settings.current().text_animation, "typewriter");
        assert_eq!(
            model
                .focused_pane()
                .expect("focused pane")
                .core
                .animation_defaults()
                .text_animation,
            TextAnimation::Typewriter
        );
    }

    #[test]
    fn palette_activation_toggles_cursor_trail_through_real_dispatch() {
        let mut model = AppModel::headless();
        let before = model.settings.current().cursor_trail;
        model.palette.toggle(&model.commands);
        model
            .palette
            .set_query("Toggle Cursor Trail", &model.commands);
        assert_eq!(
            model.palette.selected().map(|result| result.id.as_str()),
            Some("shell.toggle_cursor_trail")
        );

        let dispatched = model.activate_palette_selection();
        assert_eq!(dispatched.as_deref(), Some("shell.toggle_cursor_trail"));
        assert!(!model.palette.is_open());
        assert_eq!(
            model.palette.last_dispatched.as_deref(),
            Some("shell.toggle_cursor_trail")
        );
        assert_eq!(model.settings.current().cursor_trail, !before);
        assert_eq!(
            model
                .focused_pane()
                .expect("focused pane")
                .core
                .animation_defaults()
                .cursor_trail,
            !before
        );
    }
}

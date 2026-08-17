//! Dock behavior: dock menu, recent documents, activation policy, and
//! reopen handling.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::action::AppAction;
#[cfg(test)]
use crate::model::app_model::AppModel;
use crate::model::window::WindowId;

/// How the app presents in the dock.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivationPolicy {
    /// A normal dock app.
    Regular,
    /// No dock icon.
    Accessory,
    /// Cannot be activated.
    Prohibited,
}

/// One dock menu item.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum DockMenuItem {
    Action { name: String, action: AppAction },
    Separator,
}

impl DockMenuItem {
    pub fn action(name: impl Into<String>, action: AppAction) -> Self {
        Self::Action {
            name: name.into(),
            action,
        }
    }
}

/// What a dock reopen (clicking the dock icon) does.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReopenAction {
    /// Open a new window (Ghostty behavior when no window exists).
    NewWindow,
    /// Just activate the app.
    ActivateOnly,
}

/// Outcome of handling a dock reopen.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DockOutcome {
    NewWindowCreated(WindowId),
    Activated,
    NoWindows,
}

/// Bound on recent documents.
pub const RECENT_DOCUMENTS_CAP: usize = 10;

/// Dock behavior model.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct DockBehavior {
    pub shows_badge: bool,
    pub items: Vec<DockMenuItem>,
    pub recent_documents: Vec<PathBuf>,
    pub activation_policy: ActivationPolicy,
    pub reopen_action: ReopenAction,
}

impl DockBehavior {
    /// Default shell dock: a "New Window" item and recent documents.
    pub fn default_shell() -> Self {
        Self {
            shows_badge: false,
            items: vec![DockMenuItem::action("New Window", AppAction::NewWindow)],
            recent_documents: Vec::new(),
            activation_policy: ActivationPolicy::Regular,
            reopen_action: ReopenAction::NewWindow,
        }
    }

    /// Add a recent document, deduplicated and bounded to
    /// [`RECENT_DOCUMENTS_CAP`] newest-first entries.
    pub fn add_recent_document(&mut self, path: PathBuf) {
        self.recent_documents.retain(|existing| existing != &path);
        self.recent_documents.insert(0, path);
        self.recent_documents.truncate(RECENT_DOCUMENTS_CAP);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_shell_has_new_window_item() {
        let dock = DockBehavior::default_shell();
        assert_eq!(dock.activation_policy, ActivationPolicy::Regular);
        assert_eq!(dock.reopen_action, ReopenAction::NewWindow);
        assert!(dock.items.iter().any(|item| matches!(
            item,
            DockMenuItem::Action {
                action: AppAction::NewWindow,
                ..
            }
        )));
    }

    #[test]
    fn recent_documents_dedupe_and_stay_bounded() {
        let mut dock = DockBehavior::default_shell();
        for i in 0..15 {
            dock.add_recent_document(PathBuf::from(format!("/tmp/doc-{i}")));
        }
        assert_eq!(dock.recent_documents.len(), RECENT_DOCUMENTS_CAP);
        assert_eq!(dock.recent_documents[0], PathBuf::from("/tmp/doc-14"));
        dock.add_recent_document(PathBuf::from("/tmp/doc-5"));
        assert_eq!(dock.recent_documents.len(), RECENT_DOCUMENTS_CAP);
        assert_eq!(dock.recent_documents[0], PathBuf::from("/tmp/doc-5"));
        let count = dock
            .recent_documents
            .iter()
            .filter(|path| path.as_path() == std::path::Path::new("/tmp/doc-5"))
            .count();
        assert_eq!(count, 1, "deduplicated");
    }

    #[test]
    fn activate_only_policy_does_not_create_windows() {
        let mut model = AppModel::headless();
        model.dock.reopen_action = ReopenAction::ActivateOnly;
        // With an existing window, activate-only just activates.
        assert_eq!(model.handle_reopen(), DockOutcome::Activated);
        // With no windows, even activate-only creates one (there is nothing
        // to activate).
        let window_id = model.active_window.unwrap();
        model.close_window(window_id);
        assert!(matches!(
            model.handle_reopen(),
            DockOutcome::NewWindowCreated(_)
        ));
    }
}

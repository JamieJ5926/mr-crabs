//! Versioned shell-state persistence and restore.
//!
//! A snapshot is a plain serde structure: windows → tabs → panes + the
//! recursive split tree, with focus ids. PTY processes are never
//! serialized; restore rebuilds panes (detached, or spawned when the
//! platform supports PTYs) and validates that the tree, pane map, and
//! focus ids agree exactly.

use std::collections::BTreeMap;
use std::fmt::{self, Display, Formatter};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use mr_crabs_terminal::GridSize;
use serde::{Deserialize, Serialize};

use crate::model::app_model::AppModel;
use crate::model::pane::PaneId;
use crate::model::split::{SplitAxis, SplitTree};
use crate::model::tab::{TabError, TabId, TabModel};
use crate::model::window::{WindowError, WindowId, WindowModel};

/// The current shell-state schema version.
pub const SHELL_STATE_VERSION: u32 = 1;

/// A full shell snapshot (v1).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ShellStateV1 {
    pub version: u32,
    /// Index into `windows`; `None` when no window was active.
    pub active_window: Option<usize>,
    pub windows: Vec<WindowStateV1>,
}

/// One window's snapshot.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WindowStateV1 {
    pub id: u64,
    pub title: String,
    pub size: GridSize,
    pub active_tab: Option<usize>,
    pub tabs: Vec<TabStateV1>,
    pub is_quick_terminal: bool,
}

/// One tab's snapshot.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TabStateV1 {
    pub id: u64,
    pub title: String,
    pub panes: BTreeMap<u64, PaneStateV1>,
    pub tree: SplitTreeStateV1,
    pub focused_pane: Option<u64>,
}

/// One pane's snapshot (terminal state is not serialized; sessions are
/// respawned).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PaneStateV1 {
    pub id: u64,
    pub title: String,
}

/// The recursive split tree in snapshot form.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", content = "value", rename_all = "snake_case")]
pub enum SplitTreeStateV1 {
    Leaf(u64),
    Split {
        axis: SplitAxis,
        ratio: f32,
        first: Box<SplitTreeStateV1>,
        second: Box<SplitTreeStateV1>,
    },
}

impl From<&SplitTree> for SplitTreeStateV1 {
    fn from(tree: &SplitTree) -> Self {
        match tree {
            SplitTree::Leaf(pane) => SplitTreeStateV1::Leaf(pane.as_u64()),
            SplitTree::Split {
                axis,
                ratio,
                first,
                second,
            } => SplitTreeStateV1::Split {
                axis: *axis,
                ratio: *ratio,
                first: Box::new(SplitTreeStateV1::from(first.as_ref())),
                second: Box::new(SplitTreeStateV1::from(second.as_ref())),
            },
        }
    }
}

/// Errors from persistence and restore.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum RestoreError {
    Io(String),
    Parse(String),
    UnsupportedVersion(u32),
    InvalidState(String),
}

impl Display for RestoreError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            RestoreError::Io(message) => write!(f, "shell state io error: {message}"),
            RestoreError::Parse(message) => write!(f, "shell state parse error: {message}"),
            RestoreError::UnsupportedVersion(version) => {
                write!(f, "unsupported shell state version {version}")
            }
            RestoreError::InvalidState(message) => write!(f, "invalid shell state: {message}"),
        }
    }
}

impl std::error::Error for RestoreError {}

impl From<TabError> for RestoreError {
    fn from(error: TabError) -> Self {
        RestoreError::InvalidState(match error {
            TabError::InvalidState(message) => message,
        })
    }
}

impl From<WindowError> for RestoreError {
    fn from(error: WindowError) -> Self {
        RestoreError::InvalidState(match error {
            WindowError::InvalidState(message) => message,
        })
    }
}

/// The persistence store.
#[derive(Clone, Debug)]
pub struct RestoreStore {
    pub path: Option<PathBuf>,
    pub last_saved: Option<SystemTime>,
    pub save_count: u64,
    pub restore_count: u64,
}

impl Default for RestoreStore {
    fn default() -> Self {
        Self::new()
    }
}

impl RestoreStore {
    pub fn new() -> Self {
        Self {
            path: None,
            last_saved: None,
            save_count: 0,
            restore_count: 0,
        }
    }

    pub fn at(path: PathBuf) -> Self {
        Self {
            path: Some(path),
            ..Self::new()
        }
    }

    /// The default shell-state location for the current user
    /// (`~/Library/Application Support/dev.jamie.mr-crabs/shell-state.json`
    /// on macOS).
    pub fn default_path() -> Option<PathBuf> {
        let home = std::env::var_os("HOME")?;
        let base = PathBuf::from(home);
        #[cfg(target_os = "macos")]
        let dir = base.join("Library/Application Support/dev.jamie.mr-crabs");
        #[cfg(not(target_os = "macos"))]
        let dir = base.join(".config/mr-crabs");
        Some(dir.join("shell-state.json"))
    }

    /// Snapshot the model without touching disk.
    pub fn snapshot(&self, model: &AppModel) -> ShellStateV1 {
        let mut windows = Vec::with_capacity(model.window_order.len());
        for window_id in &model.window_order {
            let Some(window) = model.windows.get(window_id) else {
                continue;
            };
            let mut tabs = Vec::with_capacity(window.tab_order.len());
            for tab_id in &window.tab_order {
                let Some(tab) = window.tabs.get(tab_id) else {
                    continue;
                };
                let panes = tab
                    .panes
                    .iter()
                    .map(|(pane_id, pane)| {
                        (
                            pane_id.as_u64(),
                            PaneStateV1 {
                                id: pane_id.as_u64(),
                                title: pane.title.clone(),
                            },
                        )
                    })
                    .collect();
                tabs.push(TabStateV1 {
                    id: tab.id.as_u64(),
                    title: tab.title.clone(),
                    panes,
                    tree: SplitTreeStateV1::from(&tab.tree),
                    focused_pane: tab.focused_pane_id().map(PaneId::as_u64),
                });
            }
            let active_tab = window
                .active_tab
                .and_then(|tab_id| window.tab_order.iter().position(|id| *id == tab_id));
            // The serialized size comes from the committed window grid,
            // falling back to the active pane's current grid and then the
            // settings default grid only for an unmeasured window. Restored
            // panes are still created before native measurement (Wave 2),
            // so the versioned field remains the restore-time input.
            let size = window
                .grid()
                .or_else(|| {
                    window
                        .active_tab()
                        .and_then(|tab| tab.focused_pane())
                        .map(|pane| pane.last_size)
                })
                .unwrap_or(model.settings.current().default_grid);
            windows.push(WindowStateV1 {
                id: window.id.as_u64(),
                title: window.title.clone(),
                size,
                active_tab,
                tabs,
                is_quick_terminal: window.is_quick_terminal,
            });
        }
        let active_window = model
            .active_window
            .and_then(|window_id| model.window_order.iter().position(|id| *id == window_id));
        ShellStateV1 {
            version: SHELL_STATE_VERSION,
            active_window,
            windows,
        }
    }

    /// Save a snapshot to the configured path (or the given one).
    pub fn save(&mut self, model: &AppModel) -> Result<PathBuf, RestoreError> {
        let path = self
            .path
            .clone()
            .ok_or_else(|| RestoreError::Io("no restore path configured".to_string()))?;
        self.save_to(model, &path)
    }

    /// Save a snapshot to a specific path (atomic write: temp file then
    /// rename).
    pub fn save_to(&mut self, model: &AppModel, path: &Path) -> Result<PathBuf, RestoreError> {
        let state = self.snapshot(model);
        let json =
            serde_json::to_string_pretty(&state).map_err(|e| RestoreError::Parse(e.to_string()))?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| RestoreError::Io(e.to_string()))?;
        }
        let temp = path.with_extension("json.tmp");
        std::fs::write(&temp, json).map_err(|e| RestoreError::Io(e.to_string()))?;
        std::fs::rename(&temp, path).map_err(|e| RestoreError::Io(e.to_string()))?;
        self.last_saved = Some(SystemTime::now());
        self.save_count += 1;
        Ok(path.to_path_buf())
    }

    /// Load and version-check a snapshot.
    pub fn load(&self, path: &Path) -> Result<ShellStateV1, RestoreError> {
        let contents =
            std::fs::read_to_string(path).map_err(|e| RestoreError::Io(e.to_string()))?;
        let state: ShellStateV1 =
            serde_json::from_str(&contents).map_err(|e| RestoreError::Parse(e.to_string()))?;
        if state.version != SHELL_STATE_VERSION {
            return Err(RestoreError::UnsupportedVersion(state.version));
        }
        Ok(state)
    }

    /// Apply a snapshot to a model, rebuilding windows/tabs/panes. Panes are
    /// created detached; the shell spawns real sessions afterwards when the
    /// platform supports PTYs.
    pub fn apply(&mut self, model: &mut AppModel, state: ShellStateV1) -> Result<(), RestoreError> {
        if state.version != SHELL_STATE_VERSION {
            return Err(RestoreError::UnsupportedVersion(state.version));
        }
        model.windows.clear();
        model.window_order.clear();
        model.active_window = None;
        model.quit_requested = false;

        let mut max_window = 0u64;
        let mut max_tab = 0u64;
        let mut max_pane = 0u64;

        for window_state in &state.windows {
            max_window = max_window.max(window_state.id);
            let mut tabs = BTreeMap::new();
            let mut tab_order = Vec::new();
            for tab_state in &window_state.tabs {
                max_tab = max_tab.max(tab_state.id);
                let mut panes = BTreeMap::new();
                for (pane_id, pane_state) in &tab_state.panes {
                    max_pane = max_pane.max(*pane_id);
                    let pane_id = PaneId::new(*pane_id);
                    let mut pane = model.new_pane_with_id(pane_id, window_state.size);
                    pane.title = pane_state.title.clone();
                    panes.insert(pane_id, pane);
                }
                let tree = restore_tree(&tab_state.tree, &panes)?;
                let focused_pane = tab_state.focused_pane.map(PaneId::new);
                let tab = TabModel::from_parts(
                    TabId::new(tab_state.id),
                    tab_state.title.clone(),
                    panes,
                    tree,
                    focused_pane,
                )?;
                tab_order.push(tab.id);
                tabs.insert(tab.id, tab);
            }
            let active_tab = match window_state.active_tab {
                Some(index) if index < tab_order.len() => Some(tab_order[index]),
                _ => None,
            };
            // Restored windows carry no measured geometry until the live
            // view commits it before the first render.
            let window = WindowModel::from_parts(
                WindowId::new(window_state.id),
                window_state.title.clone(),
                tabs,
                tab_order,
                active_tab,
                window_state.is_quick_terminal,
            )?;
            model.window_order.push(window.id);
            model.windows.insert(window.id, window);
        }

        model.active_window = state
            .active_window
            .filter(|index| *index < model.window_order.len())
            .map(|index| model.window_order[index]);
        if model.active_window.is_none() && !model.window_order.is_empty() {
            model.active_window = model.window_order.last().copied();
        }
        model.reserve_ids(max_window, max_tab, max_pane);
        model.generation += 1;
        self.restore_count += 1;
        Ok(())
    }
}

/// Rebuild a split tree from its snapshot, validating that every leaf pane
/// exists in the pane map and that every pane appears exactly once.
fn restore_tree(
    state: &SplitTreeStateV1,
    panes: &BTreeMap<PaneId, crate::model::pane::PaneModel>,
) -> Result<SplitTree, RestoreError> {
    let mut used = Vec::new();
    let tree = restore_tree_impl(state, panes, &mut used)?;
    let mut used_sorted = used.clone();
    used_sorted.sort_unstable();
    let mut expected: Vec<u64> = panes.keys().map(|pane| pane.as_u64()).collect();
    expected.sort_unstable();
    if used_sorted != expected {
        return Err(RestoreError::InvalidState(
            "split tree leaves must match the pane map exactly once".to_string(),
        ));
    }
    Ok(tree)
}

fn restore_tree_impl(
    state: &SplitTreeStateV1,
    panes: &BTreeMap<PaneId, crate::model::pane::PaneModel>,
    used: &mut Vec<u64>,
) -> Result<SplitTree, RestoreError> {
    match state {
        SplitTreeStateV1::Leaf(pane_id) => {
            let pane_id = PaneId::new(*pane_id);
            if !panes.contains_key(&pane_id) {
                return Err(RestoreError::InvalidState(format!(
                    "split tree references missing pane {pane_id:?}"
                )));
            }
            if used.contains(&pane_id.as_u64()) {
                return Err(RestoreError::InvalidState(format!(
                    "split tree references pane {pane_id:?} more than once"
                )));
            }
            used.push(pane_id.as_u64());
            Ok(SplitTree::leaf(pane_id))
        }
        SplitTreeStateV1::Split {
            axis,
            ratio,
            first,
            second,
        } => {
            let ratio = ratio.clamp(
                crate::model::split::RATIO_MIN,
                crate::model::split::RATIO_MAX,
            );
            Ok(SplitTree::Split {
                axis: *axis,
                ratio,
                first: Box::new(restore_tree_impl(first, panes, used)?),
                second: Box::new(restore_tree_impl(second, panes, used)?),
            })
        }
    }
}

impl SplitTreeStateV1 {
    /// Number of leaves, for tests and diagnostics.
    pub fn panes_count(&self) -> usize {
        match self {
            SplitTreeStateV1::Leaf(_) => 1,
            SplitTreeStateV1::Split { first, second, .. } => {
                first.panes_count() + second.panes_count()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::action::AppAction;
    use crate::model::geometry::PaddingPx;
    use crate::platform::PlatformCapabilities;
    use mr_crabs_element::{CellMetrics, PixelExtent};
    #[test]
    fn round_trip_preserves_windows_tabs_tree_and_focus() {
        let mut model = AppModel::headless();
        model.dispatch(AppAction::NewSplitRight);
        model.dispatch(AppAction::NewTab);
        model.dispatch(AppAction::NewSplitDown);
        model.dispatch(AppAction::GotoSplitLeft);

        let store = RestoreStore::new();
        let state = store.snapshot(&model);
        assert_eq!(state.version, SHELL_STATE_VERSION);
        assert_eq!(state.windows.len(), 1);
        assert_eq!(state.windows[0].tabs.len(), 2);

        let mut restored = AppModel::headless();
        let mut store = RestoreStore::new();
        store.apply(&mut restored, state).expect("apply");
        assert_eq!(restored.windows.len(), 1);
        let original = model.active_window().unwrap();
        let restored_window = restored.active_window().unwrap();
        assert_eq!(restored_window.tabs.len(), original.tabs.len());
        for (tab_id, original_tab) in &original.tabs {
            let restored_tab = restored_window.tabs.get(tab_id).expect("tab");
            assert_eq!(restored_tab.pane_count(), original_tab.pane_count());
            assert_eq!(restored_tab.tree, original_tab.tree);
            assert_eq!(
                restored_tab.focused_pane_id(),
                original_tab.focused_pane_id()
            );
        }
        assert_eq!(
            restored_window.active_tab, original.active_tab,
            "active tab restored"
        );
        assert_eq!(
            restored.active_window, model.active_window,
            "active window restored"
        );
        // New ids continue past restored ids.
        restored.dispatch(AppAction::NewSplitRight);
        let new_pane = restored.active_tab().unwrap().focused_pane_id().unwrap();
        assert!(
            new_pane.as_u64() > 2,
            "allocator continues past restored ids"
        );
    }
    #[test]
    fn restore_waits_for_geometry_before_spawn() {
        let model = AppModel::with_platform(PlatformCapabilities::macos());
        let state = model.restore_snapshot();
        let mut restored = AppModel::with_platform(PlatformCapabilities::macos());
        RestoreStore::new()
            .apply(&mut restored, state)
            .expect("restore");
        let pane = restored.focused_pane().expect("restored pane");
        assert_eq!(pane.lifecycle, crate::model::pane::PtyLifecycle::Pending);
        assert!(pane.session.child_pid().is_none());
        assert!(pane.frame().is_none());

        let geometry = crate::model::SurfaceGeometry::from_viewport(
            PixelExtent {
                width: 640.0,
                height: 384.0,
            },
            CellMetrics::new(8.0, 16.0).expect("metrics"),
            PaddingPx::default(),
        )
        .expect("geometry");
        let window_id = restored.active_window.expect("window");
        restored.commit_geometry(window_id, geometry);
        let pane = restored.focused_pane().expect("restored pane");
        assert_eq!(pane.lifecycle, crate::model::pane::PtyLifecycle::Live);
        assert!(pane.session.child_pid().is_some());
        assert!(pane.frame().is_some());
        restored.shutdown_all();
    }

    #[test]
    fn save_and_load_round_trip_through_disk() {
        let mut model = AppModel::headless();
        model.dispatch(AppAction::NewSplitRight);
        let dir = std::env::temp_dir().join(format!(
            "mr-crabs-restore-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let path = dir.join("shell-state.json");
        let mut store = RestoreStore::at(path.clone());
        store.save(&model).expect("save");
        assert_eq!(store.save_count, 1);
        assert!(store.last_saved.is_some());
        let loaded = store.load(&path).expect("load");
        assert_eq!(loaded.windows.len(), 1);
        assert_eq!(loaded.windows[0].tabs[0].tree.panes_count(), 2);
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn unsupported_versions_are_rejected() {
        let mut store = RestoreStore::new();
        let mut state = store.snapshot(&AppModel::headless());
        state.version = 2;
        let mut model = AppModel::headless();
        assert_eq!(
            store.apply(&mut model, state),
            Err(RestoreError::UnsupportedVersion(2))
        );
        // The model is untouched after a failed restore.
        assert_eq!(model.windows.len(), 1);
    }

    #[test]
    fn invalid_trees_are_rejected() {
        let mut store = RestoreStore::new();
        let mut state = store.snapshot(&AppModel::headless());
        // Point the tree at a pane that is not in the pane map.
        state.windows[0].tabs[0].tree = SplitTreeStateV1::Leaf(999);
        let mut model = AppModel::headless();
        assert!(store.apply(&mut model, state).is_err());
        // Duplicate leaves are rejected too.
        let mut state = store.snapshot(&AppModel::headless());
        state.windows[0].tabs[0].tree = SplitTreeStateV1::Split {
            axis: SplitAxis::Horizontal,
            ratio: 0.5,
            first: Box::new(SplitTreeStateV1::Leaf(1)),
            second: Box::new(SplitTreeStateV1::Leaf(1)),
        };
        assert!(store.apply(&mut model, state).is_err());
    }

    #[test]
    fn default_path_lives_under_home() {
        let path = RestoreStore::default_path().expect("home set in tests");
        assert!(path.ends_with("shell-state.json"));
    }
}

//! A window: an ordered set of tabs plus window-level focus state.

use std::collections::BTreeMap;
use std::time::Duration;

use mr_crabs_terminal::GridSize;

use mr_crabs_pty::OutputWake;

use super::geometry::SurfaceGeometry;
use super::pane::PaneId;
use super::split::SplitDirection;
use super::tab::{TabId, TabModel};

/// A window identity, unique per shell instance.
#[derive(
    Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct WindowId(pub u64);

impl WindowId {
    pub const fn new(id: u64) -> Self {
        Self(id)
    }

    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

/// Outcome of closing one tab.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TabCloseOutcome {
    /// The tab was removed and its sessions shut down.
    Closed(TabId),
    /// The tab was the window's only tab; the window must be closed.
    LastTabClosed,
}

/// Aggregate pump statistics for a window.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct WindowPumpStats {
    pub chunks: usize,
    pub bytes: usize,
    pub frames: usize,
    pub pending: bool,
}

impl WindowPumpStats {
    pub fn changed(self) -> bool {
        self.chunks > 0
    }
}

/// A window model.
pub struct WindowModel {
    pub id: WindowId,
    pub title: String,
    pub tabs: BTreeMap<TabId, TabModel>,
    /// Tab z-order: last is the newest; the active tab is usually last.
    pub tab_order: Vec<TabId>,
    pub active_tab: Option<TabId>,
    /// The measured pixel↔cell mapping of this window's surface, committed
    /// by the live view before every render. This is the single sizing
    /// authority: pane grids and PTY sizes derive from it via
    /// [`SurfaceGeometry::for_rect`] over each tab's split rectangles.
    /// `None` before the first measurement (window construction and the
    /// restore path).
    pub geometry: Option<SurfaceGeometry>,
    /// Whether this window is the quick-terminal popup.
    pub is_quick_terminal: bool,
    /// Whether the window is currently shown (the quick terminal hides
    /// without closing its sessions).
    pub visible: bool,
    /// Whether the window was restored from shell state.
    pub restored: bool,
}

impl WindowModel {
    /// A window with one tab containing one detached pane.
    ///
    /// `size` is the pane's initial pre-measure grid; the live view commits
    /// measured [`SurfaceGeometry`] before the first render, after which it
    /// becomes the sizing authority.
    pub fn new(
        id: WindowId,
        tab_id: TabId,
        pane_id: PaneId,
        size: GridSize,
    ) -> Result<Self, mr_crabs_terminal::TerminalError> {
        let tab = TabModel::new(tab_id, pane_id, size)?;
        Ok(Self {
            id,
            title: "Mr Crabs".to_string(),
            tabs: BTreeMap::from([(tab_id, tab)]),
            tab_order: vec![tab_id],
            active_tab: Some(tab_id),
            geometry: None,
            is_quick_terminal: false,
            visible: true,
            restored: false,
        })
    }

    /// A window from parts (restore path). The tab order must contain
    /// exactly the tabs map's keys. The restored window carries no measured
    /// geometry; the live view commits it before the first render.
    #[allow(clippy::too_many_arguments)] // restore payload fields map one-to-one
    pub fn from_parts(
        id: WindowId,
        title: String,
        tabs: BTreeMap<TabId, TabModel>,
        tab_order: Vec<TabId>,
        active_tab: Option<TabId>,
        is_quick_terminal: bool,
    ) -> Result<Self, WindowError> {
        let mut seen = tab_order.clone();
        seen.sort_unstable();
        let mut keys: Vec<TabId> = tabs.keys().copied().collect();
        keys.sort_unstable();
        if seen != keys {
            return Err(WindowError::InvalidState(
                "tab order must contain exactly the tab map's keys".to_string(),
            ));
        }
        if let Some(active) = active_tab
            && !tabs.contains_key(&active)
        {
            return Err(WindowError::InvalidState(
                "active tab not in window".to_string(),
            ));
        }
        let active_tab = active_tab.or_else(|| tab_order.last().copied());
        Ok(Self {
            id,
            title,
            tabs,
            tab_order,
            active_tab,
            geometry: None,
            is_quick_terminal,
            visible: true,
            restored: true,
        })
    }

    pub fn tab(&self, id: TabId) -> Option<&TabModel> {
        self.tabs.get(&id)
    }

    pub fn tab_mut(&mut self, id: TabId) -> Option<&mut TabModel> {
        self.tabs.get_mut(&id)
    }

    pub fn active_tab(&self) -> Option<&TabModel> {
        self.active_tab.and_then(|id| self.tabs.get(&id))
    }

    pub fn active_tab_mut(&mut self) -> Option<&mut TabModel> {
        self.active_tab.and_then(|id| self.tabs.get_mut(&id))
    }

    /// Append a tab and make it active.
    pub fn add_tab(&mut self, tab: TabModel) {
        self.tab_order.push(tab.id);
        self.tabs.insert(tab.id, tab);
        self.active_tab = self.tab_order.last().copied();
    }

    /// Focus a tab by id; returns whether it exists.
    pub fn set_active_tab(&mut self, tab_id: TabId) -> bool {
        if !self.tabs.contains_key(&tab_id) {
            return false;
        }
        self.active_tab = Some(tab_id);
        true
    }

    /// Cycle the active tab through tab order.
    pub fn cycle_tab(&mut self, forward: bool) -> Option<TabId> {
        let active = self.active_tab?;
        if self.tab_order.len() <= 1 {
            return Some(active);
        }
        let position = self.tab_order.iter().position(|id| *id == active)?;
        let delta = if forward { 1 } else { -1 };
        let next = (position as isize + delta).rem_euclid(self.tab_order.len() as isize) as usize;
        let next = self.tab_order[next];
        self.active_tab = Some(next);
        Some(next)
    }

    /// Close a tab: shut its sessions down deterministically, remove it, and
    /// move the active tab to the previous one. Returns
    /// [`TabCloseOutcome::LastTabClosed`] when it was the only tab.
    pub fn close_tab(&mut self, tab_id: TabId, grace: Duration) -> TabCloseOutcome {
        let Some(mut tab) = self.tabs.remove(&tab_id) else {
            return TabCloseOutcome::LastTabClosed;
        };
        tab.close_all(grace);
        self.tab_order.retain(|id| *id != tab_id);
        if self.tabs.is_empty() {
            self.active_tab = None;
            return TabCloseOutcome::LastTabClosed;
        }
        if self.active_tab == Some(tab_id) {
            self.active_tab = self.tab_order.last().copied();
        }
        TabCloseOutcome::Closed(tab_id)
    }

    /// The window's focused pane (active tab's focused pane).
    pub fn focused_pane_id(&self) -> Option<PaneId> {
        self.active_tab().and_then(|tab| tab.focused_pane_id())
    }

    pub fn focused_pane_mut(&mut self) -> Option<&mut super::pane::PaneModel> {
        let tab = self.active_tab_mut()?;
        let pane_id = tab.focused_pane_id()?;
        tab.panes.get_mut(&pane_id)
    }

    /// Focus a pane anywhere in the window; returns whether it exists.
    pub fn focus_pane(&mut self, pane_id: PaneId) -> bool {
        for tab in self.tabs.values_mut() {
            if tab.tree.contains(pane_id) {
                self.active_tab = Some(tab.id);
                return tab.focus_pane(pane_id);
            }
        }
        false
    }

    /// Pump every tab's bounded reader queues.
    pub fn pump(&mut self, cap: usize) -> WindowPumpStats {
        let mut stats = WindowPumpStats::default();
        for tab in self.tabs.values_mut() {
            let pumped = tab.pump(cap);
            stats.chunks += pumped.chunks;
            stats.bytes += pumped.bytes;
            stats.frames += pumped.frames;
            stats.pending |= pumped.pending;
        }
        stats
    }

    /// The committed grid, once the surface has been measured.
    pub fn grid(&self) -> Option<GridSize> {
        self.geometry.map(|geometry| geometry.grid)
    }
    pub fn set_geometry_with_output_wake(
        &mut self,
        geometry: SurfaceGeometry,
        output_wake: Option<OutputWake>,
    ) -> usize {
        let changed_geometry = self.geometry != Some(geometry);
        self.geometry = Some(geometry);
        let mut changed = 0;
        for tab in self.tabs.values_mut() {
            changed += tab.resize_all_with_output_wake(geometry, output_wake.clone());
        }
        if changed_geometry {
            changed.max(1)
        } else {
            changed
        }
    }

    /// Commit measured geometry using no PTY wake (headless/tests).
    pub fn set_geometry(&mut self, geometry: SurfaceGeometry) -> usize {
        self.set_geometry_with_output_wake(geometry, None)
    }

    /// Whether any pane has output queued.
    pub fn has_pending_output(&mut self) -> bool {
        self.tabs.values_mut().any(|tab| tab.has_pending_output())
    }

    /// Move focus in a direction in the active tab. Uses the committed
    /// grid; before the first measurement (headless operation) the focused
    /// pane's current grid stands in, since split rectangles are relative.
    pub fn goto_split(&mut self, direction: SplitDirection) -> Option<PaneId> {
        let size = self.grid().or_else(|| {
            self.active_tab()
                .and_then(|tab| tab.focused_pane())
                .map(|pane| pane.last_size)
        })?;
        self.active_tab_mut()
            .and_then(|tab| tab.goto_split(direction, size))
    }

    /// Cycle focus in the active tab.
    pub fn cycle_pane(&mut self, forward: bool) -> Option<PaneId> {
        self.active_tab_mut()
            .and_then(|tab| tab.cycle_pane(forward))
    }

    /// Human title: the active tab's title when set, else the window title.
    pub fn window_title(&self) -> String {
        self.active_tab()
            .filter(|tab| !tab.title.is_empty())
            .map(|tab| tab.title.clone())
            .unwrap_or_else(|| self.title.clone())
    }
}

/// Validation errors when assembling a window from parts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum WindowError {
    InvalidState(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::geometry::PaddingPx;
    use crate::model::pane::PaneId;
    use crate::model::split::SplitAxis;
    use mr_crabs_element::{CellMetrics, PixelExtent};

    const GRACE: Duration = Duration::from_millis(50);

    fn window() -> WindowModel {
        WindowModel::new(
            WindowId::new(1),
            TabId::new(1),
            PaneId::new(1),
            GridSize::new(80, 24),
        )
        .expect("window")
    }

    fn extra_tab(id: u64, pane_id: u64) -> TabModel {
        TabModel::new(TabId::new(id), PaneId::new(pane_id), GridSize::new(80, 24)).expect("tab")
    }

    fn geometry(width: f32, height: f32) -> SurfaceGeometry {
        SurfaceGeometry::from_viewport(
            PixelExtent { width, height },
            CellMetrics::new(8.0, 16.0).expect("metrics"),
            PaddingPx::default(),
        )
        .expect("geometry")
    }

    #[test]
    fn new_window_has_one_active_tab() {
        let window = window();
        assert_eq!(window.tabs.len(), 1);
        assert_eq!(window.tab_order, vec![TabId::new(1)]);
        assert!(window.active_tab().is_some());
        assert_eq!(window.focused_pane_id(), Some(PaneId::new(1)));
        assert_eq!(window.window_title(), "shell");
    }

    #[test]
    fn add_and_cycle_tabs() {
        let mut window = window();
        window.add_tab(extra_tab(2, 2));
        window.add_tab(extra_tab(3, 3));
        assert_eq!(window.tabs.len(), 3);
        assert_eq!(window.active_tab().unwrap().id, TabId::new(3));
        assert_eq!(
            window.cycle_tab(true).unwrap(),
            TabId::new(1),
            "wraps forward"
        );
        assert_eq!(
            window.cycle_tab(false).unwrap(),
            TabId::new(3),
            "wraps backward"
        );
        assert!(window.set_active_tab(TabId::new(2)));
        assert!(!window.set_active_tab(TabId::new(99)));
    }

    #[test]
    fn close_tab_moves_active_and_shuts_down_sessions() {
        let mut window = window();
        window.add_tab(extra_tab(2, 2));
        window.add_tab(extra_tab(3, 3));
        window.set_active_tab(TabId::new(2));
        let outcome = window.close_tab(TabId::new(2), GRACE);
        assert_eq!(outcome, TabCloseOutcome::Closed(TabId::new(2)));
        assert_eq!(window.tabs.len(), 2);
        assert_eq!(
            window.active_tab().unwrap().id,
            TabId::new(3),
            "active moves to the last remaining"
        );
        // All sessions of the closed tab are shut down.
        assert!(!window.tabs.contains_key(&TabId::new(2)));
    }

    #[test]
    fn closing_the_last_tab_reports_last_tab_closed() {
        let mut window = window();
        let outcome = window.close_tab(TabId::new(1), GRACE);
        assert_eq!(outcome, TabCloseOutcome::LastTabClosed);
        assert!(window.tabs.is_empty());
        assert!(window.active_tab.is_none());
    }

    #[test]
    fn focus_pane_activates_its_tab() {
        let mut window = window();
        window.add_tab(extra_tab(2, 2));
        assert!(window.focus_pane(PaneId::new(2)));
        assert_eq!(window.active_tab().unwrap().id, TabId::new(2));
        assert_eq!(window.focused_pane_id(), Some(PaneId::new(2)));
        assert!(!window.focus_pane(PaneId::new(99)));
    }

    #[test]
    fn set_geometry_updates_all_panes_and_dedupes() {
        let mut window = window();
        window.add_tab(extra_tab(2, 2));
        let geometry = geometry(960.0, 560.0);
        assert_eq!(geometry.grid, GridSize::new(120, 35));
        let changed = window.set_geometry(geometry);
        assert_eq!(changed, 2);
        assert_eq!(window.grid(), Some(GridSize::new(120, 35)));
        for tab in window.tabs.values() {
            for pane in tab.panes.values() {
                assert_eq!(pane.last_size, GridSize::new(120, 35));
            }
        }
        // Identical geometry is a no-op.
        assert_eq!(window.set_geometry(geometry), 0);
    }

    #[test]
    fn from_parts_validation() {
        let tabs = || {
            BTreeMap::from([
                (TabId::new(1), extra_tab(1, 1)),
                (TabId::new(2), extra_tab(2, 2)),
            ])
        };
        let ok = WindowModel::from_parts(
            WindowId::new(5),
            "t".into(),
            tabs(),
            vec![TabId::new(1), TabId::new(2)],
            Some(TabId::new(2)),
            false,
        );
        assert!(ok.is_ok());
        let ok = ok.unwrap();
        assert!(ok.restored);
        assert_eq!(
            ok.geometry, None,
            "restored windows measure on first render"
        );
        // Missing tab in order is rejected.
        assert!(
            WindowModel::from_parts(
                WindowId::new(5),
                "t".into(),
                tabs(),
                vec![TabId::new(1)],
                None,
                false,
            )
            .is_err()
        );
        // Active tab outside the map is rejected.
        assert!(
            WindowModel::from_parts(
                WindowId::new(5),
                "t".into(),
                tabs(),
                vec![TabId::new(1), TabId::new(2)],
                Some(TabId::new(9)),
                false,
            )
            .is_err()
        );
    }

    #[test]
    fn split_navigation_and_cycle_work_through_window() {
        let mut window = window();
        let pane = crate::model::pane::PaneModel::detached(PaneId::new(9), GridSize::new(80, 24))
            .expect("pane");
        let tab = window.active_tab_mut().unwrap();
        tab.insert_split_pane(SplitAxis::Horizontal, pane)
            .expect("split");
        // Navigation uses the committed grid (640x384 at 8x16 = 80x24).
        window.set_geometry(geometry(640.0, 384.0));
        assert_eq!(
            window.goto_split(SplitDirection::Left),
            Some(PaneId::new(1))
        );
        assert_eq!(window.cycle_pane(true), Some(PaneId::new(9)));
    }
}

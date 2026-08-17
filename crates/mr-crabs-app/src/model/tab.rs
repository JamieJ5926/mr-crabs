//! A tab: a split tree plus the panes it owns.
//!
//! Closing a pane shuts its session down deterministically; closing the last
//! pane reports [`ClosePaneOutcome::TabEmpty`] so the caller closes the tab.
//! `TabModel` also owns split insertion, focus navigation, directional
//! split resizing, per-tab pumping, and whole-tab shutdown.

use std::collections::BTreeMap;
use std::time::Duration;

use mr_crabs_pty::OutputWake;

use mr_crabs_terminal::GridSize;

use super::geometry::SurfaceGeometry;
use super::pane::{DrainStats, PaneModel};
use super::split::{PaneId, SplitAxis, SplitDirection, SplitTree};

/// A tab identity, unique per shell instance.
#[derive(
    Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Hash, serde::Serialize, serde::Deserialize,
)]
pub struct TabId(pub u64);

impl TabId {
    pub const fn new(id: u64) -> Self {
        Self(id)
    }

    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

/// Errors from split insertion.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SplitError {
    /// No pane is focused in this tab.
    NoFocusedPane,
    /// The focused pane is not in the tree (corrupt state).
    PaneNotFound,
    /// The given pane id already exists.
    DuplicatePane,
}

/// Outcome of closing one pane.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClosePaneOutcome {
    /// The pane was removed and its session shut down.
    Closed(PaneId),
    /// The pane was the tab's only pane; the tab must be closed instead.
    TabEmpty,
}

/// Aggregate pump statistics for a tab.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct TabPumpStats {
    pub chunks: usize,
    pub bytes: usize,
    pub frames: usize,
    pub pending: bool,
}

impl TabPumpStats {
    pub fn changed(self) -> bool {
        self.chunks > 0
    }
}

/// A tab model.
pub struct TabModel {
    pub id: TabId,
    pub title: String,
    pub panes: BTreeMap<PaneId, PaneModel>,
    pub tree: SplitTree,
    pub focused_pane: Option<PaneId>,
}

impl TabModel {
    /// A tab with one detached pane.
    pub fn new(
        id: TabId,
        pane_id: PaneId,
        size: GridSize,
    ) -> Result<Self, mr_crabs_terminal::TerminalError> {
        let pane = PaneModel::detached(pane_id, size)?;
        Ok(Self {
            id,
            title: "shell".to_string(),
            panes: BTreeMap::from([(pane.id, pane)]),
            tree: SplitTree::leaf(pane_id),
            focused_pane: Some(pane_id),
        })
    }

    /// Build a tab from parts, validating the split-tree invariants:
    /// the tree's leaves are exactly the pane map's keys, and the focused
    /// pane is one of them.
    pub fn from_parts(
        id: TabId,
        title: String,
        panes: BTreeMap<PaneId, PaneModel>,
        tree: SplitTree,
        focused_pane: Option<PaneId>,
    ) -> Result<Self, TabError> {
        if tree.is_empty() {
            return Err(TabError::InvalidState(
                "split tree must contain a pane".to_string(),
            ));
        }
        let leaves: BTreeMap<PaneId, ()> =
            tree.panes().into_iter().map(|pane| (pane, ())).collect();
        let keys: BTreeMap<PaneId, ()> = panes.keys().copied().map(|pane| (pane, ())).collect();
        if leaves != keys {
            return Err(TabError::InvalidState(
                "split tree leaves must match the pane map exactly once".to_string(),
            ));
        }
        if let Some(focused) = focused_pane
            && !tree.contains(focused)
        {
            return Err(TabError::InvalidState(format!(
                "focused pane {focused:?} not in tree"
            )));
        }
        let focused_pane = focused_pane.or_else(|| tree.panes().first().copied());
        Ok(Self {
            id,
            title,
            panes,
            tree,
            focused_pane,
        })
    }

    pub fn pane(&self, id: PaneId) -> Option<&PaneModel> {
        self.panes.get(&id)
    }

    pub fn pane_mut(&mut self, id: PaneId) -> Option<&mut PaneModel> {
        self.panes.get_mut(&id)
    }

    pub fn focused_pane(&self) -> Option<&PaneModel> {
        self.focused_pane.and_then(|id| self.panes.get(&id))
    }

    pub fn focused_pane_mut(&mut self) -> Option<&mut PaneModel> {
        self.focused_pane.and_then(|id| self.panes.get_mut(&id))
    }

    pub fn focused_pane_id(&self) -> Option<PaneId> {
        self.focused_pane
    }

    /// Focus a pane; returns whether it exists.
    pub fn focus_pane(&mut self, id: PaneId) -> bool {
        if !self.tree.contains(id) {
            return false;
        }
        if let Some(pane) = self.panes.get_mut(&id) {
            pane.focus_sequence += 1;
        }
        self.focused_pane = Some(id);
        true
    }

    pub fn pane_count(&self) -> usize {
        self.panes.len()
    }

    pub fn pane_ids(&self) -> Vec<PaneId> {
        self.tree.panes()
    }

    /// Insert an existing pane as a new split of the focused pane along
    /// `axis`; the new pane receives focus (Ghostty behavior).
    pub fn insert_split_pane(
        &mut self,
        axis: SplitAxis,
        pane: PaneModel,
    ) -> Result<PaneId, SplitError> {
        if self.panes.contains_key(&pane.id) {
            return Err(SplitError::DuplicatePane);
        }
        let Some(focused) = self.focused_pane else {
            return Err(SplitError::NoFocusedPane);
        };
        if !self.tree.split(focused, axis, pane.id) {
            return Err(SplitError::PaneNotFound);
        }
        let id = pane.id;
        self.panes.insert(id, pane);
        self.focus_pane(id);
        Ok(id)
    }

    /// Close a pane: shut its session down deterministically, remove it from
    /// the tree (collapsing the parent split), and move focus to the pane
    /// that replaced it. Returns [`ClosePaneOutcome::TabEmpty`] when it was
    /// the only pane (the caller closes the tab).
    pub fn close_pane(&mut self, pane: PaneId, grace: Duration) -> ClosePaneOutcome {
        if self.tree.len() <= 1 {
            if self.panes.contains_key(&pane) {
                if let Some(pane_model) = self.panes.get_mut(&pane) {
                    let _ = pane_model.session.shutdown(grace);
                }
                return ClosePaneOutcome::TabEmpty;
            }
            return ClosePaneOutcome::TabEmpty;
        }
        let Some(new_root) = self.tree.remove(pane) else {
            return ClosePaneOutcome::TabEmpty;
        };
        if let Some(pane_model) = self.panes.get_mut(&pane) {
            let _ = pane_model.session.shutdown(grace);
        }
        self.panes.remove(&pane);
        self.tree = new_root;
        if self.focused_pane == Some(pane) {
            self.focused_pane = self.tree.panes().first().copied();
        }
        ClosePaneOutcome::Closed(pane)
    }

    /// Deterministically shut down every pane session (tab close, window
    /// close, quit).
    pub fn close_all(&mut self, grace: Duration) {
        for pane in self.panes.values_mut() {
            let _ = pane.session.shutdown(grace);
        }
    }

    /// Move focus in a direction across the split tree.
    pub fn goto_split(&mut self, direction: SplitDirection, size: GridSize) -> Option<PaneId> {
        let from = self.focused_pane?;
        let target = self.tree.neighbor(from, direction, size)?;
        self.focus_pane(target);
        Some(target)
    }

    /// Cycle focus through the tree in depth-first order.
    pub fn cycle_pane(&mut self, forward: bool) -> Option<PaneId> {
        let from = self.focused_pane?;
        let target = self.tree.next(from, forward)?;
        self.focus_pane(target);
        Some(target)
    }

    /// Resize the split nearest to the focused pane along `direction`.
    pub fn resize_split(&mut self, direction: SplitDirection, delta: f32) -> bool {
        let Some(focused) = self.focused_pane else {
            return false;
        };
        self.tree.resize(focused, direction, delta)
    }
    pub fn resize_all_with_output_wake(
        &mut self,
        geometry: SurfaceGeometry,
        output_wake: Option<OutputWake>,
    ) -> usize {
        let rects = self.tree.rects(geometry.grid);
        let mut changed = 0;
        for (pane_id, rect) in rects {
            let pane_geometry = geometry.for_rect(rect);
            if let Some(pane) = self.panes.get_mut(&pane_id)
                && matches!(
                    pane.commit_geometry(pane_geometry, output_wake.clone()),
                    Ok(true)
                )
            {
                changed += 1;
            }
        }
        changed
    }

    pub fn resize_all(&mut self, geometry: SurfaceGeometry) -> usize {
        self.resize_all_with_output_wake(geometry, None)
    }

    /// Pump every pane's bounded reader queue; aggregates statistics.
    pub fn pump(&mut self, cap: usize) -> TabPumpStats {
        let mut stats = TabPumpStats::default();
        for pane in self.panes.values_mut() {
            let DrainStats {
                chunks,
                bytes,
                frames,
                pending,
            } = pane.pump(cap);
            stats.chunks += chunks;
            stats.bytes += bytes;
            stats.frames += frames;
            stats.pending |= pending;
        }
        stats
    }

    /// Whether any pane has output queued right now.
    pub fn has_pending_output(&mut self) -> bool {
        self.panes
            .values_mut()
            .any(|pane| pane.session.has_pending())
    }

    pub fn rects(&self, size: GridSize) -> BTreeMap<PaneId, super::split::GridRect> {
        self.tree.rects(size)
    }

    /// Refresh the tab title from the focused pane.
    pub fn update_title_from_focused_pane(&mut self) {
        if let Some(pane) = self.focused_pane() {
            self.title = pane.title.clone();
        }
    }
}

/// Validation errors when assembling a tab from parts.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum TabError {
    InvalidState(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::pane::PaneSession;

    const GRACE: Duration = Duration::from_millis(50);

    fn tab() -> TabModel {
        TabModel::new(TabId::new(1), PaneId::new(1), GridSize::new(80, 24)).expect("tab")
    }

    #[test]
    fn new_tab_has_one_focused_detached_pane() {
        let tab = tab();
        assert_eq!(tab.pane_count(), 1);
        assert!(tab.focused_pane_id().is_some());
        assert_eq!(tab.pane_ids().len(), 1);
        let pane = tab.focused_pane().expect("focused");
        assert!(pane.session.is_detached());
    }

    #[test]
    fn insert_split_pane_adds_and_focuses() {
        let mut tab = tab();
        let first = tab.focused_pane_id().unwrap();
        let pane = PaneModel::detached(PaneId::new(2), GridSize::new(80, 24)).expect("pane");
        let id = tab
            .insert_split_pane(SplitAxis::Horizontal, pane)
            .expect("split");
        assert_eq!(tab.pane_count(), 2);
        assert_eq!(tab.focused_pane_id(), Some(id), "new pane takes focus");
        assert_ne!(first, id);
        // Splitting again nests and still focuses the new pane.
        let pane = PaneModel::detached(PaneId::new(3), GridSize::new(80, 24)).expect("pane");
        let id = tab
            .insert_split_pane(SplitAxis::Vertical, pane)
            .expect("split");
        assert_eq!(tab.pane_count(), 3);
        assert_eq!(tab.focused_pane_id(), Some(id));
    }

    #[test]
    fn insert_split_pane_rejects_duplicates_and_missing_focus() {
        let mut tab = tab();
        let existing = tab.focused_pane_id().unwrap();
        let dup = PaneModel::detached(existing, GridSize::new(80, 24)).expect("pane");
        assert_eq!(
            tab.insert_split_pane(SplitAxis::Horizontal, dup),
            Err(SplitError::DuplicatePane)
        );
    }

    #[test]
    fn close_pane_shuts_down_session_and_collapses() {
        let mut tab = tab();
        let first = tab.focused_pane_id().unwrap();
        let second = tab
            .insert_split_pane(
                SplitAxis::Horizontal,
                PaneModel::detached(PaneId::new(2), GridSize::new(80, 24)).expect("pane"),
            )
            .expect("split");
        assert!(!tab.pane(second).unwrap().session.is_shut_down());
        let outcome = tab.close_pane(second, GRACE);
        assert_eq!(outcome, ClosePaneOutcome::Closed(second));
        assert_eq!(tab.pane_count(), 1);
        assert!(tab.pane(first).is_some());
        assert!(tab.pane(second).is_none());
        assert_eq!(
            tab.tree.leaf_id(),
            Some(first),
            "tree collapsed to the sibling"
        );
        // Focus moved to the surviving pane.
        assert_eq!(tab.focused_pane_id(), Some(first));
    }

    #[test]
    fn close_pane_middle_of_three_collapses_to_sibling_subtree() {
        let mut tab = tab();
        let first = tab.focused_pane_id().unwrap();
        tab.insert_split_pane(
            SplitAxis::Horizontal,
            PaneModel::detached(PaneId::new(2), GridSize::new(80, 24)).expect("pane"),
        )
        .expect("split");
        let third = tab
            .insert_split_pane(
                SplitAxis::Vertical,
                PaneModel::detached(PaneId::new(3), GridSize::new(80, 24)).expect("pane"),
            )
            .expect("split");
        // Focus is on the third pane; closing it leaves the other two.
        let outcome = tab.close_pane(third, GRACE);
        assert_eq!(outcome, ClosePaneOutcome::Closed(third));
        assert_eq!(tab.pane_ids(), vec![first, PaneId::new(2)]);
        assert_eq!(
            tab.focused_pane_id(),
            Some(first),
            "focus moves to the first remaining pane"
        );
    }

    #[test]
    fn closing_the_only_pane_reports_tab_empty() {
        let mut tab = tab();
        let only = tab.focused_pane_id().unwrap();
        assert_eq!(tab.close_pane(only, GRACE), ClosePaneOutcome::TabEmpty);
        assert_eq!(
            tab.pane_count(),
            1,
            "the tab still owns the pane; caller closes the tab"
        );
    }

    #[test]
    fn close_all_shuts_down_every_session() {
        let mut tab = tab();
        tab.insert_split_pane(
            SplitAxis::Horizontal,
            PaneModel::detached(PaneId::new(2), GridSize::new(80, 24)).expect("pane"),
        )
        .expect("split");
        tab.insert_split_pane(
            SplitAxis::Vertical,
            PaneModel::detached(PaneId::new(3), GridSize::new(80, 24)).expect("pane"),
        )
        .expect("split");
        tab.close_all(GRACE);
        for pane in tab.panes.values() {
            assert!(
                pane.session.is_shut_down(),
                "pane {} must be shut down",
                pane.id.as_u64()
            );
        }
    }

    #[test]
    fn goto_split_and_cycle_pane_move_focus() {
        let mut tab = tab();
        let first = tab.focused_pane_id().unwrap();
        let second = tab
            .insert_split_pane(
                SplitAxis::Horizontal,
                PaneModel::detached(PaneId::new(2), GridSize::new(80, 24)).expect("pane"),
            )
            .expect("split");
        let size = GridSize::new(80, 24);
        // From the second (right) pane, left moves to the first.
        assert_eq!(tab.goto_split(SplitDirection::Left, size), Some(first));
        // From the first pane, right moves to the second.
        assert_eq!(tab.goto_split(SplitDirection::Right, size), Some(second));
        // Vertical directions have no target in a horizontal split.
        assert_eq!(tab.goto_split(SplitDirection::Up, size), None);
        // Cyclic focus order wraps.
        assert_eq!(tab.cycle_pane(true), Some(first));
        assert_eq!(tab.cycle_pane(true), Some(second));
    }

    #[test]
    fn resize_split_adjusts_ratios() {
        let mut tab = tab();
        tab.insert_split_pane(
            SplitAxis::Horizontal,
            PaneModel::detached(PaneId::new(2), GridSize::new(80, 24)).expect("pane"),
        )
        .expect("split");
        let rects_before = tab.rects(GridSize::new(80, 24));
        let second = PaneId::new(2);
        assert!(tab.resize_split(SplitDirection::Left, 0.3));
        let rects_after = tab.rects(GridSize::new(80, 24));
        assert!(rects_after[&second].width > rects_before[&second].width);
    }

    #[test]
    fn resize_all_updates_every_pane() {
        let mut tab = tab();
        tab.insert_split_pane(
            SplitAxis::Horizontal,
            PaneModel::detached(PaneId::new(2), GridSize::new(80, 24)).expect("pane"),
        )
        .expect("split");
        let geometry = SurfaceGeometry::from_viewport(
            mr_crabs_element::PixelExtent {
                width: 960.0,
                height: 640.0,
            },
            mr_crabs_element::CellMetrics::new(8.0, 16.0).expect("metrics"),
            crate::model::geometry::PaddingPx::default(),
        )
        .expect("geometry");
        let changed = tab.resize_all(geometry);
        assert_eq!(changed, 2);
        // Each pane receives its split-derived rect grid (60x40 at ratio
        // 0.5), not the raw window grid.
        for pane in tab.panes.values() {
            assert_eq!(pane.last_size, GridSize::new(60, 40));
            assert_eq!(pane.frame().expect("frame").size, GridSize::new(60, 40));
        }
    }

    #[test]
    fn from_parts_validates_tree_pane_equality() {
        let panes = || {
            BTreeMap::from([
                (
                    PaneId::new(1),
                    PaneModel::detached(PaneId::new(1), GridSize::new(80, 24)).expect("pane"),
                ),
                (
                    PaneId::new(2),
                    PaneModel::detached(PaneId::new(2), GridSize::new(80, 24)).expect("pane"),
                ),
            ])
        };
        let tree = {
            let mut t = SplitTree::leaf(PaneId::new(1));
            t.split(PaneId::new(1), SplitAxis::Horizontal, PaneId::new(2));
            t
        };
        let tab = TabModel::from_parts(
            TabId::new(9),
            "t".into(),
            panes(),
            tree,
            Some(PaneId::new(2)),
        );
        assert!(tab.is_ok());
        // A tree leaf that is not in the pane map is rejected.
        let bad_tree = SplitTree::leaf(PaneId::new(99));
        assert!(TabModel::from_parts(TabId::new(9), "t".into(), panes(), bad_tree, None).is_err());
        // A focused pane outside the tree is rejected.
        let good_tree = {
            let mut t = SplitTree::leaf(PaneId::new(1));
            t.split(PaneId::new(1), SplitAxis::Horizontal, PaneId::new(2));
            t
        };
        assert!(
            TabModel::from_parts(
                TabId::new(9),
                "t".into(),
                panes(),
                good_tree,
                Some(PaneId::new(99))
            )
            .is_err()
        );
    }

    #[test]
    fn pump_aggregates_and_reports_pending() {
        let mut tab = tab();
        let (tx, rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(8);
        tx.send(b"x".to_vec()).expect("send");
        tab.panes
            .get_mut(&tab.focused_pane_id().unwrap())
            .unwrap()
            .session = PaneSession::from_receivers(GridSize::new(80, 24), Some(rx), None);
        let stats = tab.pump(8);
        assert_eq!(stats.chunks, 1);
        assert!(stats.changed());
        assert!(!stats.pending);
    }
}

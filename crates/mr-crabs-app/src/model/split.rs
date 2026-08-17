//! Recursive split tree with deterministic geometry, focus navigation, and
//! ratio resizing.
//!
//! A tab's panes form a binary tree: leaves are panes, interior nodes are
//! splits along an axis with a ratio and two subtrees. All operations are
//! pure functions of the tree plus a grid size, so split/close/resize/
//! navigation invariants are fully headless-testable.

use std::collections::BTreeMap;

use mr_crabs_terminal::GridSize;
use serde::{Deserialize, Serialize};

/// Split orientation.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SplitAxis {
    /// A vertical divider: `first` is on the left, `second` on the right.
    Horizontal,
    /// A horizontal divider: `first` is on top, `second` below.
    Vertical,
}

impl SplitAxis {
    pub fn perpendicular(self) -> Self {
        match self {
            SplitAxis::Horizontal => SplitAxis::Vertical,
            SplitAxis::Vertical => SplitAxis::Horizontal,
        }
    }
}

/// A direction a pane can be reached or resized from.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SplitDirection {
    Up,
    Down,
    Left,
    Right,
}

impl SplitDirection {
    pub fn axis(self) -> SplitAxis {
        match self {
            SplitDirection::Up | SplitDirection::Down => SplitAxis::Vertical,
            SplitDirection::Left | SplitDirection::Right => SplitAxis::Horizontal,
        }
    }
}

/// A rectangle in grid cells.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct GridRect {
    pub x: u16,
    pub y: u16,
    pub width: u16,
    pub height: u16,
}

impl GridRect {
    pub fn right(self) -> u16 {
        self.x.saturating_add(self.width)
    }

    pub fn bottom(self) -> u16 {
        self.y.saturating_add(self.height)
    }

    fn overlap_with(self, other: GridRect, axis: SplitAxis) -> u16 {
        match axis {
            SplitAxis::Horizontal => {
                let start = self.x.max(other.x);
                let end = self.right().min(other.right());
                end.saturating_sub(start)
            }
            SplitAxis::Vertical => {
                let start = self.y.max(other.y);
                let end = self.bottom().min(other.bottom());
                end.saturating_sub(start)
            }
        }
    }

    fn gap_to(self, other: GridRect, direction: SplitDirection) -> u32 {
        match direction {
            SplitDirection::Up => u32::from(self.y.saturating_sub(other.bottom())),
            SplitDirection::Down => u32::from(other.y.saturating_sub(self.bottom())),
            SplitDirection::Left => u32::from(self.x.saturating_sub(other.right())),
            SplitDirection::Right => u32::from(other.x.saturating_sub(self.right())),
        }
    }
}

/// The recursive split tree.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SplitTree {
    Leaf(PaneId),
    Split {
        axis: SplitAxis,
        /// Fraction of the axis taken by `first`; clamped to `[0.1, 0.9]`.
        ratio: f32,
        first: Box<SplitTree>,
        second: Box<SplitTree>,
    },
}

/// A pane identity, unique per shell instance.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Hash, Serialize, Deserialize)]
pub struct PaneId(pub u64);

impl PaneId {
    pub const fn new(id: u64) -> Self {
        Self(id)
    }

    pub const fn as_u64(self) -> u64 {
        self.0
    }
}

/// Minimum/maximum split ratio, matching terminal split ergonomics.
pub const RATIO_MIN: f32 = 0.1;
pub const RATIO_MAX: f32 = 0.9;

impl SplitTree {
    pub fn leaf(pane: PaneId) -> Self {
        Self::Leaf(pane)
    }

    /// The single leaf when this tree is a leaf.
    pub fn leaf_id(&self) -> Option<PaneId> {
        match self {
            SplitTree::Leaf(pane) => Some(*pane),
            SplitTree::Split { .. } => None,
        }
    }

    /// Number of panes (leaves).
    pub fn len(&self) -> usize {
        match self {
            SplitTree::Leaf(_) => 1,
            SplitTree::Split { first, second, .. } => first.len() + second.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        false
    }

    pub fn contains(&self, pane: PaneId) -> bool {
        match self {
            SplitTree::Leaf(existing) => *existing == pane,
            SplitTree::Split { first, second, .. } => first.contains(pane) || second.contains(pane),
        }
    }

    /// All pane ids in deterministic depth-first order: first subtree, then
    /// second (left-to-right, top-to-bottom).
    pub fn panes(&self) -> Vec<PaneId> {
        let mut out = Vec::with_capacity(self.len());
        self.collect_panes(&mut out);
        out
    }

    fn collect_panes(&self, out: &mut Vec<PaneId>) {
        match self {
            SplitTree::Leaf(pane) => out.push(*pane),
            SplitTree::Split { first, second, .. } => {
                first.collect_panes(out);
                second.collect_panes(out);
            }
        }
    }

    /// Split the leaf `at` into two leaves along `axis`, inserting `new` as
    /// the second child. Returns whether the pane was found.
    pub fn split(&mut self, at: PaneId, axis: SplitAxis, new: PaneId) -> bool {
        if !self.contains(at) {
            return false;
        }
        self.split_impl(at, axis, new)
    }

    fn split_impl(&mut self, at: PaneId, axis: SplitAxis, new: PaneId) -> bool {
        match self {
            SplitTree::Leaf(existing) if *existing == at => {
                let old = SplitTree::Leaf(*existing);
                *self = SplitTree::Split {
                    axis,
                    ratio: 0.5,
                    first: Box::new(old),
                    second: Box::new(SplitTree::Leaf(new)),
                };
                true
            }
            SplitTree::Leaf(_) => false,
            SplitTree::Split { first, second, .. } => {
                if first.contains(at) {
                    first.split_impl(at, axis, new)
                } else if second.contains(at) {
                    second.split_impl(at, axis, new)
                } else {
                    false
                }
            }
        }
    }

    /// Remove a leaf, collapsing its parent split into the sibling subtree.
    /// Returns the new root, or `None` when the pane was not present or was
    /// the sole leaf (the caller must close the tab instead).
    pub fn remove(&self, pane: PaneId) -> Option<SplitTree> {
        if !self.contains(pane) || matches!(self, SplitTree::Leaf(_)) {
            return None;
        }
        let (next, _) = remove_rec(self.clone(), pane);
        next
    }

    /// Resize the split that is the nearest ancestor of `pane` with the
    /// direction's axis. `delta` is the fraction to move the divider;
    /// positive moves it in the direction's favor. Returns whether any
    /// divider moved.
    pub fn resize(&mut self, pane: PaneId, direction: SplitDirection, delta: f32) -> bool {
        self.resize_impl(pane, direction, delta)
    }

    fn resize_impl(&mut self, pane: PaneId, direction: SplitDirection, delta: f32) -> bool {
        match self {
            SplitTree::Leaf(_) => false,
            SplitTree::Split {
                axis,
                ratio,
                first,
                second,
            } => {
                let own_axis = *axis == direction.axis();
                let in_first = first.contains(pane);
                let in_second = second.contains(pane);
                if own_axis && (in_first || in_second) {
                    let adjustment = if in_first { delta } else { -delta };
                    let next = (*ratio + adjustment).clamp(RATIO_MIN, RATIO_MAX);
                    if (next - *ratio).abs() > f32::EPSILON {
                        *ratio = next;
                        true
                    } else {
                        false
                    }
                } else if in_first {
                    first.resize_impl(pane, direction, delta)
                } else if in_second {
                    second.resize_impl(pane, direction, delta)
                } else {
                    false
                }
            }
        }
    }

    /// Rectangle of every pane for a given grid size. Dividers split cells
    /// deterministically: `first` gets `round(width * ratio)` clamped to
    /// leave at least one cell for `second`.
    pub fn rects(&self, size: GridSize) -> BTreeMap<PaneId, GridRect> {
        let mut out = BTreeMap::new();
        let bounds = GridRect {
            x: 0,
            y: 0,
            width: size.cols,
            height: size.rows,
        };
        self.collect_rects(bounds, &mut out);
        out
    }

    fn collect_rects(&self, bounds: GridRect, out: &mut BTreeMap<PaneId, GridRect>) {
        match self {
            SplitTree::Leaf(pane) => {
                out.insert(*pane, bounds);
            }
            SplitTree::Split {
                axis,
                ratio,
                first,
                second,
            } => match axis {
                SplitAxis::Horizontal => {
                    let first_width = (f32::from(bounds.width) * ratio)
                        .round()
                        .clamp(1.0, f32::from(bounds.width.saturating_sub(1)))
                        as u16;
                    let first_bounds = GridRect {
                        x: bounds.x,
                        y: bounds.y,
                        width: first_width,
                        height: bounds.height,
                    };
                    let second_bounds = GridRect {
                        x: bounds.x + first_width,
                        y: bounds.y,
                        width: bounds.width - first_width,
                        height: bounds.height,
                    };
                    first.collect_rects(first_bounds, out);
                    second.collect_rects(second_bounds, out);
                }
                SplitAxis::Vertical => {
                    let first_height = (f32::from(bounds.height) * ratio)
                        .round()
                        .clamp(1.0, f32::from(bounds.height.saturating_sub(1)))
                        as u16;
                    let first_bounds = GridRect {
                        x: bounds.x,
                        y: bounds.y,
                        width: bounds.width,
                        height: first_height,
                    };
                    let second_bounds = GridRect {
                        x: bounds.x,
                        y: bounds.y + first_height,
                        width: bounds.width,
                        height: bounds.height - first_height,
                    };
                    first.collect_rects(first_bounds, out);
                    second.collect_rects(second_bounds, out);
                }
            },
        }
    }

    /// Rectangle of one pane for a given grid size.
    pub fn bounds_of(&self, pane: PaneId, size: GridSize) -> Option<GridRect> {
        self.rects(size).get(&pane).copied()
    }

    /// The pane reached by moving in `direction` from `from`: among panes
    /// strictly in that direction, prefer the one with the greatest
    /// perpendicular overlap, then the smallest gap, then pane order.
    pub fn neighbor(
        &self,
        from: PaneId,
        direction: SplitDirection,
        size: GridSize,
    ) -> Option<PaneId> {
        let rects = self.rects(size);
        let from_rect = rects.get(&from)?;
        let mut best: Option<(PaneId, u16, u32)> = None;
        for (candidate, rect) in &rects {
            if *candidate == from || !rect_in_direction(from_rect, rect, direction) {
                continue;
            }
            let overlap = from_rect.overlap_with(*rect, direction.axis().perpendicular());
            let gap = from_rect.gap_to(*rect, direction);
            let candidate_score = (*candidate, overlap, gap);
            let better = match best {
                None => true,
                Some((best_id, best_overlap, best_gap)) => {
                    overlap > best_overlap
                        || (overlap == best_overlap && gap < best_gap)
                        || (overlap == best_overlap
                            && gap == best_gap
                            && candidate_score.0 < best_id)
                }
            };
            if better {
                best = Some(candidate_score);
            }
        }
        best.map(|(id, _, _)| id)
    }

    /// Depth-first focus order; `next`/`previous` cycle through it.
    pub fn focus_order(&self) -> Vec<PaneId> {
        self.panes()
    }

    /// The next pane in focus order after `from` (cyclic), or `None` when
    /// the pane is not in the tree.
    pub fn next(&self, from: PaneId, forward: bool) -> Option<PaneId> {
        let order = self.panes();
        let position = order.iter().position(|pane| *pane == from)?;
        let delta = if forward { 1 } else { -1 };
        let next = (position as isize + delta).rem_euclid(order.len() as isize) as usize;
        Some(order[next])
    }
}

/// Returns `(new_subtree, removed)`; `new_subtree` is `None` when the whole
/// subtree was removed.
fn remove_rec(node: SplitTree, pane: PaneId) -> (Option<SplitTree>, bool) {
    match node {
        SplitTree::Leaf(existing) => {
            if existing == pane {
                (None, true)
            } else {
                (Some(SplitTree::Leaf(existing)), false)
            }
        }
        SplitTree::Split {
            axis,
            ratio,
            first,
            second,
        } => {
            let (first, removed) = remove_rec(*first, pane);
            if removed {
                match first {
                    None => (Some(*second), true),
                    Some(first) => (
                        Some(SplitTree::Split {
                            axis,
                            ratio,
                            first: Box::new(first),
                            second,
                        }),
                        true,
                    ),
                }
            } else {
                let (second, removed) = remove_rec(*second, pane);
                if removed {
                    match second {
                        None => (Some(first.expect("first survived")), true),
                        Some(second) => (
                            Some(SplitTree::Split {
                                axis,
                                ratio,
                                first: Box::new(first.expect("first survived")),
                                second: Box::new(second),
                            }),
                            true,
                        ),
                    }
                } else {
                    (
                        Some(SplitTree::Split {
                            axis,
                            ratio,
                            first: Box::new(first.expect("first survived")),
                            second: Box::new(second.expect("second survived")),
                        }),
                        false,
                    )
                }
            }
        }
    }
}

fn rect_in_direction(from: &GridRect, candidate: &GridRect, direction: SplitDirection) -> bool {
    match direction {
        SplitDirection::Up => candidate.bottom() <= from.y,
        SplitDirection::Down => candidate.y >= from.bottom(),
        SplitDirection::Left => candidate.right() <= from.x,
        SplitDirection::Right => candidate.x >= from.right(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(id: u64) -> PaneId {
        PaneId::new(id)
    }

    fn grid() -> GridSize {
        GridSize::new(80, 24)
    }

    #[test]
    fn leaf_and_split_basics() {
        let mut tree = SplitTree::leaf(p(1));
        assert_eq!(tree.len(), 1);
        assert!(tree.contains(p(1)));
        assert_eq!(tree.panes(), vec![p(1)]);
        assert!(!tree.contains(p(2)));

        assert!(tree.split(p(1), SplitAxis::Horizontal, p(2)));
        assert_eq!(tree.len(), 2);
        assert_eq!(tree.panes(), vec![p(1), p(2)]);
        // Splitting a missing pane is a no-op.
        assert!(!tree.split(p(9), SplitAxis::Horizontal, p(3)));
        assert_eq!(tree.len(), 2);
    }

    #[test]
    fn nested_splits_preserve_their_own_axes() {
        let mut tree = SplitTree::leaf(p(1));
        tree.split(p(1), SplitAxis::Horizontal, p(2));
        // Splitting the second pane nests a vertical split inside.
        assert!(tree.split(p(2), SplitAxis::Vertical, p(3)));
        assert_eq!(tree.len(), 3);
        assert_eq!(tree.panes(), vec![p(1), p(2), p(3)]);
    }

    #[test]
    fn rects_split_horizontally_and_vertically() {
        let mut tree = SplitTree::leaf(p(1));
        tree.split(p(1), SplitAxis::Horizontal, p(2));
        let rects = tree.rects(grid());
        assert_eq!(
            rects[&p(1)],
            GridRect {
                x: 0,
                y: 0,
                width: 40,
                height: 24
            }
        );
        assert_eq!(
            rects[&p(2)],
            GridRect {
                x: 40,
                y: 0,
                width: 40,
                height: 24
            }
        );

        tree.split(p(1), SplitAxis::Vertical, p(3));
        let rects = tree.rects(grid());
        assert_eq!(
            rects[&p(3)],
            GridRect {
                x: 0,
                y: 12,
                width: 40,
                height: 12
            }
        );
        assert_eq!(
            rects[&p(1)],
            GridRect {
                x: 0,
                y: 0,
                width: 40,
                height: 12
            }
        );
        assert_eq!(
            rects[&p(2)],
            GridRect {
                x: 40,
                y: 0,
                width: 40,
                height: 24
            }
        );
        assert_eq!(tree.bounds_of(p(2), grid()), Some(rects[&p(2)]));
    }

    #[test]
    fn removing_a_leaf_collapses_the_parent_split() {
        let mut tree = SplitTree::leaf(p(1));
        tree.split(p(1), SplitAxis::Horizontal, p(2));
        let next = tree.remove(p(1)).expect("removed");
        assert_eq!(next, SplitTree::leaf(p(2)));
        // Removing the sole leaf is refused: the caller closes the tab.
        assert_eq!(next.remove(p(2)), None);
        // Removing a missing pane is refused.
        assert_eq!(tree.remove(p(9)), None);
    }

    #[test]
    fn removing_middle_leaf_keeps_sibling_subtree() {
        let mut tree = SplitTree::leaf(p(1));
        tree.split(p(1), SplitAxis::Horizontal, p(2));
        tree.split(p(2), SplitAxis::Vertical, p(3));
        // Tree: 1 | (2 / 3)
        let next = tree.remove(p(2)).expect("removed");
        assert_eq!(next.panes(), vec![p(1), p(3)]);
        assert_eq!(next.len(), 2);
    }

    #[test]
    fn resize_adjusts_the_nearest_matching_ancestor_and_clamps() {
        let mut tree = SplitTree::leaf(p(1));
        tree.split(p(1), SplitAxis::Horizontal, p(2));
        // Right direction grows the second (right) child: ratio shrinks.
        assert!(tree.resize(p(2), SplitDirection::Right, 0.2));
        let rects = tree.rects(grid());
        assert!(rects[&p(2)].width > rects[&p(1)].width);
        // Clamp at the extremes.
        assert!(tree.resize(p(1), SplitDirection::Left, 10.0));
        let rects = tree.rects(grid());
        assert_eq!(rects[&p(1)].width, 72, "ratio clamped to 0.9 of 80");
        // Wrong-axis directions are refused (no matching ancestor).
        assert!(!tree.resize(p(1), SplitDirection::Up, 0.2));
    }

    #[test]
    fn directional_neighbor_prefers_overlap_then_gap() {
        // 2x2 grid of panes: 1 | 2 on top, 3 | 4 below.
        let mut tree = SplitTree::leaf(p(1));
        tree.split(p(1), SplitAxis::Horizontal, p(2));
        tree.split(p(1), SplitAxis::Vertical, p(3));
        tree.split(p(2), SplitAxis::Vertical, p(4));
        let size = GridSize::new(80, 24);
        assert_eq!(tree.neighbor(p(1), SplitDirection::Down, size), Some(p(3)));
        assert_eq!(tree.neighbor(p(2), SplitDirection::Down, size), Some(p(4)));
        assert_eq!(tree.neighbor(p(3), SplitDirection::Up, size), Some(p(1)));
        assert_eq!(tree.neighbor(p(1), SplitDirection::Right, size), Some(p(2)));
        assert_eq!(tree.neighbor(p(2), SplitDirection::Left, size), Some(p(1)));
        assert_eq!(tree.neighbor(p(4), SplitDirection::Right, size), None);
        assert_eq!(tree.neighbor(p(1), SplitDirection::Up, size), None);
    }

    #[test]
    fn focus_order_cycles() {
        let mut tree = SplitTree::leaf(p(1));
        tree.split(p(1), SplitAxis::Horizontal, p(2));
        tree.split(p(2), SplitAxis::Vertical, p(3));
        assert_eq!(tree.next(p(1), true), Some(p(2)));
        assert_eq!(tree.next(p(3), true), Some(p(1)), "wraps forward");
        assert_eq!(tree.next(p(1), false), Some(p(3)), "wraps backward");
        assert_eq!(tree.next(p(9), true), None);
    }

    #[test]
    fn neighbor_requires_strict_direction() {
        // 1 | 2: moving down from 1 must not wrap to 2.
        let mut tree = SplitTree::leaf(p(1));
        tree.split(p(1), SplitAxis::Horizontal, p(2));
        let size = GridSize::new(80, 24);
        assert_eq!(tree.neighbor(p(1), SplitDirection::Down, size), None);
        assert_eq!(tree.neighbor(p(1), SplitDirection::Up, size), None);
        assert_eq!(tree.neighbor(p(2), SplitDirection::Right, size), None);
    }
}

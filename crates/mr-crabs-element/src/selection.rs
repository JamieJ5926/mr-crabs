//! Selection overlay geometry.
//!
//! A selection is an inclusive-start, exclusive-end cell span in row-major
//! order: it covers cells `(row, col)` with `start <= (row, col) < end`.
//! Rectangles are emitted per row — partial rows at the span edges, full
//! rows in the middle — clipped to the grid.

use gpui::{Bounds, Pixels, point, px, size};
use mr_crabs_terminal::{GridSize, SelectionState};

use crate::CellMetrics;

/// Per-row selection rectangles in pixels at grid origin, for an active
/// selection. Returns an empty vec when the selection is inactive, has no
/// anchors, has degenerate extent, or the metrics are invalid.
///
/// The end anchor is clamped to the grid (spans past the right or bottom
/// edge are truncated); a start anchor at or past the right edge advances
/// to its row-major position instead of emitting phantom full rows.
pub fn selection_rects(
    sel: &SelectionState,
    grid: GridSize,
    metrics: CellMetrics,
) -> Vec<Bounds<Pixels>> {
    if !sel.active {
        return Vec::new();
    }
    let (Some(start), Some(end)) = (sel.start, sel.end) else {
        return Vec::new();
    };
    if metrics.width <= 0.0
        || metrics.height <= 0.0
        || !metrics.width.is_finite()
        || !metrics.height.is_finite()
    {
        return Vec::new();
    }
    let (start, end) = if start <= end {
        (start, end)
    } else {
        (end, start)
    };
    if start == end || start.0 >= grid.rows {
        return Vec::new();
    }

    let mut rects = Vec::new();
    let cols = usize::from(grid.cols);
    let rows = usize::from(grid.rows);
    if cols == 0 || rows == 0 {
        return Vec::new();
    }
    let mut row = usize::from(start.0);
    let mut col = usize::from(start.1);
    // A start column at or past the right edge advances into following rows
    // (its row-major position), so a past-the-edge anchor never emits
    // phantom full rows.
    row += col / cols;
    col %= cols;
    let end_row = usize::from(end.0.min(grid.rows));
    // Clamp the end column to the grid width so a span past the right edge
    // is truncated rather than emitted beyond the grid.
    let end_col = usize::from(end.1.min(grid.cols));

    while row < rows && (row, col) < (end_row, end_col) {
        let row_end = if row == end_row { end_col } else { cols };
        let span = row_end.saturating_sub(col);
        if span > 0 {
            rects.push(Bounds::new(
                point(
                    px(f32::from(col as u16) * metrics.width),
                    px(f32::from(row as u16) * metrics.height),
                ),
                size(
                    px(f32::from(span as u16) * metrics.width),
                    px(metrics.height),
                ),
            ));
        }
        row += 1;
        col = 0;
    }
    rects
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::px;
    use mr_crabs_terminal::SelectionState;

    const SIZE: GridSize = GridSize::new(10, 5);
    const METRICS: CellMetrics = CellMetrics {
        width: 10.0,
        height: 20.0,
    };

    fn sel(start: Option<(u16, u16)>, end: Option<(u16, u16)>, active: bool) -> SelectionState {
        SelectionState {
            start,
            end,
            active,
            kind: mr_crabs_terminal::SelectionKind::Linear,
        }
    }

    fn rect(row: u16, col: u16, len: u16) -> Bounds<Pixels> {
        Bounds::new(
            point(px(f32::from(col) * 10.0), px(f32::from(row) * 20.0)),
            size(px(f32::from(len) * 10.0), px(20.0)),
        )
    }

    #[test]
    fn inactive_or_unanchored_is_empty() {
        assert!(selection_rects(&sel(None, None, true), SIZE, METRICS).is_empty());
        assert!(selection_rects(&sel(Some((0, 0)), None, true), SIZE, METRICS).is_empty());
        assert!(selection_rects(&sel(None, Some((1, 1)), true), SIZE, METRICS).is_empty());
        assert!(selection_rects(&sel(Some((0, 0)), Some((1, 1)), false), SIZE, METRICS).is_empty());
    }

    #[test]
    fn invalid_metrics_is_empty() {
        let s = sel(Some((0, 0)), Some((2, 2)), true);
        assert!(
            selection_rects(
                &s,
                SIZE,
                CellMetrics {
                    width: 0.0,
                    height: 20.0
                }
            )
            .is_empty()
        );
        assert!(
            selection_rects(
                &s,
                SIZE,
                CellMetrics {
                    width: f32::NAN,
                    height: 20.0
                }
            )
            .is_empty()
        );
        assert!(
            selection_rects(
                &s,
                SIZE,
                CellMetrics {
                    width: 10.0,
                    height: f32::INFINITY
                }
            )
            .is_empty()
        );
    }

    #[test]
    fn single_row_partial_span() {
        assert_eq!(
            selection_rects(&sel(Some((2, 3)), Some((2, 7)), true), SIZE, METRICS),
            vec![rect(2, 3, 4)]
        );
        // Reversed anchors normalize to the same span.
        assert_eq!(
            selection_rects(&sel(Some((2, 7)), Some((2, 3)), true), SIZE, METRICS),
            vec![rect(2, 3, 4)]
        );
    }

    #[test]
    fn multi_row_produces_partial_edges_and_full_middle() {
        assert_eq!(
            selection_rects(&sel(Some((1, 8)), Some((3, 4)), true), SIZE, METRICS),
            vec![
                rect(1, 8, 2),  // row 1: cols 8..10
                rect(2, 0, 10), // row 2: full
                rect(3, 0, 4),  // row 3: cols 0..4
            ]
        );
    }

    #[test]
    fn span_at_grid_edge_is_clipped() {
        // End column beyond the grid width.
        assert_eq!(
            selection_rects(&sel(Some((0, 8)), Some((0, 20)), true), SIZE, METRICS),
            vec![rect(0, 8, 2)]
        );
        // End row beyond the grid height: clipped to the last row.
        assert_eq!(
            selection_rects(&sel(Some((4, 0)), Some((9, 0)), true), SIZE, METRICS),
            vec![rect(4, 0, 10)]
        );
    }

    #[test]
    fn degenerate_span_is_empty() {
        assert!(selection_rects(&sel(Some((1, 1)), Some((1, 1)), true), SIZE, METRICS).is_empty());
        // Start entirely below the grid.
        assert!(selection_rects(&sel(Some((5, 0)), Some((6, 0)), true), SIZE, METRICS).is_empty());
    }

    #[test]
    fn start_column_past_right_edge_advances_rows() {
        // Row-major position of (0, 12) on a 10-col grid is (1, 2): the span
        // must not emit a phantom full row at the anchor's nominal row.
        assert_eq!(
            selection_rects(&sel(Some((0, 12)), Some((2, 1)), true), SIZE, METRICS),
            vec![rect(1, 2, 8), rect(2, 0, 1),]
        );
        // Exactly at the right edge: (0, 10) == (1, 0).
        assert_eq!(
            selection_rects(&sel(Some((0, 10)), Some((2, 1)), true), SIZE, METRICS),
            vec![rect(1, 0, 10), rect(2, 0, 1),]
        );
        // Anchor advanced past the end row is degenerate.
        assert!(selection_rects(&sel(Some((0, 25)), Some((2, 1)), true), SIZE, METRICS).is_empty());
    }

    #[test]
    fn zero_sized_grid_is_empty() {
        let s = sel(Some((0, 0)), Some((1, 1)), true);
        assert!(selection_rects(&s, GridSize::new(0, 0), METRICS).is_empty());
        assert!(selection_rects(&s, GridSize::new(10, 0), METRICS).is_empty());
        assert!(selection_rects(&s, GridSize::new(0, 5), METRICS).is_empty());
    }

    #[test]
    fn reversed_multi_row_anchors_normalize() {
        assert_eq!(
            selection_rects(&sel(Some((3, 4)), Some((1, 8)), true), SIZE, METRICS),
            vec![
                rect(1, 8, 2),  // row 1: cols 8..10
                rect(2, 0, 10), // row 2: full
                rect(3, 0, 4),  // row 3: cols 0..4
            ]
        );
    }

    #[test]
    fn full_grid_span_is_one_rect_per_row() {
        assert_eq!(
            selection_rects(&sel(Some((0, 0)), Some((5, 0)), true), SIZE, METRICS),
            vec![
                rect(0, 0, 10),
                rect(1, 0, 10),
                rect(2, 0, 10),
                rect(3, 0, 10),
                rect(4, 0, 10),
            ]
        );
    }
}

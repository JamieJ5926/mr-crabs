//! Grid geometry for the built-in effects: cell metrics and the
//! change-texture row mapping in both row orientations.
//!
//! Port of the oracle's `iCellMetrics` computation
//! (`verification/manifests/dirty-oracle-v2.patch`,
//! `src/renderer/generic.zig`, new-file lines 2621-2653): the metrics are
//! `(cell width, signed row step, row-zero origin x, row-zero origin y)`;
//! for bottom-up shader coordinates (y-up graphics APIs) the row step is
//! negative and the origin is flipped to the top edge of row zero relative
//! to the bottom of the screen.
//!
//! The change texture is always one texel per grid cell with row 0 = the
//! top grid row; `cell_coord` maps a fragment position to that texture row
//! on both orientations.

/// A cell size in device pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellPx {
    pub width: f64,
    pub height: f64,
}

impl CellPx {
    pub const fn new(width: f64, height: f64) -> Self {
        Self { width, height }
    }
}

/// The native direction of the renderer's row axis.
///
/// * `TopDown` — row 0 is at the smallest y (Metal, and the default GPUI
///   presentation): origin at `padding_top`, positive row step.
/// * `BottomUp` — y grows upward (OpenGL-style fragment coordinates):
///   origin at `screen_height - padding_top - cell_height`, negative row
///   step (oracle `custom_shader_y_is_down == false`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RowOrientation {
    TopDown,
    BottomUp,
}

/// The `iCellMetrics` uniform values plus the fragment→cell mapping.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellMetricsUniform {
    pub cell_width_px: f64,
    pub row_step_px: f64,
    pub row_zero_origin_x: f64,
    pub row_zero_origin_y: f64,
}

impl CellMetricsUniform {
    /// Compute the metrics for an orientation (oracle
    /// `updateCustomShaderUniformsForFrame`).
    pub fn new(
        orientation: RowOrientation,
        cell: CellPx,
        padding_top_px: f64,
        padding_left_px: f64,
        screen_height_px: f64,
    ) -> Self {
        let (row_step_px, row_zero_origin_y) = match orientation {
            RowOrientation::TopDown => (cell.height, padding_top_px),
            RowOrientation::BottomUp => (
                -cell.height,
                screen_height_px - padding_top_px - cell.height,
            ),
        };
        Self {
            cell_width_px: cell.width,
            row_step_px,
            row_zero_origin_x: padding_left_px,
            row_zero_origin_y,
        }
    }

    /// Map a fragment position (in the renderer's native pixel space) to a
    /// grid cell in change-texture convention (row 0 = top grid row),
    /// following the oracle shader formula
    /// `cell = floor((fragCoord - origin) / metrics)`. Returns `None` when
    /// the fragment is outside the grid (negative coordinate on either
    /// axis).
    pub fn cell_coord(&self, frag_x_px: f64, frag_y_px: f64) -> Option<(u64, u64)> {
        let col = ((frag_x_px - self.row_zero_origin_x) / self.cell_width_px).floor();
        let row = ((frag_y_px - self.row_zero_origin_y) / self.row_step_px).floor();
        if col < 0.0 || row < 0.0 {
            return None;
        }
        Some((col as u64, row as u64))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CELL: CellPx = CellPx::new(10.0, 20.0);

    #[test]
    fn top_down_metrics_and_mapping() {
        let m = CellMetricsUniform::new(RowOrientation::TopDown, CELL, 5.0, 3.0, 800.0);
        assert_eq!(m.row_step_px, 20.0);
        assert_eq!(m.row_zero_origin_y, 5.0);
        assert_eq!(m.row_zero_origin_x, 3.0);
        // Row 0 spans y in [5, 25), col 0 x in [3, 13).
        assert_eq!(m.cell_coord(8.0, 10.0), Some((0, 0)));
        assert_eq!(m.cell_coord(13.0, 25.0), Some((1, 1)));
        assert_eq!(m.cell_coord(2.0, 10.0), None); // left of the grid
        assert_eq!(m.cell_coord(8.0, 4.0), None); // above the grid
    }

    #[test]
    fn bottom_up_metrics_are_flipped() {
        // Oracle: origin_y = screen.height - padding.top - cell.height and
        // the row step is negative.
        let m = CellMetricsUniform::new(RowOrientation::BottomUp, CELL, 5.0, 3.0, 800.0);
        assert_eq!(m.row_step_px, -20.0);
        assert_eq!(m.row_zero_origin_y, 775.0);
        assert_eq!(m.row_zero_origin_x, 3.0);
        // The formula maps the row band [origin - h, origin) to row 0 in
        // y-up space, and rows ascend as y decreases — the same texture
        // rows the top-down orientation produces for the same grid.
        assert_eq!(m.cell_coord(8.0, 765.0), Some((0, 0)));
        assert_eq!(m.cell_coord(8.0, 745.0), Some((0, 1)));
        assert_eq!(m.cell_coord(8.0, 775.0), Some((0, 0))); // top band edge
        assert_eq!(m.cell_coord(8.0, 785.0), None); // above the band
        assert_eq!(m.cell_coord(2.0, 765.0), None); // left of the grid
    }

    #[test]
    fn both_orientations_name_the_same_texture_rows() {
        let top = CellMetricsUniform::new(RowOrientation::TopDown, CELL, 5.0, 3.0, 800.0);
        let bottom = CellMetricsUniform::new(RowOrientation::BottomUp, CELL, 5.0, 3.0, 800.0);
        // Row 0 of the grid: top-down fragment y = 10; bottom-up fragment
        // y = 765 (the same physical row in flipped space).
        assert_eq!(top.cell_coord(8.0, 10.0), bottom.cell_coord(8.0, 765.0));
        assert_eq!(top.cell_coord(8.0, 30.0), bottom.cell_coord(8.0, 745.0));
    }
}

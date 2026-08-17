//! Cell geometry helpers: pixel↔grid conversion, resize deduplication, and
//! deterministic cell/run rectangles. All functions are pure math over
//! `gpui::Pixels` and are testable headlessly (no window required).

use gpui::{Bounds, Pixels, Point, point, px, size};
use mr_crabs_terminal::GridSize;

use crate::CellMetrics;

/// Convert pixel bounds into a nonzero grid size by flooring the pixel
/// extent against the cell metrics.
///
/// Returns `None` when the bounds are zero-sized or the metrics are not
/// finite/positive. Columns and rows are at least 1, and the division result
/// saturates at `u16::MAX` (Rust float→int casts saturate, never wrap).
pub fn pixel_bounds_to_grid(bounds: Bounds<Pixels>, metrics: CellMetrics) -> Option<GridSize> {
    let width = f32::from(bounds.size.width);
    let height = f32::from(bounds.size.height);
    if width <= 0.0 || height <= 0.0 || !width.is_finite() || !height.is_finite() {
        return None;
    }
    if metrics.width <= 0.0
        || metrics.height <= 0.0
        || !metrics.width.is_finite()
        || !metrics.height.is_finite()
    {
        return None;
    }
    let cols = (width / metrics.width).floor().max(1.0) as u16;
    let rows = (height / metrics.height).floor().max(1.0) as u16;
    Some(GridSize::new(cols, rows))
}

/// Tracks the last emitted resize and emits each distinct [`GridSize`] at
/// most once. The first `offer` always emits.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ResizeDeduper {
    last: Option<GridSize>,
}

impl ResizeDeduper {
    pub const fn new() -> Self {
        Self { last: None }
    }

    /// Returns `true` when `size` differs from the previously emitted size
    /// (or no size has been emitted yet), and records it as emitted.
    pub fn offer(&mut self, size: GridSize) -> bool {
        if self.last == Some(size) {
            return false;
        }
        self.last = Some(size);
        true
    }

    /// The most recently emitted size, if any.
    pub const fn current(&self) -> Option<GridSize> {
        self.last
    }
}

/// The pixel bounds of a single cell at `(col, row)` relative to `origin`.
pub fn cell_bounds(
    origin: Point<Pixels>,
    col: u16,
    row: u16,
    metrics: CellMetrics,
) -> Bounds<Pixels> {
    run_bounds(origin, col, 1, row, metrics)
}

/// The pixel bounds of a run of `len` cells starting at `(col, row)`,
/// relative to `origin`.
pub fn run_bounds(
    origin: Point<Pixels>,
    col: u16,
    len: u16,
    row: u16,
    metrics: CellMetrics,
) -> Bounds<Pixels> {
    Bounds::new(
        point(
            origin.x + px(f32::from(col) * metrics.width),
            origin.y + px(f32::from(row) * metrics.height),
        ),
        size(px(f32::from(len) * metrics.width), px(metrics.height)),
    )
}

/// The terminal-cell column of each text character in a shaped run.
///
/// `glyph_widths` is the run's per-character cell width (1 ordinary, 2
/// wide; parallel to the run text, from the render cache). Character `i`
/// belongs to terminal cell `start_col + sum(widths[..i])`. The paint pass
/// uses this to anchor every shaped glyph to its cell origin so natural
/// shaping advances can never accumulate drift against the grid.
pub fn glyph_cell_cols(start_col: u16, glyph_widths: &[u16]) -> Vec<u16> {
    let mut cols = Vec::with_capacity(glyph_widths.len());
    let mut col = start_col;
    for &width in glyph_widths {
        cols.push(col);
        col = col.saturating_add(width);
    }
    cols
}

/// The pixel origin of the cell at `(col, row)` relative to `origin`.
///
/// Every paint primitive — glyph, cursor, selection, background, IME —
/// shares this one cell-origin mapping: `origin + (col * cell_width,
/// row * cell_height)`. Nothing is positioned from a shaped advance or a
/// viewport-relative guess.
pub fn glyph_origin(
    origin: Point<Pixels>,
    row: u16,
    col: u16,
    metrics: CellMetrics,
) -> Point<Pixels> {
    point(
        origin.x + px(f32::from(col) * metrics.width),
        origin.y + px(f32::from(row) * metrics.height),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::px;

    const METRICS: CellMetrics = CellMetrics {
        width: 10.0,
        height: 20.0,
    };

    fn bounds(width: f32, height: f32) -> Bounds<Pixels> {
        Bounds::new(point(px(0.0), px(0.0)), size(px(width), px(height)))
    }

    #[test]
    fn pixel_bounds_to_grid_floors_to_cells() {
        assert_eq!(
            pixel_bounds_to_grid(bounds(800.0, 480.0), METRICS),
            Some(GridSize::new(80, 24))
        );
        // Partial cells are floored away.
        assert_eq!(
            pixel_bounds_to_grid(bounds(804.0, 489.0), METRICS),
            Some(GridSize::new(80, 24))
        );
        assert_eq!(
            pixel_bounds_to_grid(bounds(799.0, 479.0), METRICS),
            Some(GridSize::new(79, 23))
        );
    }

    #[test]
    fn pixel_bounds_to_grid_minimum_one() {
        assert_eq!(
            pixel_bounds_to_grid(bounds(1.0, 1.0), METRICS),
            Some(GridSize::new(1, 1))
        );
        assert_eq!(
            pixel_bounds_to_grid(bounds(5.0, 5.0), METRICS),
            Some(GridSize::new(1, 1))
        );
    }

    #[test]
    fn pixel_bounds_to_grid_floors_fractional_metrics() {
        let metrics = CellMetrics {
            width: 7.5,
            height: 12.5,
        };
        assert_eq!(
            pixel_bounds_to_grid(bounds(100.0, 50.0), metrics),
            Some(GridSize::new(13, 4))
        );
        // A fraction of a cell never rounds up.
        assert_eq!(
            pixel_bounds_to_grid(bounds(101.0, 51.0), metrics),
            Some(GridSize::new(13, 4))
        );
    }

    #[test]
    fn pixel_bounds_to_grid_rejects_zero_and_invalid() {
        assert_eq!(pixel_bounds_to_grid(bounds(0.0, 480.0), METRICS), None);
        assert_eq!(pixel_bounds_to_grid(bounds(800.0, 0.0), METRICS), None);
        assert_eq!(pixel_bounds_to_grid(bounds(-1.0, 480.0), METRICS), None);
        assert_eq!(
            pixel_bounds_to_grid(
                bounds(800.0, 480.0),
                CellMetrics {
                    width: 0.0,
                    height: 20.0
                }
            ),
            None
        );
        assert_eq!(
            pixel_bounds_to_grid(
                bounds(800.0, 480.0),
                CellMetrics {
                    width: f32::NAN,
                    height: 20.0
                }
            ),
            None
        );
        assert_eq!(
            pixel_bounds_to_grid(
                bounds(800.0, 480.0),
                CellMetrics {
                    width: 10.0,
                    height: f32::INFINITY
                }
            ),
            None
        );
    }

    #[test]
    fn pixel_bounds_to_grid_ignores_origin() {
        let offset = Bounds::new(point(px(37.0), px(12.0)), size(px(800.0), px(480.0)));
        assert_eq!(
            pixel_bounds_to_grid(offset, METRICS),
            Some(GridSize::new(80, 24))
        );
    }

    #[test]
    fn pixel_bounds_to_grid_saturates_rather_than_wrapping() {
        assert_eq!(
            pixel_bounds_to_grid(
                bounds(f32::MAX, f32::MAX),
                CellMetrics {
                    width: 1.0,
                    height: 1.0
                }
            ),
            Some(GridSize::new(u16::MAX, u16::MAX))
        );
    }

    #[test]
    fn resize_deduper_emits_each_size_once() {
        let mut deduper = ResizeDeduper::new();
        assert_eq!(deduper.current(), None);
        assert!(deduper.offer(GridSize::new(80, 24)));
        assert_eq!(deduper.current(), Some(GridSize::new(80, 24)));
        // Repeats do not emit.
        assert!(!deduper.offer(GridSize::new(80, 24)));
        assert!(!deduper.offer(GridSize::new(80, 24)));
        // A distinct size emits once, then is suppressed.
        assert!(deduper.offer(GridSize::new(100, 30)));
        assert!(!deduper.offer(GridSize::new(100, 30)));
        // Back-and-forth between two sizes emits each transition.
        assert!(deduper.offer(GridSize::new(80, 24)));
        assert!(deduper.offer(GridSize::new(100, 30)));
        assert_eq!(deduper.current(), Some(GridSize::new(100, 30)));
    }

    #[test]
    fn run_bounds_are_deterministic() {
        let origin = point(px(3.0), px(4.0));
        assert_eq!(
            run_bounds(origin, 2, 5, 3, METRICS),
            Bounds::new(point(px(23.0), px(64.0)), size(px(50.0), px(20.0)))
        );
        assert_eq!(
            cell_bounds(origin, 0, 0, METRICS),
            Bounds::new(point(px(3.0), px(4.0)), size(px(10.0), px(20.0)))
        );
    }

    #[test]
    fn run_bounds_are_exact_with_fractional_metrics() {
        let metrics = CellMetrics {
            width: 7.5,
            height: 12.5,
        };
        assert_eq!(
            run_bounds(point(px(0.0), px(0.0)), 3, 2, 1, metrics),
            Bounds::new(point(px(22.5), px(12.5)), size(px(15.0), px(12.5)))
        );
        // A nonzero origin offsets every cell deterministically.
        assert_eq!(
            cell_bounds(point(px(2.0), px(8.0)), 1, 2, metrics),
            Bounds::new(point(px(9.5), px(33.0)), size(px(7.5), px(12.5)))
        );
    }

    #[test]
    fn glyph_origins_equal_cell_origins() {
        // A shaped run's glyphs are re-anchored to their terminal cells:
        // glyph `i` lands on the cell origin of the run's `i`-th painted
        // cell (`content_origin + col * cell_width`), never on the shaper's
        // natural advance. A run starting at col 2 whose glyphs consume
        // 1, 2 (wide), 1 cells occupies cells 2, 3, 5.
        let metrics = CellMetrics {
            width: 8.5,
            height: 17.0,
        };
        let origin = point(px(40.0), px(60.0));
        let widths = [1u16, 2, 1];
        let cols = glyph_cell_cols(2, &widths);
        assert_eq!(cols, vec![2, 3, 5]);
        for &col in &cols {
            // The glyph origin is the cell origin: the same math as
            // cell_bounds, with no advance parameter anywhere.
            assert_eq!(
                glyph_origin(origin, 4, col, metrics),
                cell_bounds(origin, col, 4, metrics).origin
            );
            assert_eq!(
                glyph_origin(origin, 4, col, metrics),
                point(px(40.0 + f32::from(col) * 8.5), px(60.0 + 4.0 * 17.0),)
            );
        }
        // A run starting at column 0 maps to the content origin itself.
        assert_eq!(glyph_cell_cols(0, &[1, 1, 1]), vec![0, 1, 2]);
        assert_eq!(glyph_origin(origin, 0, 0, metrics), origin);
    }

    #[test]
    fn first_column_ink_is_inside_content() {
        // Padding precedes grid calculation: the content origin sits at the
        // padded viewport edge, and column 0's glyph ink must land inside
        // the content rect — never at the viewport origin (which would draw
        // under the padding) and never shifted by the shaper's advance.
        let metrics = CellMetrics {
            width: 8.0,
            height: 16.0,
        };
        let viewport = point(px(0.0), px(0.0));
        let padding = (12.0f32, 8.0f32);
        let content_origin = point(px(padding.0), px(padding.1));
        let content_extent = metrics.grid_extent(80, 24);
        let cols = glyph_cell_cols(0, &[1, 1, 1, 1, 1]);
        for &col in &cols {
            let o = glyph_origin(content_origin, 0, col, metrics);
            // Every glyph starts at or after the padded content edge.
            assert!(o.x >= content_origin.x, "col {col} starts inside content");
            assert!(
                o.x >= viewport.x + px(padding.0),
                "col {col} starts inside the left padding"
            );
            // And stays strictly within the content extent.
            assert!(
                o.x + px(metrics.width) <= content_origin.x + px(content_extent.width),
                "col {col} stays inside the content width"
            );
        }
    }
}

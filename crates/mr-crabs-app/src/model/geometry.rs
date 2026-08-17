//! Measured surface geometry: the pixel↔cell mapping for one terminal
//! surface (a window or one split pane).
//!
//! A [`SurfaceGeometry`] is the single source of truth for how a pixel
//! viewport maps onto the cell grid:
//!
//! - [`viewport`](SurfaceGeometry::viewport) is the full pixel extent of the
//!   surface (the window content area, or a split pane's rectangle).
//! - [`padding`](SurfaceGeometry::padding) is the border around the grid. It
//!   is applied *before* flooring partial cells: the grid is derived from
//!   `viewport - padding`.
//! - [`content`](SurfaceGeometry::content) is the pixel extent the grid
//!   actually fills after padding.
//! - [`grid`](SurfaceGeometry::grid) is the floored cell count: partial
//!   cells at the right/bottom edge are dropped, with at least one column
//!   and one row.
//! - [`cell_px`](SurfaceGeometry::cell_px) is the per-cell integer pixel
//!   size (the rounded metrics).
//! - [`pty_pixels`](SurfaceGeometry::pty_pixels) is the total PTY pixel size
//!   (`cols * cell_px.0`, `rows * cell_px.1`), saturated at `u16`.
//!
//! Every glyph/cursor/selection/background/IME primitive is positioned from
//! one origin plus [`CellMetrics`]; runs never accumulate their own advance,
//! so text cannot drift off the grid.
//!
//! Split panes derive their geometry with
//! [`for_rect`](SurfaceGeometry::for_rect): the pane rectangle from
//! [`SplitTree::rects`](crate::model::split::SplitTree::rects) is re-expressed
//! in the parent's content space with zero padding (padding is a window-level
//! concept and is consumed exactly once).

use mr_crabs_element::{CellMetrics, PixelExtent};
use mr_crabs_pty::PtySize;
use mr_crabs_terminal::GridSize;

use super::split::GridRect;

/// Window padding in device-independent pixels, applied before the grid is
/// computed. Zero by default.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct PaddingPx {
    pub left: u16,
    pub right: u16,
    pub top: u16,
    pub bottom: u16,
}

impl PaddingPx {
    pub const fn new(left: u16, right: u16, top: u16, bottom: u16) -> Self {
        Self {
            left,
            right,
            top,
            bottom,
        }
    }
}

/// The measured pixel↔cell mapping of one terminal surface.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SurfaceGeometry {
    /// Measured per-cell size in device-independent pixels.
    pub metrics: CellMetrics,
    /// Border applied before the grid is computed; zero in split derivatives.
    pub padding: PaddingPx,
    /// Full pixel extent of the surface.
    pub viewport: PixelExtent,
    /// Pixel extent the grid fills after padding.
    pub content: PixelExtent,
    /// Floored, nonzero cell count derived from `content`.
    pub grid: GridSize,
    /// Per-cell integer pixel size `(width, height)`: the rounded metrics.
    pub cell_px: (u16, u16),
    /// Total PTY pixel size `(width, height)`: `cols * cell_px.0` and
    /// `rows * cell_px.1`, saturated at `u16`.
    pub pty_pixels: (u16, u16),
}

impl SurfaceGeometry {
    /// Derive a surface from a pixel viewport, measured cell metrics, and
    /// window padding.
    ///
    /// Returns `None` when the viewport or metrics are not finite and
    /// positive, or when the padding consumes the entire viewport. Partial
    /// cells at the right/bottom edge are floored away (never rounded up),
    /// and the grid always has at least one column and one row.
    pub fn from_viewport(
        viewport: PixelExtent,
        metrics: CellMetrics,
        padding: PaddingPx,
    ) -> Option<Self> {
        if !viewport.width.is_finite()
            || !viewport.height.is_finite()
            || viewport.width <= 0.0
            || viewport.height <= 0.0
        {
            return None;
        }
        if !metrics.width.is_finite()
            || !metrics.height.is_finite()
            || metrics.width <= 0.0
            || metrics.height <= 0.0
        {
            return None;
        }
        let content = PixelExtent {
            width: viewport.width - f32::from(padding.left) - f32::from(padding.right),
            height: viewport.height - f32::from(padding.top) - f32::from(padding.bottom),
        };
        if content.width <= 0.0 || content.height <= 0.0 {
            return None;
        }
        let cols = (content.width / metrics.width).floor().max(1.0) as u16;
        let rows = (content.height / metrics.height).floor().max(1.0) as u16;
        let cell_px = (
            metrics.width.round().max(1.0) as u16,
            metrics.height.round().max(1.0) as u16,
        );
        let pty_pixels = (
            u16::saturating_mul(cols, cell_px.0),
            u16::saturating_mul(rows, cell_px.1),
        );
        Some(Self {
            metrics,
            padding,
            viewport,
            content,
            grid: GridSize::new(cols, rows),
            cell_px,
            pty_pixels,
        })
    }

    /// The derivative surface for one split pane rectangle.
    ///
    /// `rect` comes from [`SplitTree::rects`](crate::model::split::SplitTree::rects)
    /// and is expressed in this surface's content space, so the derivative
    /// carries zero padding, uses the pane's own cell count as its grid, and
    /// reports its pixel extent as both viewport and content.
    pub fn for_rect(&self, rect: GridRect) -> Self {
        let viewport = PixelExtent {
            width: f32::from(rect.width) * self.metrics.width,
            height: f32::from(rect.height) * self.metrics.height,
        };
        let pty_pixels = (
            u16::saturating_mul(rect.width, self.cell_px.0),
            u16::saturating_mul(rect.height, self.cell_px.1),
        );
        Self {
            metrics: self.metrics,
            padding: PaddingPx::default(),
            viewport,
            content: viewport,
            grid: GridSize::new(rect.width, rect.height),
            cell_px: self.cell_px,
            pty_pixels,
        }
    }

    /// The PTY geometry for this surface: grid size plus per-cell pixels.
    ///
    /// The returned [`PtySize`] reports integer per-cell dimensions; its
    /// winsize conversion derives the total pixel size as
    /// `cols * cell_width` / `rows * cell_height`, matching
    /// [`pty_pixels`](SurfaceGeometry::pty_pixels).
    pub fn pty_size(&self) -> PtySize {
        PtySize::new(
            self.grid.cols,
            self.grid.rows,
            self.cell_px.0,
            self.cell_px.1,
        )
        .expect("surface grid and cell pixels are always nonzero")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::split::{PaneId, SplitAxis, SplitTree};

    #[test]
    fn surface_geometry_floors_partial_cells() {
        // 100 / 8.4 = 11.9 → 11 cols; 50 / 17.3 = 2.89 → 2 rows. Partial
        // cells at the right/bottom edge are dropped, never rounded up.
        let metrics = CellMetrics::new(8.4, 17.3).expect("valid metrics");
        let geo = SurfaceGeometry::from_viewport(
            PixelExtent {
                width: 100.0,
                height: 50.0,
            },
            metrics,
            PaddingPx::default(),
        )
        .expect("derivable surface");
        assert_eq!(geo.grid, GridSize::new(11, 2));
        assert_eq!(
            geo.content,
            PixelExtent {
                width: 100.0,
                height: 50.0
            }
        );
        // Rounded per-cell pixels: 8.4 → 8, 17.3 → 17.
        assert_eq!(geo.cell_px, (8, 17));
        // Integer totals from the floored grid: 11*8, 2*17.
        assert_eq!(geo.pty_pixels, (88, 34));
        assert_eq!(
            geo.pty_size(),
            PtySize::new(11, 2, 8, 17).expect("valid size")
        );

        // A viewport smaller than one full cell still yields one column/row.
        let tiny = SurfaceGeometry::from_viewport(
            PixelExtent {
                width: 8.0,
                height: 16.0,
            },
            metrics,
            PaddingPx::default(),
        )
        .expect("derivable surface");
        assert_eq!(tiny.grid, GridSize::new(1, 1));
    }

    #[test]
    fn surface_geometry_applies_padding() {
        // One cell of padding on every side at 10x20 metrics: 10 px
        // left/right, 20 px top/bottom. Content = 800-20 x 480-40, and the
        // floored grid is 78x22 (padding is applied before flooring).
        let metrics = CellMetrics::new(10.0, 20.0).expect("valid metrics");
        let padding = PaddingPx::new(10, 10, 20, 20);
        let geo = SurfaceGeometry::from_viewport(
            PixelExtent {
                width: 800.0,
                height: 480.0,
            },
            metrics,
            padding,
        )
        .expect("derivable surface");
        assert_eq!(
            geo.content,
            PixelExtent {
                width: 780.0,
                height: 440.0
            }
        );
        assert_eq!(geo.grid, GridSize::new(78, 22));
        assert_eq!(geo.pty_pixels, (780, 440));
        assert_eq!(geo.padding, padding);
        assert_eq!(
            geo.viewport,
            PixelExtent {
                width: 800.0,
                height: 480.0
            }
        );

        // Padding that leaves no room for a single cell is invalid.
        assert_eq!(
            SurfaceGeometry::from_viewport(
                PixelExtent {
                    width: 20.0,
                    height: 40.0,
                },
                metrics,
                PaddingPx::new(15, 15, 0, 0),
            ),
            None
        );
        assert_eq!(
            SurfaceGeometry::from_viewport(
                PixelExtent {
                    width: 30.0,
                    height: 40.0,
                },
                metrics,
                PaddingPx::new(15, 15, 0, 0),
            ),
            None
        );

        // Zero-sized or non-finite inputs are invalid.
        assert_eq!(
            SurfaceGeometry::from_viewport(
                PixelExtent {
                    width: 0.0,
                    height: 40.0
                },
                metrics,
                PaddingPx::default(),
            ),
            None
        );
        assert_eq!(
            SurfaceGeometry::from_viewport(
                PixelExtent {
                    width: 100.0,
                    height: 40.0
                },
                CellMetrics {
                    width: f32::NAN,
                    height: 20.0,
                },
                PaddingPx::default(),
            ),
            None
        );
    }

    #[test]
    fn split_rects_produce_distinct_pty_sizes() {
        // 80x25 grid split vertically at ratio 0.5: rows round to 13 and 12,
        // so the two pane derivatives report distinct PTY sizes.
        let mut tree = SplitTree::leaf(PaneId::new(1));
        assert!(tree.split(PaneId::new(1), SplitAxis::Vertical, PaneId::new(2)));
        let metrics = CellMetrics::new(8.0, 16.0).expect("valid metrics");
        let geo = SurfaceGeometry::from_viewport(
            PixelExtent {
                width: 640.0,
                height: 400.0,
            },
            metrics,
            PaddingPx::default(),
        )
        .expect("derivable surface");
        assert_eq!(geo.grid, GridSize::new(80, 25));

        let rects = tree.rects(geo.grid);
        assert_eq!(rects.len(), 2);
        let top = geo.for_rect(rects[&PaneId::new(1)]);
        let bottom = geo.for_rect(rects[&PaneId::new(2)]);
        assert_ne!(top.pty_size(), bottom.pty_size());
        assert_ne!(top.pty_pixels, bottom.pty_pixels);

        // The derivatives tile the parent exactly and keep the full width.
        assert_eq!(top.grid.rows + bottom.grid.rows, geo.grid.rows);
        assert_eq!(top.grid.cols, geo.grid.cols);
        assert_eq!(top.grid.cols, bottom.grid.cols);
        // Zero padding in derivatives; extent equals the pane rectangle.
        assert_eq!(top.padding, PaddingPx::default());
        assert_eq!(top.viewport, top.content);
        assert_eq!(
            top.viewport,
            PixelExtent {
                width: 640.0,
                height: 208.0,
            }
        );
        // PTY totals agree with the derived per-cell pixels.
        assert_eq!(top.pty_size().to_winsize().ws_xpixel, top.pty_pixels.0);
        assert_eq!(top.pty_size().to_winsize().ws_ypixel, top.pty_pixels.1);
    }
}

//! Placement geometry: pixel sizes, grid sizes, rectangles, and the
//! terminal context they depend on.
//!
//! Provenance: `src/terminal/kitty/graphics_storage.zig` `Placement`
//! (`pixelSize`, `gridSize`, `rect`, saturating helpers) at Ghostty commit
//! `d2c70a8c7b9b6893c13640c02d7b6f9a1624f3f0`.

use crate::image::Image;

/// A cell position in active-screen coordinates.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Point {
    pub x: u32,
    pub y: u32,
}

/// A pixel size.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

/// Terminal geometry and viewport state supplied by the host for every
/// store operation. Placements are anchored to absolute scrollback rows
/// (`viewport_first_row` maps active row 0 to its absolute row), so
/// scrolling the viewport never mutates the store while `prune_history`
/// removes placements that scrolled out of retained history.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalContext {
    /// Absolute scrollback row of the first active-screen row.
    pub viewport_first_row: u64,
    /// Active-screen cursor position.
    pub cursor: Point,
    /// Grid dimensions in cells.
    pub cols: u32,
    pub rows: u32,
    /// Grid dimensions in pixels (the placement math divides by cell size).
    pub width_px: u32,
    pub height_px: u32,
}

impl Default for TerminalContext {
    fn default() -> Self {
        Self {
            viewport_first_row: 0,
            cursor: Point::default(),
            cols: 80,
            rows: 24,
            width_px: 800,
            height_px: 600,
        }
    }
}

/// A rectangle of grid cells, in absolute (scrollback) row space.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rect {
    pub top_left: (u64, u32),
    pub bottom_right: (u64, u32),
}

impl Rect {
    /// True if the absolute grid cell is inside this rectangle.
    pub fn contains(&self, row: u64, col: u32) -> bool {
        row >= self.top_left.0
            && row <= self.bottom_right.0
            && col >= self.top_left.1
            && col <= self.bottom_right.1
    }
}

/// Placement identifier: zero on the wire becomes a fresh internal id (so
/// multiple anonymous placements per image are valid); non-zero ids are
/// external and unique per (image id, placement id) pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum PlacementId {
    Internal(u32),
    External(u32),
}

/// Every placement is uniquely identified by its image id and placement id
/// (`graphics_storage.zig` `PlacementKey`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PlacementKey {
    pub image_id: u32,
    pub placement_id: PlacementId,
}

/// Placement location: an exact pinned grid position, or a virtual
/// (untracked) placement for unicode placeholders.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Location {
    Pin {
        row: u64,
        col: u32,
    },
    #[default]
    Virtual,
}

/// A placement of an image on the grid.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Placement {
    pub location: Location,
    /// Offset of the x/y from the top-left of the cell.
    pub x_offset: u32,
    pub y_offset: u32,
    /// Source rectangle within the image.
    pub source_x: u32,
    pub source_y: u32,
    pub source_width: u32,
    pub source_height: u32,
    /// The columns/rows this placement occupies (0 = derive from pixels).
    pub columns: u32,
    pub rows: u32,
    /// iTerm2 pixel/percentage dimensions. These take precedence over cell
    /// counts and are resolved against the current surface context.
    pub pixel_width: Option<u32>,
    pub pixel_height: Option<u32>,
    pub percent_width: Option<u32>,
    pub percent_height: Option<u32>,
    pub preserve_aspect: bool,
    /// The z-index for this placement.
    pub z: i32,
}

impl Placement {
    /// Multiply two protocol-controlled values without wrapping; values
    /// larger than u32 saturate (`saturatingMul`).
    fn saturating_mul(lhs: u32, rhs: u32) -> u32 {
        lhs.saturating_mul(rhs)
    }

    /// Scale a dimension by an aspect ratio and round to nearest. The u64
    /// intermediate holds the product of two u32s plus rounding.
    fn scale_dimension(value: u32, numerator: u32, denominator: u32) -> u32 {
        if denominator == 0 {
            return 0;
        }
        let rounded =
            (value as u64 * numerator as u64 + denominator as u64 / 2) / denominator as u64;
        u32::try_from(rounded).unwrap_or(u32::MAX)
    }

    fn cell_size(ctx: &TerminalContext) -> Option<Size> {
        if ctx.cols == 0 || ctx.rows == 0 {
            return None;
        }
        Some(Size {
            width: ctx.width_px / ctx.cols,
            height: ctx.height_px / ctx.rows,
        })
    }

    /// Pixel size of this placement, honoring the source rectangle,
    /// specified rows/columns, and aspect ratio (`pixelSize`).
    pub fn pixel_size(&self, image: &Image, ctx: &TerminalContext) -> Size {
        let width = if self.source_width > 0 {
            self.source_width
        } else {
            image.width
        };
        let height = if self.source_height > 0 {
            self.source_height
        } else {
            image.height
        };

        let target_width = self
            .pixel_width
            .or_else(|| {
                self.percent_width
                    .map(|percent| Self::scale_dimension(ctx.width_px, percent, 100))
            })
            .or_else(|| {
                (self.columns > 0)
                    .then(|| Self::saturating_mul(ctx.width_px / ctx.cols.max(1), self.columns))
            });
        let target_height = self
            .pixel_height
            .or_else(|| {
                self.percent_height
                    .map(|percent| Self::scale_dimension(ctx.height_px, percent, 100))
            })
            .or_else(|| {
                (self.rows > 0)
                    .then(|| Self::saturating_mul(ctx.height_px / ctx.rows.max(1), self.rows))
            });

        match (target_width, target_height) {
            (None, None) => Size { width, height },
            (Some(target_width), Some(target_height)) if self.preserve_aspect => {
                let width_limited_height = Self::scale_dimension(target_width, height, width);
                if width_limited_height <= target_height {
                    Size {
                        width: target_width,
                        height: width_limited_height,
                    }
                } else {
                    Size {
                        width: Self::scale_dimension(target_height, width, height),
                        height: target_height,
                    }
                }
            }
            (Some(target_width), Some(target_height)) => Size {
                width: target_width,
                height: target_height,
            },
            (Some(target_width), None) => Size {
                width: target_width,
                height: Self::scale_dimension(target_width, height, width),
            },
            (None, Some(target_height)) => Size {
                width: Self::scale_dimension(target_height, width, height),
                height: target_height,
            },
        }
    }
    fn div_ceil(n: u64, d: u64) -> u32 {
        if d == 0 {
            return 0;
        }
        u32::try_from(n.div_ceil(d)).unwrap_or(u32::MAX)
    }

    /// Size in grid cells this placement takes up (`gridSize`).
    pub fn grid_size(&self, image: &Image, ctx: &TerminalContext) -> Size {
        if self.columns > 0
            && self.rows > 0
            && self.pixel_width.is_none()
            && self.pixel_height.is_none()
            && self.percent_width.is_none()
            && self.percent_height.is_none()
            && !self.preserve_aspect
        {
            return Size {
                width: self.columns,
                height: self.rows,
            };
        }

        let calc = self.pixel_size(image, ctx);
        let Some(cell) = Self::cell_size(ctx) else {
            return Size {
                width: 0,
                height: 0,
            };
        };
        Size {
            width: Self::div_ceil(calc.width as u64 + self.x_offset as u64, cell.width as u64),
            height: Self::div_ceil(
                calc.height as u64 + self.y_offset as u64,
                cell.height as u64,
            ),
        }
    }

    /// The rectangle this placement occupies within the screen, or None for
    /// virtual placements or when unavailable pixel geometry makes it empty
    /// (`rect`).
    pub fn rect(&self, image: &Image, ctx: &TerminalContext) -> Option<Rect> {
        let grid = self.grid_size(image, ctx);
        let (row, col) = match self.location {
            Location::Pin { row, col } => (row, col),
            Location::Virtual => return None,
        };
        if grid.width == 0 || grid.height == 0 || ctx.cols == 0 {
            return None;
        }

        let br_col = col.saturating_add(grid.width - 1).min(ctx.cols - 1);
        Some(Rect {
            top_left: (row, col),
            bottom_right: (row.saturating_add(grid.height as u64 - 1), br_col),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::image::{ImageData, ImageFormat};

    fn image(w: u32, h: u32) -> Image {
        Image {
            id: 1,
            number: 0,
            width: w,
            height: h,
            format: ImageFormat::Rgba,
            compression: crate::image::Compression::None,
            data: ImageData::Complete(vec![0u8; (w * h * 4) as usize]),
            transient: false,
            implicit_id: false,
            placement_count: 0,
            generation: 0,
        }
    }

    #[test]
    fn native_size_when_no_grid_specified() {
        let p = Placement {
            location: Location::Virtual,
            ..Placement::default()
        };
        let ctx = TerminalContext::default();
        assert_eq!(
            p.pixel_size(&image(50, 76), &ctx),
            Size {
                width: 50,
                height: 76
            }
        );
    }

    #[test]
    fn source_rectangle_overrides_native_size() {
        let p = Placement {
            location: Location::Virtual,
            source_width: 10,
            source_height: 20,
            ..Placement::default()
        };
        assert_eq!(
            p.pixel_size(&image(50, 76), &TerminalContext::default()),
            Size {
                width: 10,
                height: 20
            }
        );
    }

    #[test]
    fn columns_and_rows_map_to_cell_sizes() {
        // 800px / 80 cols = 10px cell; 600px / 24 rows = 25px cell.
        let p = Placement {
            location: Location::Virtual,
            columns: 4,
            rows: 2,
            ..Placement::default()
        };
        assert_eq!(
            p.pixel_size(&image(50, 76), &TerminalContext::default()),
            Size {
                width: 40,
                height: 50
            }
        );
    }

    #[test]
    fn columns_only_scales_height_by_aspect() {
        // width = 4 cells * 10 = 40; aspect 50:76 -> height = 40*76/50 = 60.8 -> 61.
        let p = Placement {
            location: Location::Virtual,
            columns: 4,
            ..Placement::default()
        };
        assert_eq!(
            p.pixel_size(&image(50, 76), &TerminalContext::default()),
            Size {
                width: 40,
                height: 61
            }
        );
    }

    #[test]
    fn rows_only_scales_width_by_aspect() {
        // height = 2 cells * 25 = 50; aspect -> width = 50*50/76 = 32.9 -> 33.
        let p = Placement {
            location: Location::Virtual,
            rows: 2,
            ..Placement::default()
        };
        assert_eq!(
            p.pixel_size(&image(50, 76), &TerminalContext::default()),
            Size {
                width: 33,
                height: 50
            }
        );
    }

    #[test]
    fn saturating_geometry_for_untrusted_values() {
        let p = Placement {
            location: Location::Virtual,
            columns: u32::MAX,
            rows: u32::MAX,
            ..Placement::default()
        };
        assert_eq!(
            p.pixel_size(&image(1, 1), &TerminalContext::default()),
            Size {
                width: u32::MAX,
                height: u32::MAX
            }
        );
        // grid_size for explicit cols/rows returns them directly (bounded by
        // rect's clamp to the screen).
        assert_eq!(
            p.grid_size(&image(1, 1), &TerminalContext::default()),
            Size {
                width: u32::MAX,
                height: u32::MAX
            }
        );
    }

    #[test]
    fn rect_clamps_to_screen_and_converts_absolute_rows() {
        let ctx = TerminalContext {
            viewport_first_row: 100,
            cols: 10,
            rows: 5,
            ..TerminalContext::default()
        };
        let p = Placement {
            location: Location::Pin { row: 100, col: 3 },
            columns: 4,
            rows: 2,
            ..Placement::default()
        };
        let r = p.rect(&image(10, 10), &ctx).unwrap();
        assert_eq!(r.top_left, (100, 3));
        assert_eq!(r.bottom_right, (101, 6));

        // Clamp the right edge to the screen width.
        let p2 = Placement {
            location: Location::Pin { row: 100, col: 9 },
            columns: 4,
            rows: 2,
            ..Placement::default()
        };
        let r2 = p2.rect(&image(10, 10), &ctx).unwrap();
        assert_eq!(r2.bottom_right, (101, 9));
    }

    #[test]
    fn rect_none_for_virtual_and_empty_geometry() {
        let ctx = TerminalContext::default();
        let p = Placement {
            location: Location::Virtual,
            ..Placement::default()
        };
        assert!(p.rect(&image(10, 10), &ctx).is_none());

        let p = Placement {
            location: Location::Pin { row: 0, col: 0 },
            ..Placement::default()
        };
        // Zero grid size (no columns/rows and no pixel geometry) -> None.
        assert!(p.rect(&image(0, 0), &ctx).is_none());
    }

    #[test]
    fn rect_contains() {
        let r = Rect {
            top_left: (5, 2),
            bottom_right: (7, 4),
        };
        assert!(r.contains(5, 2));
        assert!(r.contains(7, 4));
        assert!(r.contains(6, 3));
        assert!(!r.contains(8, 3));
        assert!(!r.contains(6, 5));
        assert!(!r.contains(6, 1));
    }

    #[test]
    fn placement_defaults() {
        let p = Placement::default();
        assert_eq!(p.location, Location::Virtual);
        assert_eq!(p.z, 0);
    }
    #[test]
    fn preserve_aspect_fits_both_dimensions_and_percent_tracks_context() {
        let ctx = TerminalContext {
            width_px: 800,
            height_px: 480,
            ..TerminalContext::default()
        };
        let mut placement = Placement {
            location: Location::Virtual,
            columns: 10,
            rows: 10,
            preserve_aspect: true,
            ..Placement::default()
        };
        assert_eq!(
            placement.pixel_size(&image(100, 50), &ctx),
            Size {
                width: 100,
                height: 50
            }
        );
        placement.preserve_aspect = false;
        assert_eq!(
            placement.pixel_size(&image(100, 50), &ctx),
            Size {
                width: 100,
                height: 200
            }
        );

        placement.columns = 0;
        placement.rows = 0;
        placement.percent_width = Some(50);
        placement.pixel_height = None;
        assert_eq!(placement.pixel_size(&image(100, 50), &ctx).width, 400);
        let resized = TerminalContext {
            width_px: 1000,
            ..ctx
        };
        assert_eq!(placement.pixel_size(&image(100, 50), &resized).width, 500);
    }
}

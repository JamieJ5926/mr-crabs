//! S4: a custom GPUI terminal element for the Mr Crabs rewrite.
//!
//! This crate renders [`FrameDelta`]s produced by `mr-crabs-terminal` through
//! the GPUI element pipeline at the pinned zed revision
//! `03e5ad8a630c84c3990055905d0444ea0a519b7f`. It owns the paint-ready render
//! cache (retained, allocation-free on unchanged frames), cell geometry and
//! resize deduplication, cursor/selection geometry, and the
//! [`TerminalElement`] GPUI element itself.
//!
//! The element never locks the terminal engine: it consumes an owned
//! [`FrameDelta`] (or a shared `Arc<FrameDelta>`), so paint is a pure function
//! of the frame plus the render cache. No wgpu, no libghostty-vt, no Zig;
//! font resolution and shaping go through GPUI's text system (CoreText on
//! macOS).

mod cache;
mod cursor;
mod element;
mod geometry;
mod palette;
mod selection;

pub use cache::{CacheAction, Capacities, RectBatch, RenderCache, RowBatch, RunBatch};
pub use cursor::{
    BlinkHalfPeriod, CursorGeometry, CursorStateExt, blink_phase_active, cursor_geometry,
    needs_blink_animation, needs_blink_animation_with_phase, should_request_animation,
};
pub use element::{GraphicsOverlay, TerminalElement, TerminalInputHandler};
pub use geometry::{
    ResizeDeduper, cell_bounds, glyph_cell_cols, glyph_origin, pixel_bounds_to_grid, run_bounds,
};
pub use mr_crabs_graphics::placement::{Point, TerminalContext};
pub use palette::{
    ANSI_PALETTE, TerminalPalette, background_color, color_to_hsla, cursor_color, indexed_rgb,
    named_rgb, selection_color, style_background, style_foreground, style_underline,
};
pub use selection::selection_rects;

use mr_crabs_terminal::{Cell, Run};

/// Terminal cell metrics in pixels: the width and height of one grid cell.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellMetrics {
    pub width: f32,
    pub height: f32,
}

impl CellMetrics {
    /// Returns `None` when the metrics are not finite or not positive.
    pub fn new(width: f32, height: f32) -> Option<Self> {
        (width.is_finite() && width > 0.0 && height.is_finite() && height > 0.0)
            .then_some(Self { width, height })
    }

    pub fn grid_extent(self, cols: u16, rows: u16) -> PixelExtent {
        PixelExtent {
            width: self.width * f32::from(cols),
            height: self.height * f32::from(rows),
        }
    }
}

/// The pixel extent of a grid in device-independent pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct PixelExtent {
    pub width: f32,
    pub height: f32,
}

/// Derive contiguous same-style runs from a row of cells.
///
/// This is the element-side batching helper (simple style coalescing) used
/// by the render-cache tests: a run is a maximal span of cells with the same
/// style index in ascending column order, and a style index reappearing
/// after a different style starts a new run (no merging across style
/// boundaries).
///
/// The engine-side batching invariant lives in
/// [`mr_crabs_terminal::delta::batch_runs`], which additionally keeps a
/// `WIDE` cell's `WIDE_SPACER` successor in the same run regardless of
/// style so wide pairs are never split across runs. Both helpers agree on
/// `u16` bounds: the grid width is `u16`-bounded and every run start column
/// and length converts with `u16::try_from`, panicking only on rows that
/// exceed `u16::MAX`.
pub fn batch_runs(cells: &[Cell]) -> Vec<Run> {
    let mut runs: Vec<Run> = Vec::new();
    for (col, cell) in cells.iter().enumerate() {
        let col = u16::try_from(col).expect("row fits u16");
        match runs.last_mut() {
            Some(run)
                if run.style == cell.style
                    && u32::from(run.start_col) + u32::from(run.len) == u32::from(col) =>
            {
                run.len += 1;
            }
            _ => runs.push(Run {
                start_col: col,
                len: 1,
                style: cell.style,
            }),
        }
    }
    runs
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::Element;
    use mr_crabs_terminal::GridSize;
    use std::path::{Path, PathBuf};

    #[test]
    fn cell_metrics_validate() {
        assert_eq!(
            CellMetrics::new(7.0, 14.0),
            Some(CellMetrics {
                width: 7.0,
                height: 14.0
            })
        );
        assert_eq!(CellMetrics::new(0.0, 14.0), None);
        assert_eq!(CellMetrics::new(7.0, 0.0), None);
        assert_eq!(CellMetrics::new(f32::NAN, 14.0), None);
        assert_eq!(CellMetrics::new(7.0, f32::INFINITY), None);
    }

    #[test]
    fn grid_extent_scales_metrics() {
        let metrics = CellMetrics::new(10.0, 20.0).unwrap();
        assert_eq!(
            metrics.grid_extent(80, 24),
            PixelExtent {
                width: 800.0,
                height: 480.0
            }
        );
    }

    #[test]
    fn batch_runs_merges_contiguous_same_style() {
        let cells = vec![
            Cell {
                content: u32::from('a'),
                style: 0,
                flags: 0,
            },
            Cell {
                content: u32::from('b'),
                style: 0,
                flags: 0,
            },
            Cell {
                content: u32::from('c'),
                style: 1,
                flags: 0,
            },
            Cell {
                content: u32::from('d'),
                style: 1,
                flags: 0,
            },
            Cell {
                content: u32::from('e'),
                style: 1,
                flags: 0,
            },
            Cell {
                content: u32::from('f'),
                style: 0,
                flags: 0,
            },
        ];
        assert_eq!(
            batch_runs(&cells),
            vec![
                Run {
                    start_col: 0,
                    len: 2,
                    style: 0
                },
                Run {
                    start_col: 2,
                    len: 3,
                    style: 1
                },
                Run {
                    start_col: 5,
                    len: 1,
                    style: 0
                },
            ]
        );
    }

    #[test]
    fn batch_runs_handles_empty_and_single() {
        assert_eq!(batch_runs(&[]), Vec::<Run>::new());
        assert_eq!(
            batch_runs(&[Cell::default()]),
            vec![Run {
                start_col: 0,
                len: 1,
                style: 0
            }]
        );
    }

    #[test]
    fn batch_runs_style_reuse_does_not_merge_across_boundaries() {
        // Same style reappearing after a different style must start a new run.
        let cells = vec![
            Cell {
                content: 1,
                style: 7,
                flags: 0,
            },
            Cell {
                content: 2,
                style: 3,
                flags: 0,
            },
            Cell {
                content: 3,
                style: 7,
                flags: 0,
            },
        ];
        assert_eq!(
            batch_runs(&cells),
            vec![
                Run {
                    start_col: 0,
                    len: 1,
                    style: 7
                },
                Run {
                    start_col: 1,
                    len: 1,
                    style: 3
                },
                Run {
                    start_col: 2,
                    len: 1,
                    style: 7
                },
            ]
        );
    }

    #[test]
    fn size_matches_terminal_grid_type() {
        let _ = GridSize::new(80, 24);
        // batch_runs result columns are u16-capped like the grid itself.
        let cells: Vec<Cell> = (0..80).map(|_| Cell::default()).collect();
        let runs = batch_runs(&cells);
        assert_eq!(runs.len(), 1);
        assert_eq!(runs[0].start_col, 0);
        assert_eq!(runs[0].len, 80);
    }

    /// Recursively collect every `.rs` file under `dir` into `out`, sorting
    /// directory entries at every level so the scan order is deterministic.
    fn collect_rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
        let mut entries: Vec<_> = std::fs::read_dir(dir)
            .unwrap_or_else(|err| panic!("read_dir {}: {err}", dir.display()))
            .collect::<Result<_, _>>()
            .unwrap_or_else(|err| panic!("read_dir {} entry: {err}", dir.display()));
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            if path.is_dir() {
                collect_rs_files(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                out.push(path);
            }
        }
    }

    /// The production prefix of a source file: everything before the first
    /// `#[cfg(test)]` module. Test-only fixtures below that boundary are
    /// never inspected by the live-path regression.
    fn production_prefix(src: &str) -> &str {
        match src.find("#[cfg(test)]") {
            Some(at) => &src[..at],
            None => src,
        }
    }

    #[test]
    fn no_hardcoded_7x14_on_live_path() {
        // The live paint path derives cell metrics from the measured font
        // and the grid from the padded content viewport. None of the
        // production paint/layout sources may hardcode the old 7x14 default
        // cell, guess a grid size, or call the obsolete pre-measurement
        // resize API.
        //
        // Deterministic recursive scan: every .rs file under this crate's
        // src and the sibling app crate's src, with directory entries sorted
        // at every level, so the scanned source set is stable and no file
        // can be silently skipped.
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
        let mut files = Vec::new();
        collect_rs_files(&manifest.join("src"), &mut files);
        collect_rs_files(
            &manifest
                .parent()
                .expect("crate manifest has a parent directory")
                .join("mr-crabs-app/src"),
            &mut files,
        );
        files.sort();

        // The scan must actually cover the live paint/model sources; if the
        // roots above ever stop resolving, these fail instead of passing
        // vacuously.
        for required in [
            "element.rs",
            "cache.rs",
            "cursor.rs",
            "geometry.rs",
            "selection.rs",
            "ui/workspace.rs",
            "model/app_model.rs",
            "model/pane.rs",
            "model/geometry.rs",
        ] {
            assert!(
                files.iter().any(|path| path.ends_with(required)),
                "scan missed {required}; production sources are not covered"
            );
        }

        // Only the production prefix before the first `#[cfg(test)]` module
        // is inspected, so legitimate test fixtures stay accepted.
        for file in &files {
            let src = std::fs::read_to_string(file)
                .unwrap_or_else(|err| panic!("read {}: {err}", file.display()));
            let prefix = production_prefix(&src);
            for needle in ["7.0, 14.0", "7, 14", "DEFAULT_CELL", "resize_window"] {
                assert!(
                    !prefix.contains(needle),
                    "{} hardcodes the 7x14 default cell, a grid guess, or the obsolete resize_window symbol",
                    file.display()
                );
            }
            if file.ends_with("ui/workspace.rs") {
                assert!(
                    !prefix.contains("window.bounds()"),
                    "{} sizes the terminal from outer window bounds instead of the measured viewport",
                    file.display()
                );
            }
        }
        // Constructors take explicit metrics and derive every dependent
        // value from them: a 9x17 cell produces a 17px glyph font, never a
        // 14px default, and no built-in grid is assumed anywhere. The
        // element stays unkeyed until the app pins an element id.
        let metrics = CellMetrics::new(9.0, 17.0).expect("explicit metrics");
        let mut pool = mr_crabs_terminal::FramePool::new(1);
        let frame = pool.acquire(1, mr_crabs_terminal::GridSize::new(80, 24));
        let element = TerminalElement::new(frame, metrics);
        assert_eq!(element.metrics(), metrics);
        assert_eq!(element.content_origin(), None);
        assert_eq!(element.id(), None);
    }
}

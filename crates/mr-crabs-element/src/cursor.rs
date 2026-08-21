//! Cursor rendering logic: shape→rectangle mapping, blink phase tracking,
//! and animation scheduling. All pure functions — headless-testable.

use gpui::{Bounds, Pixels, point, px, size};
use mr_crabs_terminal::{CursorShape, CursorState, DamageKind, FrameDelta};
use std::time::Instant;

use crate::{CellMetrics, cell_bounds};

/// Number of frames per blink half-period. The blink phase is derived from
/// the frame sequence, so it is deterministic and needs no wall clock.
pub struct BlinkHalfPeriod;

impl BlinkHalfPeriod {
    pub const FRAMES: u64 = 30;
}

/// True when the cursor should be visible for this frame's blink phase.
///
/// The phase alternates every [`BlinkHalfPeriod::FRAMES`] sequences, so
/// `0..30` visible, `30..60` hidden, and so on.
pub fn blink_phase_active(sequence: u64) -> bool {
    (sequence / BlinkHalfPeriod::FRAMES) % 2 == 0
}

/// Wall-clock blink phase used by the live element. A frame sequence does not
/// advance while the terminal is idle, so live blinking must use elapsed time.
pub fn blink_phase_at_millis(elapsed_ms: u64) -> bool {
    (elapsed_ms / 500) % 2 == 0
}

/// Retained monotonic blink timing for one terminal element.
///
/// Terminal activity or a cursor-state change starts a fresh visible phase.
/// Animation repaints of the same frame continue from the retained epoch.
#[derive(Debug, Default)]
pub(crate) struct BlinkState {
    last_sequence: Option<u64>,
    last_cursor: Option<CursorState>,
    epoch: Option<Instant>,
}

impl BlinkState {
    pub(crate) fn phase_at(&mut self, frame: &FrameDelta, now: Instant) -> bool {
        let changed = self.last_sequence != Some(frame.sequence)
            || self.last_cursor.as_ref() != Some(&frame.cursor);
        if changed {
            self.last_sequence = Some(frame.sequence);
            self.last_cursor = Some(frame.cursor);
            self.epoch = Some(now);
        }
        let epoch = *self.epoch.get_or_insert(now);
        let elapsed_ms = now.saturating_duration_since(epoch).as_millis();
        blink_phase_at_millis(u64::try_from(elapsed_ms).unwrap_or(u64::MAX))
    }
}

/// The subset of cursor state that affects rendering, with the flag→rect
/// mapping. `wrap_pending` is carried verbatim: consumers decide whether the
/// cursor should be drawn at `col` or shifted to the next line (DECAWM
/// policy), the rect math is identical either way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CursorStateExt {
    pub shape: CursorShape,
    pub blinking: bool,
    pub visible: bool,
    pub wrap_pending: bool,
}

impl From<&CursorState> for CursorStateExt {
    fn from(cursor: &CursorState) -> Self {
        Self {
            shape: cursor.shape,
            blinking: cursor.blinking,
            visible: cursor.visible,
            wrap_pending: cursor.wrap_pending,
        }
    }
}

impl CursorStateExt {
    /// The rendering rect for this cursor at `(col, row)`, at grid origin.
    pub fn geometry(self, col: u16, row: u16, metrics: CellMetrics) -> CursorGeometry {
        let cell = cell_bounds(point(px(0.0), px(0.0)), col, row, metrics);
        match self.shape {
            CursorShape::Block | CursorShape::HollowBlock => CursorGeometry {
                bounds: cell,
                shape: self.shape,
            },
            CursorShape::Bar => CursorGeometry {
                bounds: Bounds::new(
                    cell.origin,
                    size(bar_width(metrics.width), cell.size.height),
                ),
                shape: self.shape,
            },
            CursorShape::Underline => CursorGeometry {
                bounds: Bounds::new(
                    point(
                        cell.origin.x,
                        cell.origin.y + cell.size.height - underline_height(metrics.height),
                    ),
                    size(cell.size.width, underline_height(metrics.height)),
                ),
                shape: self.shape,
            },
        }
    }
}

/// The pixel rect for a cursor shape plus the shape itself (paint uses the
/// shape to choose fill vs. outline).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CursorGeometry {
    pub bounds: Bounds<Pixels>,
    pub shape: CursorShape,
}

/// Compute the rendering rect for a cursor at its frame position, at grid
/// origin. The paint origin is added by the element.
///
/// - `Block`: the full cell.
/// - `Bar`: the cell height, one eighth of the cell width (at least 1px),
///   flush left.
/// - `Underline`: the cell width, one eighth of the cell height (at least
///   1px), flush bottom.
/// - `HollowBlock`: the full cell (painted as an outline).
pub fn cursor_geometry(cursor: &CursorState, metrics: CellMetrics) -> CursorGeometry {
    CursorStateExt::from(cursor).geometry(cursor.col, cursor.row, metrics)
}

/// True only when the cursor is visible and blinking AND the frame requires a
/// repaint (`damage != Clean`) or the blink phase is active.
///
/// The two-argument form cannot observe the blink phase, so it treats a clean
/// frame as phase-inactive; the element uses
/// [`needs_blink_animation_with_phase`] with its sequence-derived phase.
pub fn needs_blink_animation(cursor: &CursorState, damage: &DamageKind) -> bool {
    needs_blink_animation_with_phase(cursor, damage, false)
}

/// [`needs_blink_animation`] with an explicit blink-phase argument.
pub fn needs_blink_animation_with_phase(
    cursor: &CursorState,
    damage: &DamageKind,
    phase_active: bool,
) -> bool {
    cursor.visible && cursor.blinking && (*damage != DamageKind::Clean || phase_active)
}

/// Whether another animation frame must be requested after painting `frame`.
///
/// True only while the cursor is visible and blinking. An idle terminal —
/// clean frame, non-blinking cursor — requests zero animation frames.
/// Damage-driven repaints and output-pump frames are the application's
/// responsibility, not the element's.
pub fn should_request_animation(frame: &FrameDelta) -> bool {
    frame.cursor.visible && frame.cursor.blinking
}

fn bar_width(cell_width: f32) -> Pixels {
    px((cell_width / 8.0).max(1.0))
}

fn underline_height(cell_height: f32) -> Pixels {
    px((cell_height / 8.0).max(1.0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::px;
    use mr_crabs_terminal::{FramePool, GridSize};

    const METRICS: CellMetrics = CellMetrics {
        width: 16.0,
        height: 32.0,
    };

    fn cursor(shape: CursorShape, blinking: bool, visible: bool) -> CursorState {
        CursorState {
            row: 1,
            col: 2,
            shape,
            blinking,
            visible,
            wrap_pending: false,
        }
    }

    fn frame(cursor: CursorState, damage: DamageKind, sequence: u64) -> FrameDelta {
        // FrameDelta's spare-row stash is crate-private to mr-crabs-terminal,
        // so the public construction path is FramePool::acquire.
        let mut pool = FramePool::new(1);
        let mut frame = pool.acquire(sequence, GridSize::new(4, 4));
        frame.damage = damage;
        frame.cursor = cursor;
        frame
    }

    #[test]
    fn blink_state_resets_on_activity_and_cursor_movement() {
        use std::time::Duration;

        let start = Instant::now();
        let mut state = BlinkState::default();
        let mut active = frame(cursor(CursorShape::Block, true, true), DamageKind::Clean, 1);

        assert!(state.phase_at(&active, start));
        assert!(!state.phase_at(&active, start + Duration::from_millis(500)));
        assert!(state.phase_at(&active, start + Duration::from_millis(1_000)));

        active.sequence += 1;
        assert!(state.phase_at(&active, start + Duration::from_millis(1_250)));
        assert!(!state.phase_at(&active, start + Duration::from_millis(1_750)));

        active.cursor.col += 1;
        assert!(state.phase_at(&active, start + Duration::from_millis(1_800)));
    }

    #[test]
    fn non_blinking_cursor_never_requests_animation() {
        let steady = frame(
            cursor(CursorShape::Block, false, true),
            DamageKind::Clean,
            1,
        );
        assert!(!should_request_animation(&steady));
        assert!(steady.cursor.visible);
    }

    #[test]
    fn block_covers_full_cell() {
        let geo = cursor_geometry(&cursor(CursorShape::Block, false, true), METRICS);
        assert_eq!(
            geo.bounds,
            Bounds::new(point(px(32.0), px(32.0)), size(px(16.0), px(32.0)))
        );
        assert_eq!(geo.shape, CursorShape::Block);
    }

    #[test]
    fn bar_is_left_flush_and_thin() {
        let geo = cursor_geometry(&cursor(CursorShape::Bar, false, true), METRICS);
        assert_eq!(
            geo.bounds,
            Bounds::new(point(px(32.0), px(32.0)), size(px(2.0), px(32.0)))
        );
    }

    #[test]
    fn bar_never_zero_width() {
        let geo = cursor_geometry(
            &cursor(CursorShape::Bar, false, true),
            CellMetrics {
                width: 1.0,
                height: 32.0,
            },
        );
        assert_eq!(f32::from(geo.bounds.size.width), 1.0);
    }

    #[test]
    fn underline_is_bottom_flush_and_thin() {
        let geo = cursor_geometry(&cursor(CursorShape::Underline, false, true), METRICS);
        assert_eq!(
            geo.bounds,
            Bounds::new(
                point(px(32.0), px(32.0 + 32.0 - 4.0)),
                size(px(16.0), px(4.0))
            )
        );
    }

    #[test]
    fn underline_never_zero_height() {
        // One-pixel-tall cells still produce a 1px underline, flush bottom.
        let geo = cursor_geometry(
            &cursor(CursorShape::Underline, false, true),
            CellMetrics {
                width: 16.0,
                height: 1.0,
            },
        );
        assert_eq!(
            geo.bounds,
            Bounds::new(point(px(32.0), px(1.0)), size(px(16.0), px(1.0)))
        );
    }

    #[test]
    fn hollow_block_covers_full_cell() {
        let geo = cursor_geometry(&cursor(CursorShape::HollowBlock, false, true), METRICS);
        assert_eq!(
            geo.bounds,
            Bounds::new(point(px(32.0), px(32.0)), size(px(16.0), px(32.0)))
        );
        assert_eq!(geo.shape, CursorShape::HollowBlock);
    }

    #[test]
    fn paint_bounds_compose_grid_cell_with_content_origin_once() {
        // Mirrors the element's paint expression
        // `cursor_geometry(cursor, self.metrics).bounds + origin`
        // (element.rs): the cursor rect is the grid-positioned cell
        // translated by the element's content origin exactly once. A
        // nonzero row/column plus a nonzero origin pin the prompt-end
        // placement contract — the cursor at (row, col) must land on that
        // cell in window pixels, never shifted by a second viewport or
        // padding offset.
        let origin = point(px(40.0), px(60.0));
        let state = CursorState {
            row: 7,
            col: 12,
            shape: CursorShape::Block,
            blinking: false,
            visible: true,
            wrap_pending: false,
        };
        let rect = cursor_geometry(&state, METRICS).bounds + origin;
        assert_eq!(
            rect,
            Bounds::new(
                point(
                    px(40.0 + 12.0 * METRICS.width),
                    px(60.0 + 7.0 * METRICS.height)
                ),
                size(px(METRICS.width), px(METRICS.height)),
            )
        );
        // The same cell at the zero origin keeps its size: only the origin
        // translation differs, so no offset is applied twice.
        assert_eq!(
            cursor_geometry(&state, METRICS).bounds,
            Bounds::new(point(px(192.0), px(224.0)), size(px(16.0), px(32.0)))
        );

        // Wrap-pending: terminal semantics keep the cursor on the reported
        // last column until the next character consumes the wrap; paint
        // must not shift it to the next line.
        let wrapping = CursorState {
            wrap_pending: true,
            ..state
        };
        assert_eq!(
            cursor_geometry(&wrapping, METRICS).bounds + origin,
            rect,
            "wrap_pending does not move the painted cursor"
        );
    }

    #[test]
    fn cursor_state_ext_maps_flags() {
        let state = cursor(CursorShape::Underline, true, false);
        let ext = CursorStateExt::from(&state);
        assert_eq!(
            ext,
            CursorStateExt {
                shape: CursorShape::Underline,
                blinking: true,
                visible: false,
                wrap_pending: false
            }
        );
        assert_eq!(
            ext.geometry(2, 1, METRICS),
            cursor_geometry(&state, METRICS)
        );
    }

    #[test]
    fn cursor_state_ext_carries_wrap_pending() {
        let state = CursorState {
            row: 0,
            col: 0,
            shape: CursorShape::Bar,
            blinking: false,
            visible: true,
            wrap_pending: true,
        };
        let ext = CursorStateExt::from(&state);
        assert!(ext.wrap_pending);
        // Geometry is identical either way; wrap_pending only informs the
        // DECAWM column decision.
        assert_eq!(
            ext.geometry(0, 0, METRICS),
            cursor_geometry(&state, METRICS)
        );
    }

    #[test]
    fn blink_phase_alternates_every_half_period() {
        assert!(blink_phase_active(0));
        assert!(blink_phase_active(29));
        assert!(!blink_phase_active(30));
        assert!(!blink_phase_active(59));
        assert!(blink_phase_active(60));
        assert!(blink_phase_active(120));
        assert!(!blink_phase_active(90));
    }

    #[test]
    fn needs_blink_animation_requires_visible_and_blinking() {
        let mut c = cursor(CursorShape::Block, true, true);
        // Dirty frame: animation needed regardless of phase.
        assert!(needs_blink_animation(&c, &DamageKind::Partial));
        assert!(needs_blink_animation(&c, &DamageKind::Full));
        // Clean frame without a known phase: not needed.
        assert!(!needs_blink_animation(&c, &DamageKind::Clean));

        // Phase-active clean frame: needed.
        assert!(needs_blink_animation_with_phase(
            &c,
            &DamageKind::Clean,
            true
        ));
        assert!(!needs_blink_animation_with_phase(
            &c,
            &DamageKind::Clean,
            false
        ));
        assert!(needs_blink_animation_with_phase(
            &c,
            &DamageKind::Partial,
            false
        ));

        c.visible = false;
        assert!(!needs_blink_animation_with_phase(
            &c,
            &DamageKind::Partial,
            true
        ));
        c.visible = true;
        c.blinking = false;
        assert!(!needs_blink_animation_with_phase(
            &c,
            &DamageKind::Partial,
            true
        ));
        assert!(!needs_blink_animation(&c, &DamageKind::Partial));
    }

    #[test]
    fn should_request_animation_idle_returns_false() {
        let idle = frame(
            cursor(CursorShape::Block, false, true),
            DamageKind::Clean,
            5,
        );
        assert!(!should_request_animation(&idle));
        // Invisible non-blinking cursor is equally idle.
        let hidden = frame(
            cursor(CursorShape::Block, false, false),
            DamageKind::Clean,
            5,
        );
        assert!(!should_request_animation(&hidden));
        // Dirty frames are repaints driven by the application pump, not
        // element animation frames.
        let dirty = frame(
            cursor(CursorShape::Block, false, true),
            DamageKind::Partial,
            5,
        );
        assert!(!should_request_animation(&dirty));
    }

    #[test]
    fn should_request_animation_blinking_only() {
        let blinking = frame(cursor(CursorShape::Block, true, true), DamageKind::Clean, 5);
        assert!(should_request_animation(&blinking));
        // Blinking is only meaningful for a visible cursor.
        let invisible = frame(
            cursor(CursorShape::Block, true, false),
            DamageKind::Clean,
            5,
        );
        assert!(!should_request_animation(&invisible));
        // A dirty frame with a blinking cursor still requests frames.
        let blinking_dirty = frame(
            cursor(CursorShape::Block, true, true),
            DamageKind::Partial,
            5,
        );
        assert!(should_request_animation(&blinking_dirty));
    }

    #[test]
    fn paint_cursor_matches_prompt_end() {
        // The prompt "~> " leaves the block cursor at column 3 of the prompt
        // row. The painted cursor rect (cell geometry + content origin) must
        // land exactly on that terminal cell in window pixels — the same
        // cell-origin math as glyphs, selection, and backgrounds, with the
        // padded content origin applied exactly once.
        let origin = point(px(30.0), px(50.0));
        let state = CursorState {
            row: 2,
            col: 3,
            shape: CursorShape::Block,
            blinking: false,
            visible: true,
            wrap_pending: false,
        };
        let painted = cursor_geometry(&state, METRICS).bounds + origin;
        assert_eq!(painted, cell_bounds(origin, 3, 2, METRICS));
        assert_eq!(
            painted.origin,
            point(
                px(30.0 + 3.0 * METRICS.width),
                px(50.0 + 2.0 * METRICS.height)
            )
        );
        assert_eq!(painted.size, size(px(METRICS.width), px(METRICS.height)));
    }
}

//! The cursor glow/trail effect: shape-aware cursor rectangles and the
//! linear leftover fade.
//!
//! Port of the oracle's cursor-trail contract
//! (`verification/manifests/dirty-oracle-v2.patch`):
//!
//! * `src/renderer/shaders/cursor-trail.glsl:11-27` — defaults: enabled,
//!   opacity 0.35, duration 250 ms.
//! * `cursor-trail.glsl:53-88` — a soft glow around the leftover cursor
//!   rectangle (`exp(-d / radius)` with `radius = 0.5 * max(w, h)`) plus a
//!   trail along the segment connecting the leftover and current rectangle
//!   centers, blended with `fade * opacity` where `fade = 1 - elapsed /
//!   duration` (linear); nothing is drawn when the cursor is hidden, the
//!   surface is unfocused, or the rectangle is degenerate.
//! * `src/renderer/generic.zig` (committed cursor-change plumbing): the
//!   previous rect is captured and the change time reset whenever the
//!   cursor rect changes.
//!
//! Coordinates are grid-relative pixels (top-left origin, y down); alpha,
//! radius, and segment geometry are origin-independent, so the renderer
//! adds its own paint origin.

use mr_crabs_terminal::{CursorShape, CursorState};

use crate::coords::CellPx;

/// A pixel point.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct PointPx {
    pub x: f64,
    pub y: f64,
}

impl PointPx {
    pub const fn new(x: f64, y: f64) -> Self {
        Self { x, y }
    }
}

/// An axis-aligned pixel rectangle (`x`, `y` are the top-left corner).
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct RectPx {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl RectPx {
    pub const fn new(x: f64, y: f64, w: f64, h: f64) -> Self {
        Self { x, y, w, h }
    }

    /// True when the rectangle has no area (the oracle shader's
    /// degenerate-rectangle guard: `current.z <= 0 || current.w <= 0`).
    pub const fn degenerate(self) -> bool {
        self.w <= 0.0 || self.h <= 0.0
    }

    pub const fn center(self) -> PointPx {
        PointPx::new(self.x + 0.5 * self.w, self.y + 0.5 * self.h)
    }
}

/// A trail segment between two points (the previous and current cursor
/// rectangle centers).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LinePx {
    pub from: PointPx,
    pub to: PointPx,
}

impl LinePx {
    pub const fn new(from: PointPx, to: PointPx) -> Self {
        Self { from, to }
    }
}

/// The cursor rectangle for a cursor state, in grid-relative pixels.
///
/// Mirrors the S4 element geometry rules: `Block`/`HollowBlock` occupy the
/// full cell, `Bar` is one eighth of the cell width (at least 1 px) flush
/// left, `Underline` is one eighth of the cell height (at least 1 px) flush
/// bottom.
pub fn cursor_rect(cursor: &CursorState, cell: CellPx) -> RectPx {
    let x = f64::from(cursor.col) * cell.width;
    let y = f64::from(cursor.row) * cell.height;
    match cursor.shape {
        CursorShape::Block | CursorShape::HollowBlock => RectPx::new(x, y, cell.width, cell.height),
        CursorShape::Bar => {
            let w = (cell.width / 8.0).max(1.0);
            RectPx::new(x, y, w, cell.height)
        }
        CursorShape::Underline => {
            let h = (cell.height / 8.0).max(1.0);
            RectPx::new(x, y + cell.height - h, cell.width, h)
        }
    }
}

/// Trail configuration (clamped by [`crate::EffectsConfig`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrailConfig {
    pub enabled: bool,
    pub opacity: f64,
    pub duration_ms: u64,
}

impl TrailConfig {
    pub const fn new(enabled: bool, opacity: f64, duration_ms: u64) -> Self {
        Self {
            enabled,
            opacity,
            duration_ms,
        }
    }
}

/// One frame of trail state for the renderer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrailFrame {
    /// True when a vacated leftover cell should draw: enabled, focused,
    /// cursor visible, leftover present, and still inside the fade window.
    pub active: bool,
    /// Milliseconds since the last cursor change.
    pub elapsed_ms: f64,
    /// The blend alpha: `(1 - elapsed / duration) * opacity`, 0 when
    /// inactive.
    pub alpha: f64,
    /// The glow falloff radius: `0.5 * max(w, h)` of the leftover rect.
    pub radius_px: f64,
    /// The vacated cursor rectangle. `None` before the first move and
    /// whenever the trail is not drawing.
    pub leftover_rect: Option<RectPx>,
    /// The segment between the leftover and current cursor centers; `None`
    /// until the cursor has moved at least once.
    pub segment: Option<LinePx>,
}

impl Default for TrailFrame {
    fn default() -> Self {
        Self {
            active: false,
            elapsed_ms: 0.0,
            alpha: 0.0,
            radius_px: 0.0,
            leftover_rect: None,
            segment: None,
        }
    }
}


/// The cursor trail state machine.
#[derive(Clone, Debug, PartialEq)]
pub struct CursorTrail {
    config: TrailConfig,
    current: Option<RectPx>,
    previous: Option<RectPx>,
    change_ms: f64,
    last_rect: Option<RectPx>,
}

impl CursorTrail {
    pub const fn new(config: TrailConfig) -> Self {
        Self {
            config,
            current: None,
            previous: None,
            change_ms: 0.0,
            last_rect: None,
        }
    }

    /// Replace the trail configuration, keeping the retained geometry.
    /// Disabling resets the state so the disabled path retains nothing.
    pub fn set_config(&mut self, config: TrailConfig) {
        if !config.enabled {
            *self = Self::new(config);
            return;
        }
        self.config = config;
    }

    pub const fn config(&self) -> TrailConfig {
        self.config
    }

    /// Geometry is stack-sized; the trail retains no heap.
    pub const fn retained_capacity(&self) -> usize {
        0
    }

    /// Advance the trail to a frame: track cursor movement and compute the
    /// leftover fade. The cursor rect is tracked regardless of
    /// visibility/focus (matching the oracle, which updates
    /// `iPreviousCursor`/`iTimeCursorChange` on every rect change);
    /// drawing is gated by `active` and requires a leftover cell.
    pub fn frame(&mut self, rect: RectPx, visible: bool, now_ms: f64, focus: bool) -> TrailFrame {
        if self.last_rect != Some(rect) {
            self.previous = self.current;
            self.current = Some(rect);
            self.change_ms = now_ms;
            self.last_rect = Some(rect);
        }

        let mut frame = TrailFrame::default();
        let Some(current) = self.current else {
            return frame;
        };
        if current.degenerate() {
            return frame;
        }
        let leftover = self.previous.filter(|r| !r.degenerate());
        let Some(leftover) = leftover else {
            return frame;
        };
        let elapsed = (now_ms - self.change_ms).max(0.0);
        if !self.config.enabled || !focus || !visible {
            return frame;
        }
        if elapsed >= self.config.duration_ms as f64 {
            return frame;
        }
        frame.active = true;
        frame.elapsed_ms = elapsed;
        frame.alpha = (1.0 - elapsed / self.config.duration_ms as f64) * self.config.opacity;
        frame.leftover_rect = Some(leftover);
        frame.radius_px = 0.5 * leftover.w.max(leftover.h);
        frame.segment = Some(LinePx::new(leftover.center(), current.center()));
        frame
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> TrailConfig {
        TrailConfig::new(true, 0.35, 250)
    }

    fn cursor(row: u16, col: u16) -> CursorState {
        CursorState {
            row,
            col,
            ..CursorState::default()
        }
    }

    #[test]
    fn block_cursor_rect_is_the_full_cell() {
        let c = cursor(2, 3);
        assert_eq!(
            cursor_rect(&c, CellPx::new(10.0, 20.0)),
            RectPx::new(30.0, 40.0, 10.0, 20.0)
        );
    }

    #[test]
    fn bar_and_underline_rects_follow_shape_rules() {
        let mut c = cursor(1, 4);
        c.shape = CursorShape::Bar;
        assert_eq!(
            cursor_rect(&c, CellPx::new(10.0, 20.0)),
            RectPx::new(40.0, 20.0, 1.25, 20.0)
        );
        c.shape = CursorShape::Underline;
        assert_eq!(
            cursor_rect(&c, CellPx::new(10.0, 20.0)),
            RectPx::new(40.0, 37.5, 10.0, 2.5)
        );
    }

    #[test]
    fn first_frame_has_no_leftover() {
        let mut t = CursorTrail::new(config());
        let f = t.frame(RectPx::new(0.0, 0.0, 10.0, 20.0), true, 2000.0, true);
        assert!(!f.active);
        assert_eq!(f.alpha, 0.0);
        assert_eq!(f.leftover_rect, None);
        assert_eq!(f.segment, None);
        assert_eq!(f.radius_px, 0.0);
    }

    #[test]
    fn move_captures_previous_rect_and_resets_fade() {
        let mut t = CursorTrail::new(config());
        _ = t.frame(RectPx::new(0.0, 0.0, 10.0, 20.0), true, 2000.0, true);
        let f = t.frame(RectPx::new(50.0, 0.0, 10.0, 20.0), true, 2016.0, true);
        assert!(f.active);
        assert_eq!(f.elapsed_ms, 0.0);
        assert_eq!(f.alpha, 0.35);
        assert_eq!(f.leftover_rect, Some(RectPx::new(0.0, 0.0, 10.0, 20.0)));
        assert_eq!(f.radius_px, 10.0);
        assert_eq!(
            f.segment,
            Some(LinePx::new(
                PointPx::new(5.0, 10.0),
                PointPx::new(55.0, 10.0)
            ))
        );
    }

    #[test]
    fn fade_is_linear_and_expires() {
        let mut t = CursorTrail::new(config());
        _ = t.frame(RectPx::new(0.0, 0.0, 10.0, 20.0), true, 2000.0, true);
        _ = t.frame(RectPx::new(50.0, 0.0, 10.0, 20.0), true, 2016.0, true);
        let mid = t.frame(RectPx::new(50.0, 0.0, 10.0, 20.0), true, 2141.0, true);
        assert!(mid.active);
        assert_eq!(mid.elapsed_ms, 125.0);
        assert_eq!(mid.alpha, 0.175);
        assert_eq!(mid.leftover_rect, Some(RectPx::new(0.0, 0.0, 10.0, 20.0)));
        let end = t.frame(RectPx::new(50.0, 0.0, 10.0, 20.0), true, 2266.0, true);
        assert!(!end.active);
        assert_eq!(end.alpha, 0.0);
        assert_eq!(end.leftover_rect, None);
    }

    #[test]
    fn hidden_or_unfocused_or_disabled_draws_nothing() {
        let mut t = CursorTrail::new(config());
        let first = RectPx::new(0.0, 0.0, 10.0, 20.0);
        let next = RectPx::new(50.0, 0.0, 10.0, 20.0);
        _ = t.frame(first, true, 2000.0, true);
        _ = t.frame(next, true, 2016.0, true);
        assert!(!t.frame(next, false, 2016.0, true).active);
        assert!(!t.frame(next, true, 2016.0, false).active);
        t.set_config(TrailConfig::new(false, 0.35, 250));
        assert!(!t.frame(next, true, 2016.0, true).active);
        assert_eq!(t.retained_capacity(), 0);
    }

    #[test]
    fn degenerate_rect_draws_nothing() {
        let mut t = CursorTrail::new(config());
        let f = t.frame(RectPx::new(0.0, 0.0, 0.0, 20.0), true, 2000.0, true);
        assert!(!f.active);
    }

    #[test]
    fn leftover_fade_survives_without_gradient_id() {
        let mut t = CursorTrail::new(config());
        _ = t.frame(RectPx::new(0.0, 0.0, 10.0, 20.0), true, 2000.0, true);
        let f = t.frame(RectPx::new(50.0, 0.0, 10.0, 20.0), true, 2016.0, true);
        assert!(f.active);
        assert_eq!(f.alpha, 0.35);
        assert_eq!(f.radius_px, 10.0);
        assert_eq!(f.leftover_rect, Some(RectPx::new(0.0, 0.0, 10.0, 20.0)));
        assert_eq!(
            f.segment,
            Some(LinePx::new(
                PointPx::new(5.0, 10.0),
                PointPx::new(55.0, 10.0)
            ))
        );
    }
}

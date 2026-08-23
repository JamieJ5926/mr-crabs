//! The cursor glow/trail effect: shape-aware cursor rectangles, the linear
//! fade, and the bounded gradient-resource descriptor cache.
//!
//! Port of the oracle's cursor-trail contract
//! (`verification/manifests/dirty-oracle-v2.patch`):
//!
//! * `src/renderer/shaders/cursor-trail.glsl:11-27` — defaults: enabled,
//!   opacity 0.35, duration 250 ms.
//! * `cursor-trail.glsl:53-88` — a soft glow around the current cursor
//!   rectangle (`exp(-d / radius)` with `radius = 0.5 * max(w, h)`) plus a
//!   trail along the segment connecting the previous and current rectangle
//!   centers, blended with `fade * opacity` where `fade = 1 - elapsed /
//!   duration` (linear); nothing is drawn when the cursor is hidden, the
//!   surface is unfocused, or the rectangle is degenerate.
//! * `src/renderer/generic.zig` (committed cursor-change plumbing): the
//!   previous rect is captured and the change time reset whenever the
//!   cursor rect changes.
//!
//! The gradient resource descriptor comes from a bounded LRU cache keyed
//! by the quantized glow radius, so a renderer caches one radial-gradient
//! texture per radius bucket and reuses it across frames (the same
//! `GradientId` is returned while the radius bucket is unchanged).
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

const ECHO_COUNT: usize = 3;
const ECHO_BASE_POSITIONS: [f64; ECHO_COUNT] = [0.15, 0.45, 0.75];
const ECHO_ALPHA_WEIGHTS: [f64; ECHO_COUNT] = [0.22, 0.45, 0.72];

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct TrailEcho {
    pub rect: RectPx,
    pub alpha: f64,
}

fn lerp_rect(a: RectPx, b: RectPx, t: f64) -> RectPx {
    RectPx::new(
        a.x + (b.x - a.x) * t,
        a.y + (b.y - a.y) * t,
        a.w + (b.w - a.w) * t,
        a.h + (b.h - a.h) * t,
    )
}

/// One frame of trail state for the renderer.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TrailFrame {
    /// True when the effect should draw: enabled, focused, cursor visible,
    /// current rectangle valid, and still inside the fade window.
    pub active: bool,
    /// Milliseconds since the last cursor change.
    pub elapsed_ms: f64,
    /// The blend alpha: `(1 - elapsed / duration) * opacity`, 0 when
    /// inactive.
    pub alpha: f64,
    /// The glow falloff radius: `0.5 * max(w, h)` of the current rect.
    pub radius_px: f64,
    /// The current cursor rectangle (glow anchor).
    pub glow_rect: RectPx,
    /// The segment between the previous and current cursor centers; `None`
    /// until the cursor has moved at least once.
    pub segment: Option<LinePx>,
    /// Shape-aware echoes collapsing into the live cursor.
    pub echoes: [TrailEcho; ECHO_COUNT],
    /// The cached gradient descriptor for `radius_px`.
    pub gradient: GradientId,
}

impl Default for TrailFrame {
    fn default() -> Self {
        Self {
            active: false,
            elapsed_ms: 0.0,
            alpha: 0.0,
            radius_px: 0.0,
            glow_rect: RectPx::default(),
            segment: None,
            echoes: [TrailEcho::default(); ECHO_COUNT],
            gradient: GradientId(0),
        }
    }
}

/// A stable descriptor for a cached radial-gradient resource. Equal ids
/// denote the same cached gradient (same radius bucket); ids are only
/// reused for live cache entries, so a renderer may key its gradient
/// texture by this id safely.
#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
pub struct GradientId(pub u32);

/// The maximum number of gradient descriptors retained.
pub const MAX_GRADIENTS: usize = 16;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct GradientEntry {
    bucket: u64,
    id: u32,
    last_use: u64,
}

/// Bounded LRU cache of gradient resource descriptors keyed by the
/// quantized glow radius (0.5 px buckets). Never grows beyond
/// [`MAX_GRADIENTS`] entries; on overflow the least-recently-used entry is
/// evicted and its slot reused with a fresh id, so stale renderer caches
/// are invalidated deterministically.
#[derive(Clone, Debug, PartialEq)]
pub struct GradientCache {
    entries: Vec<GradientEntry>,
    next_id: u32,
    clock: u64,
}

impl GradientCache {
    pub const fn new() -> Self {
        Self {
            entries: Vec::new(),
            next_id: 0,
            clock: 0,
        }
    }

    /// Resolve the gradient descriptor for a glow radius, touching the
    /// entry's recency. A radius bucket of `ceil(radius * 2)` px.
    pub fn get(&mut self, radius_px: f64) -> GradientId {
        let bucket = (radius_px * 2.0).ceil().max(0.0) as u64;
        self.clock += 1;
        if let Some(entry) = self.entries.iter_mut().find(|e| e.bucket == bucket) {
            entry.last_use = self.clock;
            return GradientId(entry.id);
        }
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        if self.entries.len() < MAX_GRADIENTS {
            self.entries.push(GradientEntry {
                bucket,
                id,
                last_use: self.clock,
            });
        } else {
            let lru = self
                .entries
                .iter()
                .enumerate()
                .min_by_key(|(_, e)| e.last_use)
                .map(|(i, _)| i)
                .expect("non-empty at MAX_GRADIENTS");
            self.entries[lru] = GradientEntry {
                bucket,
                id,
                last_use: self.clock,
            };
        }
        GradientId(id)
    }

    /// The number of retained descriptors.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Retained heap bytes.
    pub fn retained_capacity(&self) -> usize {
        self.entries.capacity() * std::mem::size_of::<GradientEntry>()
    }
}

impl Default for GradientCache {
    fn default() -> Self {
        Self::new()
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
    gradient: GradientCache,
}

impl CursorTrail {
    pub const fn new(config: TrailConfig) -> Self {
        Self {
            config,
            current: None,
            previous: None,
            change_ms: 0.0,
            last_rect: None,
            gradient: GradientCache::new(),
        }
    }

    /// Replace the trail configuration, keeping the retained geometry and
    /// gradient descriptors (changing opacity/duration does not invalidate
    /// radius buckets). Disabling resets the state so the disabled path
    /// retains nothing.
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

    pub fn gradient_cache(&self) -> &GradientCache {
        &self.gradient
    }

    /// Retained heap bytes (the gradient descriptor cache only; geometry
    /// is stack-sized).
    pub fn retained_capacity(&self) -> usize {
        self.gradient.retained_capacity()
    }

    /// Advance the trail to a frame: track cursor movement, compute the
    /// fade, and resolve the gradient descriptor. The cursor rect is
    /// tracked regardless of visibility/focus (matching the oracle, which
    /// updates `iPreviousCursor`/`iTimeCursorChange` on every rect
    /// change); drawing is gated by `active`.
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
        frame.radius_px = 0.5 * current.w.max(current.h);
        frame.glow_rect = current;
        frame.segment = self
            .previous
            .map(|prev| LinePx::new(prev.center(), current.center()));
        if let Some(prev) = self.previous {
            let duration = self.config.duration_ms as f64;
            let p = if duration > 0.0 {
                (elapsed / duration).clamp(0.0, 1.0)
            } else {
                1.0
            };
            let settle = 1.0 - (1.0 - p) * (1.0 - p);
            for i in 0..ECHO_COUNT {
                let u = ECHO_BASE_POSITIONS[i] + (1.0 - ECHO_BASE_POSITIONS[i]) * settle;
                let rect = lerp_rect(prev, current, u);
                frame.echoes[i] = TrailEcho {
                    rect,
                    alpha: frame.alpha * ECHO_ALPHA_WEIGHTS[i],
                };
            }
        }
        frame.gradient = self.gradient.get(frame.radius_px);
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
    fn first_frame_glows_without_segment() {
        let mut t = CursorTrail::new(config());
        let f = t.frame(RectPx::new(0.0, 0.0, 10.0, 20.0), true, 2000.0, true);
        assert!(f.active);
        assert_eq!(f.elapsed_ms, 0.0);
        assert_eq!(f.alpha, 0.35);
        assert_eq!(f.radius_px, 10.0);
        assert_eq!(f.glow_rect, RectPx::new(0.0, 0.0, 10.0, 20.0));
        assert_eq!(f.segment, None);
        assert_eq!(f.gradient, GradientId(0));
    }

    #[test]
    fn move_captures_previous_rect_and_resets_fade() {
        let mut t = CursorTrail::new(config());
        _ = t.frame(RectPx::new(0.0, 0.0, 10.0, 20.0), true, 2000.0, true);
        let f = t.frame(RectPx::new(50.0, 0.0, 10.0, 20.0), true, 2016.0, true);
        assert!(f.active);
        assert_eq!(f.elapsed_ms, 0.0); // fade restarted at the move
        assert_eq!(f.alpha, 0.35);
        assert_eq!(f.glow_rect, RectPx::new(50.0, 0.0, 10.0, 20.0));
        assert_eq!(
            f.segment,
            Some(LinePx::new(
                PointPx::new(5.0, 10.0),
                PointPx::new(55.0, 10.0)
            ))
        );
        // Same radius bucket: the descriptor is reused.
        assert_eq!(f.gradient, GradientId(0));
    }

    #[test]
    fn fade_is_linear_and_expires() {
        let mut t = CursorTrail::new(config());
        _ = t.frame(RectPx::new(0.0, 0.0, 10.0, 20.0), true, 2000.0, true);
        let mid = t.frame(RectPx::new(0.0, 0.0, 10.0, 20.0), true, 2125.0, true);
        assert!(mid.active);
        assert_eq!(mid.elapsed_ms, 125.0);
        assert_eq!(mid.alpha, 0.175); // (1 - 125/250) * 0.35
        assert_eq!(mid.gradient, GradientId(0));
        let end = t.frame(RectPx::new(0.0, 0.0, 10.0, 20.0), true, 2250.0, true);
        assert!(!end.active);
        assert_eq!(end.alpha, 0.0);
    }

    #[test]
    fn hidden_or_unfocused_or_disabled_draws_nothing() {
        let mut t = CursorTrail::new(config());
        let rect = RectPx::new(0.0, 0.0, 10.0, 20.0);
        _ = t.frame(rect, true, 2000.0, true);
        assert!(!t.frame(rect, false, 2016.0, true).active);
        assert!(!t.frame(rect, true, 2016.0, false).active);
        t.set_config(TrailConfig::new(false, 0.35, 250));
        assert!(!t.frame(rect, true, 2016.0, true).active);
        assert!(t.gradient_cache().is_empty());
        assert_eq!(t.retained_capacity(), 0);
    }

    #[test]
    fn degenerate_rect_draws_nothing() {
        let mut t = CursorTrail::new(config());
        let f = t.frame(RectPx::new(0.0, 0.0, 0.0, 20.0), true, 2000.0, true);
        assert!(!f.active);
    }

    #[test]
    fn gradient_cache_is_bounded_and_evicts_lru() {
        let mut t = CursorTrail::new(config());
        // 16 distinct radius buckets fill the cache: vary the rect height
        // (radius = 0.5 * max(w, h)).
        let mut first = GradientId(0);
        for i in 0..MAX_GRADIENTS {
            let h = 20.0 + f64::from(i as u32) * 2.0;
            let f = t.frame(RectPx::new(0.0, 0.0, 10.0, h), true, 2000.0, true);
            if i == 0 {
                first = f.gradient;
            }
            assert_eq!(t.gradient_cache().len(), i + 1);
        }
        assert_eq!(t.gradient_cache().len(), MAX_GRADIENTS);
        // Touching bucket 0 again reuses its id (cache hit).
        let f = t.frame(RectPx::new(0.0, 0.0, 10.0, 20.0), true, 2000.0, true);
        assert_eq!(f.gradient, first);
        assert_eq!(t.gradient_cache().len(), MAX_GRADIENTS);
        // A new bucket evicts the least-recently-used entry with a fresh id.
        let f = t.frame(
            RectPx::new(0.0, 0.0, 10.0, 20.0 + 2.0 * 32.0),
            true,
            2000.0,
            true,
        );
        assert_ne!(f.gradient, first);
        assert_eq!(t.gradient_cache().len(), MAX_GRADIENTS);
    }
}

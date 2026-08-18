//! S9: Mr Crabs built-in animation effects as a deterministic headless model.
//!
//! This crate ports the frozen Mr Crabs visual-effect contract from
//! `verification/manifests/dirty-oracle-v2.patch` (the oracle's
//! `src/renderer/shaders/{streaming-text,typewriter-text,cursor-trail}.glsl`
//! and the `src/renderer/generic.zig` additions) into renderer-independent
//! Rust:
//!
//! * **Streaming text reveal** (`text-animation=streaming`, opt-in): a
//!   per-cell change-time store records when each cell's rendered content
//!   last changed; the reveal sweeps left-to-right over the configured
//!   duration (default 120 ms) and only cells that actually changed are
//!   ever concealed — unchanged cells carry a sentinel and pass through.
//! * **Typewriter reveal** (`text-animation=typewriter`): changed cells are
//!   timestamped one eighth of the duration later than the previous changed
//!   cell in reading order, via a persistent bounded burst schedule that
//!   sequences adjacent rebuilds; cells whose timestamp has not arrived yet
//!   stay fully concealed and keep animation frames scheduled.
//! * **Cursor trail** (`cursor-trail`, opt-in): a linear 250 ms
//!   fade at opacity 0.35 with a soft glow around the current cursor rect
//!   and a segment connecting the previous and current rect centers. The
//!   gradient resource descriptors come from a bounded cache.
//! * **Disabled path**: with `text-animation=none` and `cursor-trail=false`
//!   the model retains zero allocations and never schedules a frame.
//!
//! The model is a pure consumer of [`FrameDelta`]s: it never mutates
//! terminal text, and its retained memory is exclusively numeric change
//! state (explicitly bounded by grid size and [`EffectsConfig::max_tracked_cells`]).
//!
//! All timestamps are expressed in milliseconds and driven by an explicit
//! clock passed to [`EffectsModel::apply_frame`], so frame sequences are
//! deterministic and byte-comparable against the corpus at
//! `rust/verification/effects-corpus/s9-effects.json`.

mod coords;
mod key;
mod model;
mod reveal;
mod schedule;
mod trail;

pub use coords::{CellMetricsUniform, CellPx, RowOrientation};
pub use key::ChangeTracker;
pub use model::{EffectsFrame, EffectsModel};
pub use reveal::{CellPos, CellReveal, RevealMath, RevealPhase};
pub use schedule::TypewriterSchedule;
pub use trail::{
    CursorTrail, GradientCache, GradientId, LinePx, PointPx, RectPx, TrailConfig, TrailFrame,
};

pub use mr_crabs_config::TextAnimation;

use mr_crabs_config::AnimationDefaults;

/// Minimum text-animation duration after clamping (oracle
/// `src/config/Config.zig: finalize`, `text-animation-duration` clamp).
pub const TEXT_ANIM_DURATION_MIN_MS: u64 = 1;
/// Maximum text-animation duration after clamping (5 s, oracle
/// `src/config/Config.zig: finalize`).
pub const TEXT_ANIM_DURATION_MAX_MS: u64 = 5_000;
/// Minimum cursor-trail duration after clamping (1 ms, oracle
/// `src/config/Config.zig: finalize`).
pub const TRAIL_DURATION_MIN_MS: u64 = 1;
/// Maximum cursor-trail duration after clamping (60 s, oracle
/// `src/config/Config.zig: finalize`).
pub const TRAIL_DURATION_MAX_MS: u64 = 60_000;
/// Default bound on the number of tracked cells. The change tracker is
/// dense (one entry per tracked grid cell), so every payload has an
/// explicit count bound: `min(cols * rows, max_tracked_cells)`.
pub const DEFAULT_MAX_TRACKED_CELLS: usize = 1 << 20;

/// Fully clamped animation configuration for one terminal surface.
///
/// All durations and intensities are clamped to the oracle's safe finite
/// ranges at construction (see the `*_MIN_MS`/`*_MAX_MS` constants), so the
/// animation window is always bounded. [`Default`] reproduces the exact
/// Mr Crabs defaults: streaming text reveal and cursor trail on, matching
/// the live product. Tuning values are 120 ms / 1.0 and 250 ms / 0.35.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EffectsConfig {
    /// Text reveal mode; `Disabled` allocates no text-animation state.
    pub text_animation: TextAnimation,
    /// Reveal window in milliseconds, clamped to `[1, 5000]`.
    pub text_animation_duration_ms: u64,
    /// Reveal intensity in `[0, 1]`; 0 shows newly changed glyphs at once.
    pub text_animation_intensity: f64,
    /// Whether the cursor glow/trail post effect is enabled.
    pub cursor_trail: bool,
    /// Trail opacity in `[0, 1]` (default 0.35).
    pub cursor_trail_opacity: f64,
    /// Trail fade duration in milliseconds, clamped to `[1, 60000]`.
    pub cursor_trail_duration_ms: u64,
    /// Upper bound on tracked cells; the change store never exceeds
    /// `min(cols * rows, max_tracked_cells)` entries.
    pub max_tracked_cells: usize,
}

impl EffectsConfig {
    /// Build a configuration, applying the oracle's finalize clamps.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        text_animation: TextAnimation,
        text_animation_duration_ms: u64,
        text_animation_intensity: f64,
        cursor_trail: bool,
        cursor_trail_opacity: f64,
        cursor_trail_duration_ms: u64,
        max_tracked_cells: usize,
    ) -> Self {
        Self {
            text_animation,
            text_animation_duration_ms: text_animation_duration_ms
                .clamp(TEXT_ANIM_DURATION_MIN_MS, TEXT_ANIM_DURATION_MAX_MS),
            text_animation_intensity: text_animation_intensity.clamp(0.0, 1.0),
            cursor_trail,
            cursor_trail_opacity: cursor_trail_opacity.clamp(0.0, 1.0),
            cursor_trail_duration_ms: cursor_trail_duration_ms
                .clamp(TRAIL_DURATION_MIN_MS, TRAIL_DURATION_MAX_MS),
            max_tracked_cells: max_tracked_cells.max(1),
        }
    }
}

impl Default for EffectsConfig {
    fn default() -> Self {
        Self::from(AnimationDefaults::default())
    }
}

impl From<AnimationDefaults> for EffectsConfig {
    /// Map the frozen `mr-crabs-config` defaults onto the clamped effects
    /// configuration.
    fn from(a: AnimationDefaults) -> Self {
        Self::new(
            a.text_animation,
            u64::try_from(a.text_animation_duration.as_millis()).unwrap_or(0),
            f64::from(a.text_animation_intensity),
            a.cursor_trail,
            f64::from(a.cursor_trail_opacity),
            u64::try_from(a.cursor_trail_duration.as_millis()).unwrap_or(0),
            DEFAULT_MAX_TRACKED_CELLS,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn defaults_match_mr_crabs() {
        let cfg = EffectsConfig::default();
        assert_eq!(cfg.text_animation, TextAnimation::Streaming);
        assert_eq!(cfg.text_animation_duration_ms, 120);
        assert_eq!(cfg.text_animation_intensity, 1.0);
        assert!(cfg.cursor_trail);
        assert_eq!(cfg.cursor_trail_duration_ms, 250);
        assert!((cfg.cursor_trail_opacity - 0.35).abs() < f64::from(f32::EPSILON));
    }

    #[test]
    fn config_clamps_to_oracle_ranges() {
        let cfg = EffectsConfig::new(TextAnimation::Streaming, 0, 2.5, true, -1.0, 0, 0);
        assert_eq!(cfg.text_animation_duration_ms, TEXT_ANIM_DURATION_MIN_MS);
        assert_eq!(cfg.text_animation_intensity, 1.0);
        assert_eq!(cfg.cursor_trail_opacity, 0.0);
        assert_eq!(cfg.cursor_trail_duration_ms, TRAIL_DURATION_MIN_MS);
        assert_eq!(cfg.max_tracked_cells, 1);

        let cfg = EffectsConfig::new(
            TextAnimation::Disabled,
            u64::MAX,
            -3.0,
            true,
            7.0,
            u64::MAX,
            usize::MAX,
        );
        assert_eq!(cfg.text_animation_duration_ms, TEXT_ANIM_DURATION_MAX_MS);
        assert_eq!(cfg.text_animation_intensity, 0.0);
        assert_eq!(cfg.cursor_trail_opacity, 1.0);
        assert_eq!(cfg.cursor_trail_duration_ms, TRAIL_DURATION_MAX_MS);
    }

    #[test]
    fn from_animation_defaults_maps_duration() {
        let a = AnimationDefaults {
            text_animation_duration: Duration::from_millis(77),
            ..AnimationDefaults::default()
        };
        let cfg = EffectsConfig::from(a);
        assert_eq!(cfg.text_animation_duration_ms, 77);
        assert_eq!(cfg.text_animation, TextAnimation::Streaming);
    }
}

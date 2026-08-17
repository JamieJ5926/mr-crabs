//! Deterministic reveal math for the built-in text animations.
//!
//! Port of the oracle shader math (`verification/manifests/dirty-oracle-v2.patch`):
//!
//! * `streaming-text.glsl:40-81` — reveal sweeps left-to-right over the
//!   configured duration; cells whose elapsed time is negative or past the
//!   duration pass through unchanged; concealment blends toward the
//!   terminal background by
//!   `intensity * (1 - reveal) * smoothstep(0, 2, dist)`.
//! * `typewriter-text.glsl:44-91` — each cell strikes in over the first
//!   quarter of its window; cells whose timestamp has not arrived yet are
//!   fully concealed (intensity), and once the strike completes the
//!   character stays visible for the rest of the window.
//!
//! The model is cell-level: it reports the reveal phase, the left-to-right
//! boundary as a fraction of the cell width, and the conceal fraction at an
//! arbitrary cell-local pixel position, so both the frame fixtures and a
//! renderer's pixel passes can be deterministic.

use crate::TextAnimation;

/// A grid position; `row` counts from the top of the grid, matching the
/// change-texture row convention.
#[derive(Clone, Copy, Debug, Default, Eq, Hash, PartialEq)]
pub struct CellPos {
    pub row: u16,
    pub col: u16,
}

impl CellPos {
    pub const fn new(row: u16, col: u16) -> Self {
        Self { row, col }
    }
}

/// The reveal state of one changed cell at one instant.
///
/// * `Pending` — the cell's change timestamp is in the future (typewriter
///   stagger): the glyph is fully concealed and animation frames must stay
///   scheduled until the timestamp arrives.
/// * `Animating` — the reveal is within its window; `boundary_fraction`
///   gives the left-to-right sweep position.
/// * `Revealed` — the window has elapsed; the cell passes through.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RevealPhase {
    #[default]
    Revealed,
    Pending,
    Animating,
}

/// The shader's conceal formulas, parameterized by the active mode,
/// duration, intensity, and cell width in pixels.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RevealMath {
    mode: TextAnimation,
    duration_ms: f64,
    intensity: f64,
    cell_width_px: f64,
}

impl RevealMath {
    pub fn new(mode: TextAnimation, duration_ms: u64, intensity: f64, cell_width_px: f64) -> Self {
        Self {
            mode,
            duration_ms: duration_ms as f64,
            intensity,
            cell_width_px,
        }
    }

    pub const fn mode(&self) -> TextAnimation {
        self.mode
    }

    pub const fn duration_ms(&self) -> f64 {
        self.duration_ms
    }

    pub const fn intensity(&self) -> f64 {
        self.intensity
    }

    pub const fn cell_width_px(&self) -> f64 {
        self.cell_width_px
    }

    /// The reveal phase for an elapsed time since the cell's change.
    pub fn phase(&self, elapsed_ms: f64) -> RevealPhase {
        match self.mode {
            TextAnimation::Disabled => RevealPhase::Revealed,
            TextAnimation::Streaming => {
                if elapsed_ms < 0.0 || elapsed_ms >= self.duration_ms {
                    RevealPhase::Revealed
                } else {
                    RevealPhase::Animating
                }
            }
            TextAnimation::Typewriter => {
                if elapsed_ms >= self.duration_ms {
                    RevealPhase::Revealed
                } else if elapsed_ms < 0.0 {
                    RevealPhase::Pending
                } else {
                    RevealPhase::Animating
                }
            }
        }
    }

    /// The left-to-right sweep position as a fraction of the cell width
    /// (0 = nothing revealed, 1 = fully swept). For a `Pending` cell the
    /// boundary is meaningless (the cell is fully concealed); callers
    /// should gate on [`RevealPhase`] first.
    pub fn boundary_fraction(&self, elapsed_ms: f64) -> f64 {
        match self.mode {
            TextAnimation::Disabled => 1.0,
            TextAnimation::Streaming => (elapsed_ms / self.duration_ms).clamp(0.0, 1.0),
            TextAnimation::Typewriter => {
                let strike_ms = self.duration_ms * 0.25;
                (elapsed_ms / strike_ms).clamp(0.0, 1.0)
            }
        }
    }

    /// The conceal fraction in `[0, 1]` at a cell-local pixel position
    /// (0 = fully visible, 1 = fully hidden toward the cell background).
    ///
    /// Exact port of the oracle formulas:
    /// * streaming: `intensity * (1 - reveal) * smoothstep(0, 2, dist)`
    ///   with `dist = local_x - reveal * cell_width`, zero for pixels left
    ///   of the boundary and outside the window;
    /// * typewriter: the same with the strike fraction `t =
    ///   clamp(elapsed / (duration / 4))`, and full `intensity`
    ///   concealment while the timestamp is in the future.
    pub fn hidden_fraction_at(&self, elapsed_ms: f64, local_px_x: f64) -> f64 {
        match self.mode {
            TextAnimation::Disabled => 0.0,
            TextAnimation::Streaming => {
                if elapsed_ms < 0.0 || elapsed_ms >= self.duration_ms {
                    return 0.0;
                }
                let reveal = elapsed_ms / self.duration_ms;
                let dist = local_px_x - reveal * self.cell_width_px;
                if dist <= 0.0 {
                    return 0.0;
                }
                self.intensity * (1.0 - reveal) * smoothstep_0_2(dist)
            }
            TextAnimation::Typewriter => {
                if elapsed_ms >= self.duration_ms {
                    return 0.0;
                }
                if elapsed_ms < 0.0 {
                    // Future timestamp: fully concealed at the intensity.
                    return self.intensity;
                }
                let strike_ms = self.duration_ms * 0.25;
                let t = (elapsed_ms / strike_ms).clamp(0.0, 1.0);
                let dist = local_px_x - t * self.cell_width_px;
                if dist <= 0.0 {
                    return 0.0;
                }
                self.intensity * (1.0 - t) * smoothstep_0_2(dist)
            }
        }
    }
}

/// GLSL `smoothstep(0.0, 2.0, d)` — the soft edge of the concealment
/// boundary, in pixels.
fn smoothstep_0_2(d: f64) -> f64 {
    if d <= 0.0 {
        0.0
    } else if d >= 2.0 {
        1.0
    } else {
        let x = d / 2.0;
        x * x * (3.0 - 2.0 * x)
    }
}

/// One changed cell inside its reveal window (or pending), as observed at a
/// frame instant. `change_ms` is the cell's change timestamp and
/// `elapsed_ms` the time since it; both are exact model values, so frames
/// are deterministic.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CellReveal {
    pub pos: CellPos,
    pub change_ms: f64,
    pub elapsed_ms: f64,
}

impl CellReveal {
    pub const fn new(pos: CellPos, change_ms: f64, elapsed_ms: f64) -> Self {
        Self {
            pos,
            change_ms,
            elapsed_ms,
        }
    }

    pub fn phase(&self, math: &RevealMath) -> RevealPhase {
        math.phase(self.elapsed_ms)
    }

    pub fn boundary_fraction(&self, math: &RevealMath) -> f64 {
        math.boundary_fraction(self.elapsed_ms)
    }

    pub fn hidden_fraction_at(&self, math: &RevealMath, local_px_x: f64) -> f64 {
        math.hidden_fraction_at(self.elapsed_ms, local_px_x)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn math(mode: TextAnimation) -> RevealMath {
        RevealMath::new(mode, 120, 1.0, 10.0)
    }

    #[test]
    fn streaming_phases_and_boundary() {
        let m = math(TextAnimation::Streaming);
        assert_eq!(m.phase(-1.0), RevealPhase::Revealed);
        assert_eq!(m.phase(0.0), RevealPhase::Animating);
        assert_eq!(m.phase(60.0), RevealPhase::Animating);
        assert_eq!(m.phase(120.0), RevealPhase::Revealed);
        assert_eq!(m.phase(500.0), RevealPhase::Revealed);
        assert_eq!(m.boundary_fraction(60.0), 0.5);
        assert_eq!(m.boundary_fraction(0.0), 0.0);
    }

    #[test]
    fn typewriter_future_is_pending_then_strikes_then_holds() {
        let m = math(TextAnimation::Typewriter);
        assert_eq!(m.phase(-30.0), RevealPhase::Pending);
        assert_eq!(m.phase(0.0), RevealPhase::Animating);
        assert_eq!(m.phase(30.0), RevealPhase::Animating); // strike complete
        assert_eq!(m.phase(119.0), RevealPhase::Animating); // holds
        assert_eq!(m.phase(120.0), RevealPhase::Revealed);
        // Boundary: quarter-of-window strike.
        assert_eq!(m.boundary_fraction(15.0), 0.5);
        assert_eq!(m.boundary_fraction(30.0), 1.0);
        assert_eq!(m.boundary_fraction(119.0), 1.0);
    }

    #[test]
    fn streaming_conceal_math_matches_shader() {
        let m = math(TextAnimation::Streaming);
        // At the change instant: boundary at 0, so every pixel is hidden.
        assert_eq!(m.hidden_fraction_at(0.0, 5.0), 1.0);
        // Mid-window: boundary at half the cell; right edge fully hidden
        // (smoothstep saturates beyond 2px), left of boundary revealed.
        assert_eq!(m.hidden_fraction_at(60.0, 9.0), 0.5);
        assert_eq!(m.hidden_fraction_at(60.0, 4.0), 0.0);
        // Soft edge inside the 2px falloff: dist 1px at 60ms.
        // reveal = 0.5, boundary = 5px, local_x = 6px -> dist = 1,
        // smoothstep(0,2,1) = 0.5, hidden = 0.5 * 0.5 = 0.25.
        assert_eq!(m.hidden_fraction_at(60.0, 6.0), 0.25);
        // Outside the window: revealed.
        assert_eq!(m.hidden_fraction_at(120.0, 9.0), 0.0);
        assert_eq!(m.hidden_fraction_at(-1.0, 9.0), 0.0);
    }

    #[test]
    fn typewriter_conceal_math_matches_shader() {
        let m = math(TextAnimation::Typewriter);
        // Future timestamp: fully concealed regardless of x.
        assert_eq!(m.hidden_fraction_at(-15.0, 0.5), 1.0);
        assert_eq!(m.hidden_fraction_at(-15.0, 9.5), 1.0);
        // At the strike start: everything hidden.
        assert_eq!(m.hidden_fraction_at(0.0, 9.0), 1.0);
        // Half strike: boundary at 5px; right edge hidden by (1-t).
        assert_eq!(m.hidden_fraction_at(15.0, 9.0), 0.5);
        assert_eq!(m.hidden_fraction_at(15.0, 4.0), 0.0);
        // Strike complete: fully visible for the rest of the window.
        assert_eq!(m.hidden_fraction_at(30.0, 9.0), 0.0);
        assert_eq!(m.hidden_fraction_at(119.0, 9.0), 0.0);
        // Window over: revealed.
        assert_eq!(m.hidden_fraction_at(120.0, 9.0), 0.0);
    }

    #[test]
    fn intensity_scales_concealment() {
        let m = RevealMath::new(TextAnimation::Streaming, 120, 0.5, 10.0);
        assert_eq!(m.hidden_fraction_at(0.0, 5.0), 0.5);
        assert_eq!(m.hidden_fraction_at(60.0, 9.0), 0.25);
        let m = RevealMath::new(TextAnimation::Typewriter, 120, 0.5, 10.0);
        assert_eq!(m.hidden_fraction_at(-1.0, 5.0), 0.5);
    }

    #[test]
    fn disabled_is_always_revealed() {
        let m = math(TextAnimation::Disabled);
        assert_eq!(m.phase(-100.0), RevealPhase::Revealed);
        assert_eq!(m.phase(50.0), RevealPhase::Revealed);
        assert_eq!(m.hidden_fraction_at(-100.0, 5.0), 0.0);
        assert_eq!(m.hidden_fraction_at(50.0, 5.0), 0.0);
    }
}

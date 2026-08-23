//! The deterministic effects model: drives the change tracker, the
//! typewriter burst schedule, and the cursor trail from [`FrameDelta`]s
//! under an explicit clock.
//!
//! Scheduling semantics are a direct port of the oracle's
//! `textAnimationAnimating` / `cursorTrailAnimating` (`verification/manifests/dirty-oracle-v2.patch`,
//! `src/renderer/generic.zig`, new-file lines 1396-1432):
//!
//! * text: animation frames are demanded while the most recent change
//!   timestamp is still within the reveal window — or *in the future* (a
//!   typewriter burst that ran ahead of the present keeps frames alive
//!   until its final staggered character reveals);
//! * trail: frames are demanded while within the configured fade duration;
//! * disabled effects demand nothing and retain nothing.

use mr_crabs_terminal::{DamageKind, FrameDelta, GridSize};

use crate::coords::CellPx;
use crate::key::{ChangeTracker, NEVER_BITS, NEVER_MS};
use crate::reveal::{CellPos, CellReveal};
use crate::schedule::TypewriterSchedule;
use crate::trail::{CursorTrail, TrailConfig, TrailFrame, cursor_rect};
use crate::{EffectsConfig, TextAnimation};

/// The per-frame payload: the changed cells still animating (or pending),
/// the trail state, and the scheduling decision. All vectors are retained
/// and refilled in place across frames.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct EffectsFrame {
    /// Changed cells currently inside their reveal window (both modes).
    /// Unchanged cells are never listed — the effect clips to changed
    /// cells only.
    pub revealing: Vec<CellReveal>,
    /// Typewriter cells whose change timestamp is still in the future;
    /// they are fully concealed and keep animation frames scheduled.
    pub pending: Vec<CellPos>,
    /// The cursor trail state for this frame.
    pub trail: TrailFrame,
    pub text_reveal_allowed: bool,
    /// True when another animation frame must be scheduled after this one.
    pub needs_frame: bool,
}

impl EffectsFrame {
    /// True when nothing is animating and no further frame is required.
    pub const fn is_idle(&self) -> bool {
        !self.needs_frame
    }
}

/// The deterministic effects model. One per terminal surface.
pub struct EffectsModel {
    config: EffectsConfig,
    size: GridSize,
    cell: CellPx,
    tracker: Option<ChangeTracker>,
    schedule: TypewriterSchedule,
    trail: CursorTrail,
    last_now_ms: f64,
    last_history_rows: Option<u32>,
    frame: EffectsFrame,
}

impl EffectsModel {
    /// Create a model for a grid and cell size. With a fully disabled
    /// config no state is allocated at all.
    pub fn new(config: EffectsConfig, size: GridSize, cell: CellPx) -> Self {
        let text_active = config.text_animation != TextAnimation::Disabled;
        let schedule = TypewriterSchedule::new(
            if text_active && config.text_animation == TextAnimation::Typewriter {
                config.text_animation_duration_ms as f64 / 8.0
            } else {
                0.0
            },
        );
        let tracker = if text_active {
            Some(ChangeTracker::new(
                usize::from(size.cols),
                usize::from(size.rows),
                config.max_tracked_cells,
            ))
        } else {
            None
        };
        Self {
            config,
            size,
            cell,
            tracker,
            schedule,
            trail: CursorTrail::new(TrailConfig::new(
                config.cursor_trail,
                config.cursor_trail_opacity,
                config.cursor_trail_duration_ms,
            )),
            last_now_ms: 0.0,
            last_history_rows: None,
            frame: EffectsFrame::default(),
        }
    }

    /// Replace the configuration at runtime. Disabling the text animation
    /// drops the change tracker and reveal buffers (zero retained
    /// allocations); disabling the trail drops its gradient cache.
    /// Re-enabling allocates a fresh tracker whose cells are all
    /// never-seen, so the next rebuild stamps every changed row — the
    /// oracle's enable behavior.
    pub fn set_config(&mut self, config: EffectsConfig) {
        let text_active = config.text_animation != TextAnimation::Disabled;
        self.config = config;
        self.schedule = TypewriterSchedule::new(
            if text_active && config.text_animation == TextAnimation::Typewriter {
                config.text_animation_duration_ms as f64 / 8.0
            } else {
                0.0
            },
        );
        if text_active {
            if self.tracker.is_none() {
                self.tracker = Some(ChangeTracker::new(
                    usize::from(self.size.cols),
                    usize::from(self.size.rows),
                    config.max_tracked_cells,
                ));
            }
        } else {
            self.tracker = None;
            self.frame.revealing = Vec::new();
            self.frame.pending = Vec::new();
        }
        self.trail.set_config(TrailConfig::new(
            config.cursor_trail,
            config.cursor_trail_opacity,
            config.cursor_trail_duration_ms,
        ));
    }

    pub const fn config(&self) -> &EffectsConfig {
        &self.config
    }

    pub const fn size(&self) -> GridSize {
        self.size
    }

    pub const fn cell(&self) -> CellPx {
        self.cell
    }

    /// Advance the model to a new frame under an explicit clock.
    ///
    /// `now_ms` is the animation clock in milliseconds; it is clamped to be
    /// monotonic so a backward clock can never regress timestamps. `focus`
    /// mirrors the oracle's `iFocus` (the trail draws nothing while the
    /// surface is unfocused).
    ///
    /// The returned reference borrows the model and is valid until the next
    /// call.
    pub fn apply_frame(&mut self, frame: &FrameDelta, now_ms: u64, focus: bool) -> &EffectsFrame {
        let now = (now_ms as f64).max(self.last_now_ms);
        self.last_now_ms = now;
        let size_changed = frame.size != self.size;
        let previous_history_rows = self.last_history_rows.replace(frame.viewport.history_rows);

        let out = &mut self.frame;
        out.revealing.clear();
        out.pending.clear();
        out.trail = TrailFrame::default();
        out.needs_frame = false;
        out.text_reveal_allowed = false;

        if size_changed {
            self.size = frame.size;
            if let Some(tracker) = &mut self.tracker {
                tracker.resize(
                    usize::from(frame.size.cols),
                    usize::from(frame.size.rows),
                    now,
                );
            }
        }

        let mut needs_text = false;
        if let Some(tracker) = &mut self.tracker {
            let duration_ms = self.config.text_animation_duration_ms as f64;
            let is_full = frame.damage == DamageKind::Full;
            let is_alt = frame.viewport.alternate_screen;
            let is_large = frame.rows.len() > 16;
            let history_consistent = previous_history_rows.is_some_and(|previous| {
                frame.viewport.history_rows == previous
                    || previous.checked_add(1) == Some(frame.viewport.history_rows)
            });
            let can_translate = !is_alt
                && !size_changed
                && is_full
                && history_consistent
                && tracker.can_translate_up_one(&frame.rows);
            let process_rows = !is_full || can_translate;
            let mut translated = false;
            let mut bottom_only = false;
            if is_full {
                if can_translate {
                    tracker.translate_up_one();
                    translated = true;
                    bottom_only = true;
                } else {
                    tracker.adopt_rows(&frame.rows);
                    self.schedule = TypewriterSchedule::new(
                        if self.config.text_animation == TextAnimation::Typewriter {
                            duration_ms / 8.0
                        } else {
                            0.0
                        },
                    );
                }
            }
            out.text_reveal_allowed =
                !is_alt && !size_changed && (translated || (!is_full && !is_large));
            if process_rows {
                self.schedule.begin_build(now, duration_ms);
                if bottom_only {
                    let target = self.size.rows.saturating_sub(1);
                    for rd in &frame.rows {
                        if rd.row == target {
                            tracker.update_row(
                                rd.row,
                                rd.generation,
                                &rd.cells,
                                now,
                                &mut self.schedule,
                            );
                            break;
                        }
                    }
                } else {
                    for rd in &frame.rows {
                        tracker.update_row(
                            rd.row,
                            rd.generation,
                            &rd.cells,
                            now,
                            &mut self.schedule,
                        );
                    }
                }
            }
            if tracker.last_change_ms() != NEVER_MS {
                let elapsed = now - tracker.last_change_ms();
                needs_text = elapsed < 0.0 || elapsed < duration_ms;
            }
            collect_reveals(tracker, self.config.text_animation, duration_ms, now, out);
        }

        out.trail = self.trail.frame(
            cursor_rect(&frame.cursor, self.cell),
            frame.cursor.visible,
            now,
            focus,
        );
        out.needs_frame = needs_text || out.trail.active;
        out
    }

    /// The scheduling decision from the most recent frame.
    pub const fn needs_frame(&self) -> bool {
        self.frame.needs_frame
    }

    /// The most recent frame payload.
    pub const fn frame(&self) -> &EffectsFrame {
        &self.frame
    }

    /// The packed per-cell change texture (one rgba8 texel per tracked
    /// cell, little-endian IEEE-754 bit pattern of shader-time seconds;
    /// sentinel = never changed). Empty when the text animation is
    /// disabled.
    pub fn change_texture(&self) -> &[u8] {
        self.tracker
            .as_ref()
            .map_or(&[], ChangeTracker::change_texture)
    }

    /// True when the change texture needs re-uploading since the last
    /// [`Self::clear_change_texture_dirty`].
    pub fn change_texture_dirty(&self) -> bool {
        self.tracker
            .as_ref()
            .is_some_and(ChangeTracker::upload_dirty)
    }

    /// Acknowledge a texture upload.
    pub fn clear_change_texture_dirty(&mut self) {
        if let Some(tracker) = &mut self.tracker {
            tracker.clear_upload_dirty();
        }
    }

    /// The most recent change timestamp (oracle `last_change_time`), in
    /// milliseconds; `None` until a cell changes. May lie in the future
    /// when a typewriter burst runs ahead of the present.
    pub fn last_change_ms(&self) -> Option<f64> {
        let last = self.tracker.as_ref()?.last_change_ms();
        (last != NEVER_MS).then_some(last)
    }

    /// Retained heap bytes across every subsystem: the change tracker
    /// arrays (exactly `min(cols * rows, max_tracked_cells)` tracked
    /// cells), the reveal/pending buffers, and the gradient descriptor
    /// cache. With all effects disabled this is exactly 0.
    pub fn retained_capacity(&self) -> usize {
        self.tracker
            .as_ref()
            .map_or(0, ChangeTracker::retained_capacity)
            + self.frame.revealing.capacity() * std::mem::size_of::<CellReveal>()
            + self.frame.pending.capacity() * std::mem::size_of::<CellPos>()
            + self.trail.retained_capacity()
    }
}

/// Build the per-frame reveal/pending lists from the tracked cells,
/// clipping to changed cells only (sentinel texels are skipped). Cells past
/// the reveal window are skipped as well.
fn collect_reveals(
    tracker: &ChangeTracker,
    mode: TextAnimation,
    duration_ms: f64,
    now: f64,
    out: &mut EffectsFrame,
) {
    let cols = tracker.cols();
    for i in 0..tracker.tracked_cells() {
        let bits = tracker.bits_at(i);
        if bits == NEVER_BITS {
            continue;
        }
        let change_ms = tracker.change_ms_at(i);
        let elapsed = now - change_ms;
        if elapsed >= duration_ms {
            continue;
        }
        let pos = CellPos::new((i / cols) as u16, (i % cols) as u16);
        if mode == TextAnimation::Typewriter && elapsed < 0.0 {
            out.pending.push(pos);
        } else {
            out.revealing.push(CellReveal::new(pos, change_ms, elapsed));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TrailEcho;
    use mr_crabs_terminal::{Cell, CursorState, DamageKind, RowDelta};

    fn cell(content: u32) -> Cell {
        Cell {
            content,
            style: 0,
            flags: 0,
        }
    }

    fn row(row: u16, generation: u64, contents: &[u32]) -> RowDelta {
        RowDelta {
            row,
            generation,
            cells: contents.iter().copied().map(cell).collect(),
            runs: Vec::new(),
        }
    }

    fn frame_at(size: GridSize, seq: u64, rows: Vec<RowDelta>, cursor: CursorState) -> FrameDelta {
        let mut f = FrameDelta::empty(size);
        f.sequence = seq;
        f.damage = if rows.is_empty() {
            DamageKind::Clean
        } else {
            DamageKind::Partial
        };
        f.rows = rows;
        f.cursor = cursor;
        f
    }

    fn config(mode: TextAnimation) -> EffectsConfig {
        EffectsConfig::new(mode, 120, 1.0, false, 0.35, 250, 256)
    }

    fn model(mode: TextAnimation, cols: u16, rows: u16) -> EffectsModel {
        EffectsModel::new(
            config(mode),
            GridSize::new(cols, rows),
            CellPx::new(10.0, 20.0),
        )
    }

    #[test]
    fn streaming_end_to_end_reveal_and_expiry() {
        let size = GridSize::new(8, 3);
        let mut m = model(TextAnimation::Streaming, 8, 3);
        let cursor = CursorState::default();
        let rows = vec![
            row(0, 1, &[65, 66, 32, 32, 32, 32, 32, 32]),
            row(1, 1, &[32, 32, 32, 32, 88, 32, 32, 32]),
        ];
        // Fresh tracker: the first rebuild stamps every rebuilt cell
        // (oracle never-seen behavior) at the shared rebuild time.
        let f = m.apply_frame(&frame_at(size, 1, rows, cursor), 1000, true);
        assert!(f.needs_frame);
        assert_eq!(f.revealing.len(), 16);
        assert!(f.revealing.iter().all(|r| r.change_ms == 1000.0));
        assert!(
            f.revealing
                .iter()
                .any(|r| r.pos == CellPos::new(0, 0) && r.elapsed_ms == 0.0)
        );
        assert!(
            f.revealing
                .iter()
                .any(|r| r.pos == CellPos::new(1, 4) && r.elapsed_ms == 0.0)
        );
        assert!(f.pending.is_empty());

        // Mid-window: the same cells keep revealing with growing elapsed.
        let f = m.apply_frame(&frame_at(size, 2, Vec::new(), cursor), 1060, true);
        assert!(f.needs_frame);
        assert!(f.revealing.iter().all(|r| r.elapsed_ms == 60.0));

        // Exactly at expiry: zero frames, empty lists.
        let f = m.apply_frame(&frame_at(size, 3, Vec::new(), cursor), 1120, true);
        assert!(!f.needs_frame);
        assert!(f.revealing.is_empty());
        assert!(f.pending.is_empty());
        assert!(f.is_idle());

        // And stays idle afterwards.
        let f = m.apply_frame(&frame_at(size, 4, Vec::new(), cursor), 2000, true);
        assert!(f.is_idle());
        assert!(!m.needs_frame());
    }

    #[test]
    fn unchanged_cells_never_restamp() {
        let size = GridSize::new(4, 1);
        let mut m = model(TextAnimation::Streaming, 4, 1);
        let cursor = CursorState::default();
        let rows = vec![row(0, 1, &[65, 32, 32, 32])];
        let f = m.apply_frame(&frame_at(size, 1, rows, cursor), 1000, true);
        assert_eq!(f.revealing.len(), 4); // fresh tracker stamps all
        assert!(f.revealing.iter().all(|r| r.change_ms == 1000.0));

        // Same content, new generation: no restamp, no re-animation.
        let rows = vec![row(0, 2, &[65, 32, 32, 32])];
        let f = m.apply_frame(&frame_at(size, 2, rows, cursor), 1100, true);
        assert_eq!(f.revealing.len(), 4);
        assert!(f.revealing.iter().all(|r| r.change_ms == 1000.0));
    }

    #[test]
    fn typewriter_stagger_across_rebuilds_via_model() {
        let size = GridSize::new(4, 1);
        let mut m = model(TextAnimation::Typewriter, 4, 1);
        let cursor = CursorState::default();

        // Rebuild 1 at t=1000 writes 'A' (fresh tracker: every rebuilt
        // cell consumes a schedule slot, spaces included).
        let rows = vec![row(0, 1, &[65, 32, 32, 32])];
        let f = m.apply_frame(&frame_at(size, 1, rows, cursor), 1000, true);
        assert_eq!(f.revealing[0].change_ms, 1000.0);
        assert_eq!(f.pending.len(), 3); // slots 1015/1030/1045 not yet due

        // Rebuild 2 at t=1016 writes 'B' (burst continues): the next slot
        // (1060) is still in the future, so (0,1) stays fully concealed
        // and pending — the corpus `typewriter-across-rebuilds` behavior.
        let rows = vec![row(0, 2, &[65, 66, 32, 32])];
        let f = m.apply_frame(&frame_at(size, 2, rows, cursor), 1016, true);
        assert!(f.pending.iter().any(|p| *p == CellPos::new(0, 1)));
        assert!(f.revealing.iter().all(|r| r.pos != CellPos::new(0, 1)));
        // `f`'s borrow of `m` ends here; the model's stamp is then
        // observable without aliasing.
        assert_eq!(m.last_change_ms(), Some(1060.0));

        // Rebuild 3 at t=1032 writes 'C' -> the following slot (1075),
        // likewise still pending at this instant.
        let rows = vec![row(0, 3, &[65, 66, 67, 32])];
        let f = m.apply_frame(&frame_at(size, 3, rows, cursor), 1032, true);
        assert!(f.pending.iter().any(|p| *p == CellPos::new(0, 2)));
        assert!(f.revealing.iter().all(|r| r.pos != CellPos::new(0, 2)));
        assert_eq!(m.last_change_ms(), Some(1075.0));

        // Once the cascade's slots arrive, the staggered cells reveal.
        let f = m.apply_frame(&frame_at(size, 4, Vec::new(), cursor), 1075, true);
        let b = f
            .revealing
            .iter()
            .find(|r| r.pos == CellPos::new(0, 1))
            .unwrap();
        assert_eq!(b.change_ms, 1060.0);
        let c = f
            .revealing
            .iter()
            .find(|r| r.pos == CellPos::new(0, 2))
            .unwrap();
        assert_eq!(c.change_ms, 1075.0);

        // Long after the cascade completes, a fresh burst starts at now.
        let rows = vec![row(0, 4, &[90, 66, 67, 32])];
        let f = m.apply_frame(&frame_at(size, 5, rows, cursor), 2000, true);
        let z = f
            .revealing
            .iter()
            .find(|r| r.pos == CellPos::new(0, 0))
            .unwrap();
        assert_eq!(z.change_ms, 2000.0);

        // Expiry: last stamp (2000) + 120 = 2120.
        let f = m.apply_frame(&frame_at(size, 6, Vec::new(), cursor), 2120, true);
        assert!(f.is_idle());
    }

    #[test]
    fn typewriter_future_cells_are_pending_and_keep_frames_alive() {
        let size = GridSize::new(4, 1);
        let mut m = model(TextAnimation::Typewriter, 4, 1);
        let cursor = CursorState::default();
        // Four cells change on a fresh tracker: stamps 1000/1015/1030/1045.
        let rows = vec![row(0, 1, &[65, 66, 67, 32])];
        let f = m.apply_frame(&frame_at(size, 1, rows, cursor), 1000, true);
        assert_eq!(f.revealing.len(), 1); // only the first cell animates
        assert_eq!(
            f.pending,
            vec![CellPos::new(0, 1), CellPos::new(0, 2), CellPos::new(0, 3)]
        );
        assert!(f.needs_frame);
        // At t=1016 the second cell's timestamp has arrived; 1030/1045
        // are still pending.
        let f = m.apply_frame(&frame_at(size, 2, Vec::new(), cursor), 1016, true);
        assert_eq!(f.pending, vec![CellPos::new(0, 2), CellPos::new(0, 3)]);
        assert!(f.needs_frame);
        // At t=1030 only the final staggered cell is still pending.
        let f = m.apply_frame(&frame_at(size, 3, Vec::new(), cursor), 1030, true);
        assert_eq!(f.pending, vec![CellPos::new(0, 3)]);
        assert!(f.needs_frame);
        // All timestamps arrived by 1045.
        let f = m.apply_frame(&frame_at(size, 4, Vec::new(), cursor), 1045, true);
        assert!(f.pending.is_empty());
        assert_eq!(f.revealing.len(), 4);
        // Final staggered cell reveals at 1045 + 120.
        let f = m.apply_frame(&frame_at(size, 5, Vec::new(), cursor), 1165, true);
        assert!(f.is_idle());
    }

    #[test]
    fn trail_fades_and_resumes_with_focus() {
        let mut cfg = config(TextAnimation::Disabled);
        cfg.cursor_trail = true;
        let mut m = EffectsModel::new(cfg, GridSize::new(8, 1), CellPx::new(10.0, 20.0));
        let mut cursor = CursorState::default();
        let size = GridSize::new(8, 1);

        let f = m.apply_frame(&frame_at(size, 1, Vec::new(), cursor), 2000, true);
        assert!(f.trail.active);
        assert_eq!(f.trail.alpha, 0.35);
        assert_eq!(f.trail.echoes, [TrailEcho::default(); 3]);
        assert!(f.needs_frame);

        cursor.col = 5;
        let f = m.apply_frame(&frame_at(size, 2, Vec::new(), cursor), 2016, true);
        assert!(f.trail.active);
        assert_eq!(f.trail.alpha, 0.35);
        assert_eq!(f.trail.echoes.len(), 3);
        assert!(f.trail.echoes[0].alpha > 0.0);
        assert_eq!(f.trail.glow_rect.x, 50.0);
        // Linear fade at t=2241: (1 - 225/250) * 0.35.
        let f = m.apply_frame(&frame_at(size, 3, Vec::new(), cursor), 2241, true);
        assert!((f.trail.alpha - 0.035).abs() < f64::EPSILON);

        // Unfocus: nothing draws, no frames scheduled.
        let f = m.apply_frame(&frame_at(size, 4, Vec::new(), cursor), 2241, false);
        assert!(!f.trail.active);
        assert!(!f.needs_frame);

        // Refocus within the window: the fade resumes from the change time.
        let f = m.apply_frame(&frame_at(size, 5, Vec::new(), cursor), 2251, true);
        assert!(f.trail.active);
        assert_eq!(f.trail.elapsed_ms, 235.0);

        // Expiry at change (2016) + 250 = 2266.
        let f = m.apply_frame(&frame_at(size, 6, Vec::new(), cursor), 2266, true);
        assert!(!f.trail.active);
        assert!(f.is_idle());
    }

    #[test]
    fn disabled_path_retains_and_schedules_nothing() {
        let cfg = EffectsConfig::new(TextAnimation::Disabled, 120, 1.0, false, 0.35, 250, 256);
        let mut m = EffectsModel::new(cfg, GridSize::new(8, 3), CellPx::new(10.0, 20.0));
        assert_eq!(m.retained_capacity(), 0);
        let cursor = CursorState::default();
        let rows = vec![row(0, 1, &[65, 66, 32, 32, 32, 32, 32, 32])];
        let f = m.apply_frame(&frame_at(GridSize::new(8, 3), 1, rows, cursor), 1000, true);
        assert!(f.revealing.is_empty());
        assert!(f.pending.is_empty());
        assert!(f.is_idle());
        assert_eq!(m.retained_capacity(), 0);
        assert!(m.change_texture().is_empty());
        assert!(!m.change_texture_dirty());
        assert_eq!(m.last_change_ms(), None);
    }

    #[test]
    fn disabling_drops_all_text_state() {
        let mut m = model(TextAnimation::Streaming, 8, 3);
        let cursor = CursorState::default();
        let rows = vec![row(0, 1, &[65, 32, 32, 32, 32, 32, 32, 32])];
        let f = m.apply_frame(&frame_at(GridSize::new(8, 3), 1, rows, cursor), 1000, true);
        assert!(f.needs_frame);
        assert!(m.retained_capacity() > 0);

        let mut cfg = config(TextAnimation::Disabled);
        cfg.cursor_trail = false;
        m.set_config(cfg);
        assert_eq!(m.retained_capacity(), 0);
        let f = m.apply_frame(
            &frame_at(GridSize::new(8, 3), 2, Vec::new(), cursor),
            1001,
            true,
        );
        assert!(f.is_idle());

        // Re-enable: fresh tracker; the next changed row re-animates.
        m.set_config(config(TextAnimation::Streaming));
        let rows = vec![row(0, 2, &[90, 32, 32, 32, 32, 32, 32, 32])];
        let f = m.apply_frame(&frame_at(GridSize::new(8, 3), 3, rows, cursor), 2000, true);
        assert!(f.needs_frame);
        assert_eq!(f.revealing[0].change_ms, 2000.0);
    }

    #[test]
    fn text_integrity_is_untouched() {
        let size = GridSize::new(8, 3);
        let mut m = model(TextAnimation::Streaming, 8, 3);
        let cursor = CursorState::default();
        let rows = vec![row(0, 1, &[65, 66, 32, 32, 32, 32, 32, 32])];
        let f = frame_at(size, 1, rows, cursor);
        let before: Vec<(u16, u64, Vec<Cell>)> = f
            .rows
            .iter()
            .map(|r| (r.row, r.generation, r.cells.clone()))
            .collect();
        let _ = m.apply_frame(&f, 1000, true);
        let after: Vec<(u16, u64, Vec<Cell>)> = f
            .rows
            .iter()
            .map(|r| (r.row, r.generation, r.cells.clone()))
            .collect();
        assert_eq!(before, after);
        // The model retains no text: only numeric change state counts.
        assert!(m.retained_capacity() > 0);
    }

    #[test]
    fn retained_capacity_is_stable_after_warmup() {
        let size = GridSize::new(8, 3);
        let mut m = model(TextAnimation::Streaming, 8, 3);
        let cursor = CursorState::default();
        let rows = vec![row(0, 1, &[65, 66, 32, 32, 32, 32, 32, 32])];
        let _ = m.apply_frame(&frame_at(size, 1, rows, cursor), 1000, true);
        let warm = m.retained_capacity();
        for seq in 2..20u64 {
            let _ = m.apply_frame(
                &frame_at(size, seq, Vec::new(), cursor),
                1000 + seq * 16,
                true,
            );
        }
        assert_eq!(m.retained_capacity(), warm);
    }

    #[test]
    fn backward_clock_is_clamped_monotonic() {
        let size = GridSize::new(4, 1);
        let mut m = model(TextAnimation::Streaming, 4, 1);
        let cursor = CursorState::default();
        let rows = vec![row(0, 1, &[65, 32, 32, 32])];
        let f = m.apply_frame(&frame_at(size, 1, rows, cursor), 2000, true);
        assert_eq!(f.revealing[0].change_ms, 2000.0);
        // A backwards clock must not regress stamps or reveal times.
        let f = m.apply_frame(&frame_at(size, 2, Vec::new(), cursor), 1000, true);
        assert_eq!(f.revealing[0].change_ms, 2000.0);
        assert_eq!(f.revealing[0].elapsed_ms, 0.0); // now stays 2000
    }

    #[test]
    fn resize_reveals_new_cells_and_preserves_prefix() {
        let mut m = model(TextAnimation::Streaming, 2, 1);
        let cursor = CursorState::default();
        let old = GridSize::new(2, 1);
        let rows = vec![row(0, 1, &[65, 66])];
        let f = m.apply_frame(&frame_at(old, 1, rows, cursor), 1000, true);
        assert_eq!(f.revealing.len(), 2);
        assert!(f.revealing.iter().all(|r| r.change_ms == 1000.0));

        // Resize to 3x2 at t=1500: the stored prefix survives (cells 0,1
        // keep their keys), new cells are marked changed at the resize
        // time, and rebuilt rows diff against the preserved snapshot.
        let new = GridSize::new(3, 2);
        let rows = vec![row(0, 2, &[65, 66, 67]), row(1, 1, &[88, 32, 32])];
        let f = m.apply_frame(&frame_at(new, 2, rows, cursor), 1500, true);
        assert_eq!(f.revealing.len(), 4); // (0,2) + row 1's three cells
        assert!(
            f.revealing
                .iter()
                .all(|r| r.change_ms == 1500.0 && r.pos.row == 1 || r.pos == CellPos::new(0, 2))
        );
        // Unchanged prefix cells (0,0)/(0,1) were not restamped: after the
        // 1500 window elapses, a real change at (0,0) restamps only it.
        let rows = vec![row(0, 3, &[90, 66, 67])];
        let f = m.apply_frame(&frame_at(new, 3, rows, cursor), 3000, true);
        let z = f
            .revealing
            .iter()
            .find(|r| r.pos == CellPos::new(0, 0))
            .unwrap();
        assert_eq!(z.change_ms, 3000.0);
        assert_eq!(f.revealing.len(), 1); // (0,1)/(0,2) unchanged content
    }

    #[test]
    fn change_texture_flows_through_model() {
        let size = GridSize::new(4, 1);
        let mut m = model(TextAnimation::Streaming, 4, 1);
        let cursor = CursorState::default();
        assert!(m.change_texture_dirty()); // fresh tracker needs upload
        // A fresh tracker's texels are all sentinel: -1000 shader seconds
        // -> -1000.0 f32 -> 0xC47A0000 little-endian.
        assert_eq!(m.change_texture().len(), 16); // 4 cells x rgba8
        assert_eq!(&m.change_texture()[0..4], &[0x00, 0x00, 0x7A, 0xC4]);
        m.clear_change_texture_dirty();
        assert!(!m.change_texture_dirty());

        let rows = vec![row(0, 1, &[65, 32, 32, 32])];
        let _ = m.apply_frame(&frame_at(size, 1, rows, cursor), 1000, true);
        assert!(m.change_texture_dirty());
        // Every rebuilt cell was stamped at 1000 ms -> 1.0 s -> 0x3F800000
        // little-endian.
        assert!(
            m.change_texture()
                .chunks_exact(4)
                .all(|t| t == [0x00, 0x00, 0x80, 0x3F])
        );
        m.clear_change_texture_dirty();
        assert!(!m.change_texture_dirty());
    }
    #[test]
    fn primary_scroll_translates_reveal_state_and_stamps_only_new_row() {
        let size = GridSize::new(3, 3);
        let mut m = model(TextAnimation::Streaming, 3, 3);
        let cursor = CursorState::default();
        let mut baseline = frame_at(
            size,
            1,
            vec![
                row(0, 1, &[65, 32, 32]),
                row(1, 1, &[66, 32, 32]),
                row(2, 1, &[67, 32, 32]),
            ],
            cursor,
        );
        baseline.damage = DamageKind::Full;
        let frame = m.apply_frame(&baseline, 1000, true);
        assert!(!frame.text_reveal_allowed);
        assert!(frame.revealing.is_empty());

        let mut scrolled = frame_at(
            size,
            2,
            vec![
                row(0, 2, &[66, 32, 32]),
                row(1, 2, &[67, 32, 32]),
                row(2, 2, &[68, 32, 32]),
            ],
            cursor,
        );
        scrolled.damage = DamageKind::Full;
        scrolled.viewport.history_rows = 1;
        let frame = m.apply_frame(&scrolled, 1100, true);
        assert!(frame.text_reveal_allowed);
        assert_eq!(frame.revealing.len(), 3);
        assert!(
            frame
                .revealing
                .iter()
                .all(|reveal| reveal.pos.row == 2 && reveal.change_ms == 1100.0)
        );

        let mut clean = frame_at(size, 3, Vec::new(), cursor);
        clean.viewport.history_rows = 1;
        let frame = m.apply_frame(&clean, 1110, true);
        assert!(frame.text_reveal_allowed);
        assert_eq!(frame.revealing.len(), 3);
        assert!(frame.revealing.iter().all(|reveal| reveal.pos.row == 2));
    }
    #[test]
    fn tall_and_saturated_history_scrolls_still_reveal_new_bottom_row() {
        let size = GridSize::new(3, 20);
        let mut m = model(TextAnimation::Streaming, 3, 20);
        let cursor = CursorState::default();
        let rows = (0..20)
            .map(|index| row(index, 1, &[65 + u32::from(index), 32, 32]))
            .collect();
        let mut baseline = frame_at(size, 1, rows, cursor);
        baseline.damage = DamageKind::Full;
        let _ = m.apply_frame(&baseline, 1000, true);

        let rows = (0..20)
            .map(|index| {
                let content = if index == 19 {
                    90
                } else {
                    66 + u32::from(index)
                };
                row(index, 2, &[content, 32, 32])
            })
            .collect();
        let mut first_scroll = frame_at(size, 2, rows, cursor);
        first_scroll.damage = DamageKind::Full;
        first_scroll.viewport.history_rows = 1;
        let frame = m.apply_frame(&first_scroll, 1100, true);
        assert!(frame.text_reveal_allowed);
        assert_eq!(frame.revealing.len(), 3);
        assert!(frame.revealing.iter().all(|reveal| reveal.pos.row == 19));

        let rows = (0..20)
            .map(|index| {
                let content = if index == 19 {
                    91
                } else if index == 18 {
                    90
                } else {
                    67 + u32::from(index)
                };
                row(index, 3, &[content, 32, 32])
            })
            .collect();
        let mut saturated_scroll = frame_at(size, 3, rows, cursor);
        saturated_scroll.damage = DamageKind::Full;
        saturated_scroll.viewport.history_rows = 1;
        let frame = m.apply_frame(&saturated_scroll, 1300, true);
        assert!(frame.text_reveal_allowed);
        assert_eq!(frame.revealing.len(), 3);
        assert!(frame.revealing.iter().all(|reveal| reveal.pos.row == 19));
    }
}

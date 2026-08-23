//! Persistent, bounded burst scheduling for the typewriter text animation.
//!
//! Port of the oracle's `TypewriterSchedule` (frozen in
//! `verification/manifests/dirty-oracle-v2.patch`, `src/renderer/generic.zig`,
//! new-file lines 48-186): every changed cell is timestamped one stagger
//! later than the previous changed cell, in reading order, and the schedule
//! survives across closely adjacent rebuilds so a shell output burst (a
//! command echo, its error output, and the next prompt arriving in separate
//! rebuilds) reveals in reading order instead of all at one timestamp.
//!
//! A rebuild starts a fresh burst when there is no schedule yet, when the
//! previous cascade has fully revealed (`now >= last + duration`), or when
//! continuing would run the schedule more than [`MAX_AHEAD_MS`] past the
//! present — the last rule bounds the reveal backlog so sustained fast
//! output can never grow the animation window without limit. A fresh burst
//! starts at the current time, so newly written lines are never delayed by
//! an already-finished (or unreasonably long) cascade. Timestamps only ever
//! advance when a changed cell consumes one, so unchanged content never
//! retimes the schedule.

/// Sentinel for "no schedule yet": -1000 shader seconds (oracle
/// `TextAnimationState.never = -1000`), expressed in milliseconds. Only
/// ever compared for equality, never used in time arithmetic.
pub const NEVER_MS: f64 = -1_000_000.0;

/// The maximum amount (milliseconds) the schedule may run ahead of the
/// present before a rebuild starts a fresh burst (oracle
/// `TypewriterSchedule.max_ahead = 1.0` seconds).
pub const MAX_AHEAD_MS: f64 = 1000.0;

/// Persistent typewriter burst schedule (see module docs).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TypewriterSchedule {
    next_ms: f64,
    last_ms: f64,
    stagger_ms: f64,
    build_now_ms: f64,
    build_limit_ms: f64,
    active_row: Option<u16>,
}

impl TypewriterSchedule {
    /// A schedule with the given per-cell stagger. A zero stagger leaves
    /// the schedule idle.
    pub const fn new(stagger_ms: f64) -> Self {
        Self {
            next_ms: NEVER_MS,
            last_ms: NEVER_MS,
            stagger_ms,
            build_now_ms: NEVER_MS,
            build_limit_ms: NEVER_MS,
            active_row: None,
        }
    }

    /// The per-cell stagger in milliseconds.
    pub const fn stagger_ms(&self) -> f64 {
        self.stagger_ms
    }

    /// True when the schedule hands out staggered timestamps (typewriter
    /// mode); false for streaming/none.
    pub const fn is_active(&self) -> bool {
        self.stagger_ms > 0.0
    }

    /// The next timestamp to hand out, or [`NEVER_MS`] when the schedule
    /// has not been started.
    pub const fn next_ms(&self) -> f64 {
        self.next_ms
    }

    /// The most recent timestamp handed out, or [`NEVER_MS`] when none.
    pub const fn last_ms(&self) -> f64 {
        self.last_ms
    }

    /// Begin a rebuild. If this rebuild starts a fresh burst, reset `next`
    /// to the given current time; otherwise the burst continues and `next`
    /// is left untouched so this rebuild's changed cells are timestamped
    /// after the previous rebuild's. No-op when the schedule is idle.
    pub fn begin_build(&mut self, now_ms: f64, duration_ms: f64) {
        if !self.is_active() {
            return;
        }
        self.build_now_ms = now_ms;
        self.build_limit_ms = now_ms + duration_ms;
        if self.next_ms != NEVER_MS
            && now_ms < self.last_ms + duration_ms
            && self.next_ms <= now_ms + MAX_AHEAD_MS
        {
            return;
        }
        self.next_ms = now_ms;
    }

    pub fn begin_row(&mut self, row: u16) {
        if !self.is_active() || self.active_row == Some(row) {
            return;
        }
        self.active_row = Some(row);
        self.next_ms = self.build_now_ms;
    }

    pub fn translate_up_one(&mut self) {
        self.active_row = self.active_row.and_then(|row| row.checked_sub(1));
    }

    /// Hand out the timestamp for the next changed cell and advance the
    /// schedule by one stagger step.
    pub fn next_timestamp(&mut self) -> f64 {
        let timestamp = self.next_ms.min(self.build_limit_ms);
        self.next_ms += self.stagger_ms;
        self.last_ms = timestamp;
        timestamp
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn continued_burst_is_clamped_to_each_build_window() {
        let duration_ms = 60.0;
        let stagger = duration_ms / 8.0;
        let mut schedule = TypewriterSchedule::new(stagger);
        let now = 10_000.0;

        schedule.begin_build(now, duration_ms);
        let first = schedule.next_timestamp();
        let mut last = first;
        for _ in 0..24 {
            last = schedule.next_timestamp();
        }
        assert_eq!(first, now);
        assert_eq!(last, now + duration_ms);

        schedule.begin_build(now + 4.0, duration_ms);
        assert_eq!(schedule.next_timestamp(), now + 4.0 + duration_ms);

        schedule.begin_build(now + 8.0, duration_ms);
        assert_eq!(schedule.next_timestamp(), now + 8.0 + duration_ms);
    }

    #[test]
    fn different_row_starts_at_current_build_time() {
        let mut schedule = TypewriterSchedule::new(75.0);
        schedule.begin_build(1_000.0, 600.0);
        schedule.begin_row(0);
        for _ in 0..20 {
            _ = schedule.next_timestamp();
        }
        schedule.begin_row(1);
        assert_eq!(schedule.next_timestamp(), 1_000.0);
    }

    /// Port of the oracle test "typewriter schedule starts a fresh burst
    /// after the cascade completes".
    #[test]
    fn fresh_burst_after_cascade_completes() {
        let duration_ms = 60.0;
        let stagger = duration_ms / 8.0;
        let mut s = TypewriterSchedule::new(stagger);
        s.begin_build(1000.0, duration_ms);
        _ = s.next_timestamp();
        let old_last = s.next_timestamp();

        // The cascade has fully revealed: a rebuild long after it starts a
        // fresh burst at the current time instead of continuing, so the
        // new line reveals immediately rather than at a stale timestamp.
        let now = old_last + duration_ms + 500.0;
        s.begin_build(now, duration_ms);
        let first = s.next_timestamp();
        assert_eq!(first, now);
        assert!(first > old_last);
    }

    #[test]
    fn bounds_the_reveal_backlog() {
        let duration_ms = 60.0;
        let stagger = duration_ms / 8.0;
        let mut schedule = TypewriterSchedule::new(stagger);
        schedule.begin_build(1_000.0, duration_ms);
        let mut last = 1_000.0;
        for _ in 0..300 {
            last = schedule.next_timestamp();
        }
        assert_eq!(last, 1_000.0 + duration_ms);

        let now = 1_500.0;
        schedule.begin_build(now, duration_ms);
        assert_eq!(schedule.next_timestamp(), now);
    }

    #[test]
    fn idle_schedule_is_a_noop() {
        // Streaming: zero stagger leaves the schedule idle; begin_build
        // never resets `next` and next_timestamp would hand out the
        // sentinel — callers must not invoke it (the tracker routes
        // streaming through the rebuild time instead).
        let mut s = TypewriterSchedule::new(0.0);
        s.begin_build(5000.0, 120.0);
        assert!(!s.is_active());
        assert_eq!(s.next_ms(), NEVER_MS);
        assert_eq!(s.last_ms(), NEVER_MS);
    }
}

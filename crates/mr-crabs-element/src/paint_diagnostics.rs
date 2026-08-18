use std::sync::Arc;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaintRafReason {
    None,
    CursorBlink,
    Effects,
    Both,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PaintDiagnosticsEvent {
    pub sequence: u64,
    pub cursor_blink_requested: bool,
    pub cursor_visible_phase: bool,
    pub effects_busy: bool,
    pub burst_bypass: bool,
    pub revealing: usize,
    pub pending: usize,
    pub effects_needs_frame: bool,
    pub trail_active: bool,
    pub trail_alpha: f64,
    pub raf_reason: PaintRafReason,
}

pub type PaintDiagnosticsSink = Arc<dyn Fn(PaintDiagnosticsEvent) + Send + Sync>;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct PaintEffectsOutcome {
    pub busy: bool,
    pub burst_bypass: bool,
    pub revealing: usize,
    pub pending: usize,
    pub needs_frame: bool,
    pub trail_active: bool,
    pub trail_alpha: f64,
}

pub(crate) fn diagnostic_event(
    sequence: u64,
    cursor_blink_requested: bool,
    cursor_visible_phase: bool,
    effects: PaintEffectsOutcome,
) -> PaintDiagnosticsEvent {
    let raf_reason = match (cursor_blink_requested, effects.busy) {
        (true, true) => PaintRafReason::Both,
        (true, false) => PaintRafReason::CursorBlink,
        (false, true) => PaintRafReason::Effects,
        (false, false) => PaintRafReason::None,
    };
    PaintDiagnosticsEvent {
        sequence,
        cursor_blink_requested,
        cursor_visible_phase,
        effects_busy: effects.busy,
        burst_bypass: effects.burst_bypass,
        revealing: effects.revealing,
        pending: effects.pending,
        effects_needs_frame: effects.needs_frame,
        trail_active: effects.trail_active,
        trail_alpha: effects.trail_alpha,
        raf_reason,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raf_reason_comes_from_cursor_and_effects_requests() {
        for (cursor, effects, expected) in [
            (false, false, PaintRafReason::None),
            (true, false, PaintRafReason::CursorBlink),
            (false, true, PaintRafReason::Effects),
            (true, true, PaintRafReason::Both),
        ] {
            let event = diagnostic_event(
                7,
                cursor,
                false,
                PaintEffectsOutcome {
                    busy: effects,
                    ..PaintEffectsOutcome::default()
                },
            );
            assert_eq!(event.raf_reason, expected);
        }
    }

    #[test]
    fn event_preserves_actual_paint_outcome() {
        let event = diagnostic_event(
            42,
            false,
            true,
            PaintEffectsOutcome {
                busy: true,
                burst_bypass: true,
                revealing: 3,
                pending: 2,
                needs_frame: true,
                trail_active: true,
                trail_alpha: 0.35,
            },
        );
        assert_eq!(event.sequence, 42);
        assert!(event.cursor_visible_phase);
        assert!(event.effects_busy);
        assert!(event.burst_bypass);
        assert_eq!((event.revealing, event.pending), (3, 2));
        assert!(event.effects_needs_frame);
        assert!(event.trail_active);
        assert_eq!(event.trail_alpha, 0.35);
        assert_eq!(event.raf_reason, PaintRafReason::Effects);
    }
}

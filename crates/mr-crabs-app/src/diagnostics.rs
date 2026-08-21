use std::collections::VecDeque;

use parking_lot::Mutex;

use crate::model::split::PaneId;
use mr_crabs_terminal::{CursorShape, DamageKind};

/// Flattened diagnostic frame event. Cloneable and uses existing terminal types.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiagnosticFrameEvent {
    pub pane_id: PaneId,
    pub sequence: u64,
    pub damage: DamageKind,
    pub cursor_row: u16,
    pub cursor_col: u16,
    pub cursor_shape: CursorShape,
    pub cursor_visible: bool,
    pub cursor_blinking: bool,
    pub cursor_wrap_pending: bool,
    pub alternate_screen: bool,
}

/// Aggregate pump diagnostic event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiagnosticPumpEvent {
    pub chunks: usize,
    pub bytes: usize,
    pub frames: usize,
    pub pending: bool,
}

impl DiagnosticPumpEvent {
    pub fn changed(&self) -> bool {
        self.chunks > 0 || self.frames > 0
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DiagnosticRafReason {
    None,
    CursorBlink,
    Effects,
    Both,
}

#[derive(Clone, Debug, PartialEq)]
pub struct DiagnosticPaintEvent {
    pub pane_id: PaneId,
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
    pub raf_reason: DiagnosticRafReason,
}

/// A single diagnostic entry: either a pump aggregate or a focused frame snapshot.
#[derive(Clone, Debug, PartialEq)]
pub enum DiagnosticEvent {
    Pump(DiagnosticPumpEvent),
    Frame(DiagnosticFrameEvent),
    Paint(DiagnosticPaintEvent),
}

impl DiagnosticEvent {
    pub fn as_pump(&self) -> Option<&DiagnosticPumpEvent> {
        match self {
            Self::Pump(e) => Some(e),
            _ => None,
        }
    }

    pub fn as_frame(&self) -> Option<&DiagnosticFrameEvent> {
        match self {
            Self::Frame(e) => Some(e),
            _ => None,
        }
    }

    pub fn as_paint(&self) -> Option<&DiagnosticPaintEvent> {
        match self {
            Self::Paint(e) => Some(e),
            _ => None,
        }
    }
}

/// Bounded diagnostic ring owned by the app crate.
pub struct DiagnosticTrace {
    capacity: usize,
    events: Mutex<VecDeque<DiagnosticEvent>>,
}

impl DiagnosticTrace {
    pub fn new(capacity: usize) -> Self {
        let capacity = capacity.max(1);
        Self {
            capacity,
            events: Mutex::new(VecDeque::with_capacity(capacity)),
        }
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }

    pub fn len(&self) -> usize {
        self.events.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.lock().is_empty()
    }

    pub fn push(&self, event: DiagnosticEvent) {
        let mut guard = self.events.lock();
        if guard.len() >= self.capacity {
            guard.pop_front();
        }
        guard.push_back(event);
    }

    /// Snapshot clones without draining.
    pub fn snapshot(&self) -> Vec<DiagnosticEvent> {
        self.events.lock().iter().cloned().collect()
    }

    pub fn clear(&self) {
        self.events.lock().clear();
    }
}

impl std::fmt::Debug for DiagnosticTrace {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiagnosticTrace")
            .field("capacity", &self.capacity)
            .field("len", &self.len())
            .finish()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn trace_capacity_clamped_and_evicts_oldest() {
        let trace = DiagnosticTrace::new(0);
        assert_eq!(trace.capacity(), 1, "capacity clamped to 1");
        assert!(trace.is_empty());
        trace.push(DiagnosticEvent::Pump(DiagnosticPumpEvent {
            chunks: 1,
            bytes: 10,
            frames: 1,
            pending: false,
        }));
        trace.push(DiagnosticEvent::Pump(DiagnosticPumpEvent {
            chunks: 2,
            bytes: 20,
            frames: 1,
            pending: false,
        }));
        let snap = trace.snapshot();
        assert_eq!(snap.len(), 1);
        assert_eq!(snap[0].as_pump().unwrap().chunks, 2, "oldest evicted");
        assert_eq!(trace.len(), 1);
        trace.push(DiagnosticEvent::Pump(DiagnosticPumpEvent {
            chunks: 3,
            bytes: 30,
            frames: 1,
            pending: true,
        }));
        assert_eq!(trace.snapshot()[0].as_pump().unwrap().chunks, 3);
    }

    #[test]
    fn trace_order_preserved_and_snapshot_nondraining() {
        let trace = DiagnosticTrace::new(3);
        for i in 0..5 {
            trace.push(DiagnosticEvent::Pump(DiagnosticPumpEvent {
                chunks: i,
                bytes: i * 10,
                frames: 1,
                pending: false,
            }));
        }
        let snap = trace.snapshot();
        assert_eq!(snap.len(), 3);
        assert_eq!(snap[0].as_pump().unwrap().chunks, 2);
        assert_eq!(snap[1].as_pump().unwrap().chunks, 3);
        assert_eq!(snap[2].as_pump().unwrap().chunks, 4);
        assert_eq!(trace.len(), 3);
        assert_eq!(trace.snapshot().len(), 3);
    }

    #[test]
    fn frame_event_cloneable_with_terminal_types() {
        let ev = DiagnosticFrameEvent {
            pane_id: PaneId::new(7),
            sequence: 42,
            damage: DamageKind::Partial,
            cursor_row: 2,
            cursor_col: 3,
            cursor_shape: CursorShape::Block,
            cursor_visible: true,
            cursor_blinking: false,
            cursor_wrap_pending: true,
            alternate_screen: false,
        };
        let cloned = ev.clone();
        assert_eq!(cloned, ev);
        let pump = DiagnosticPumpEvent {
            chunks: 1,
            bytes: 5,
            frames: 1,
            pending: false,
        };
        assert!(pump.changed());
        let empty_pump = DiagnosticPumpEvent {
            chunks: 0,
            bytes: 0,
            frames: 0,
            pending: false,
        };
        assert!(!empty_pump.changed());
        let wrapped = DiagnosticEvent::Frame(ev.clone());
        assert!(wrapped.as_frame().is_some());
        assert!(wrapped.as_pump().is_none());
        assert_eq!(wrapped.as_frame().unwrap().pane_id, PaneId::new(7));
    }

    #[test]
    fn paint_event_pushed_and_mapped() {
        let trace = DiagnosticTrace::new(4);
        let ev = DiagnosticPaintEvent {
            pane_id: PaneId::new(3),
            sequence: 11,
            cursor_blink_requested: true,
            cursor_visible_phase: true,
            effects_busy: false,
            burst_bypass: false,
            revealing: 1,
            pending: 0,
            effects_needs_frame: false,
            trail_active: false,
            trail_alpha: 0.0,
            raf_reason: DiagnosticRafReason::CursorBlink,
        };
        trace.push(DiagnosticEvent::Paint(ev.clone()));
        let snap = trace.snapshot();
        assert_eq!(snap.len(), 1);
        let got = snap[0].as_paint().unwrap();
        assert_eq!(got.pane_id, PaneId::new(3));
        assert_eq!(got.sequence, 11);
        assert_eq!(got.raf_reason, DiagnosticRafReason::CursorBlink);
        assert!(snap[0].as_pump().is_none());
        assert!(snap[0].as_frame().is_none());
        // PartialEq works with f64
        assert_eq!(got.trail_alpha, 0.0);
        assert_eq!(got.clone(), ev);
    }

    #[test]
    fn paint_ring_evicts_and_preserves_order() {
        let trace = DiagnosticTrace::new(2);
        trace.push(DiagnosticEvent::Pump(DiagnosticPumpEvent {
            chunks: 1,
            bytes: 1,
            frames: 1,
            pending: false,
        }));
        trace.push(DiagnosticEvent::Paint(DiagnosticPaintEvent {
            pane_id: PaneId::new(1),
            sequence: 1,
            cursor_blink_requested: false,
            cursor_visible_phase: false,
            effects_busy: true,
            burst_bypass: true,
            revealing: 5,
            pending: 2,
            effects_needs_frame: true,
            trail_active: true,
            trail_alpha: 0.5,
            raf_reason: DiagnosticRafReason::Effects,
        }));
        trace.push(DiagnosticEvent::Paint(DiagnosticPaintEvent {
            pane_id: PaneId::new(2),
            sequence: 2,
            cursor_blink_requested: true,
            cursor_visible_phase: false,
            effects_busy: true,
            burst_bypass: false,
            revealing: 0,
            pending: 0,
            effects_needs_frame: false,
            trail_active: false,
            trail_alpha: 0.0,
            raf_reason: DiagnosticRafReason::Both,
        }));
        let snap = trace.snapshot();
        assert_eq!(snap.len(), 2);
        // First pump evicted
        assert!(snap[0].as_paint().is_some());
        assert_eq!(snap[0].as_paint().unwrap().pane_id, PaneId::new(1));
        assert_eq!(snap[1].as_paint().unwrap().pane_id, PaneId::new(2));
        assert_eq!(
            snap[1].as_paint().unwrap().raf_reason,
            DiagnosticRafReason::Both
        );
    }
}

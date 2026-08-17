//! Secure input: the state machine for hiding typed input from other apps.
//!
//! macOS secure input is a platform service (NSApplication secure input).
//! The shell models the full state machine here — enabled/disabled,
//! per-pane tracking, toggle counts — and applies it through a
//! [`SecureInputBackend`]. The shipped default backend is the explicit
//! disabled implementation; a platform backend (e.g. an objc2 shim) can be
//! plugged in without changing the model.

use std::time::Instant;

use serde::{Deserialize, Serialize};

use crate::model::pane::PaneId;

/// Secure-input state machine.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct SecureInputState {
    enabled: bool,
    /// Panes currently under secure input.
    panes: Vec<PaneId>,
    /// Total enable/disable transitions.
    pub toggles: u64,
    #[serde(skip)]
    pub last_toggle: Option<Instant>,
}

impl SecureInputState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Set the desired state; returns the new state. The model always
    /// records the intent; the backend reports whether the platform honored
    /// it.
    pub fn set_enabled(&mut self, enabled: bool) -> bool {
        if self.enabled == enabled {
            return self.enabled;
        }
        self.enabled = enabled;
        self.toggles += 1;
        self.last_toggle = Some(Instant::now());
        if !enabled {
            self.panes.clear();
        }
        self.enabled
    }

    /// Toggle secure input; returns the new state.
    pub fn toggle(&mut self) -> bool {
        self.set_enabled(!self.enabled)
    }

    pub fn track_pane(&mut self, pane: PaneId) {
        if !self.panes.contains(&pane) {
            self.panes.push(pane);
        }
    }

    pub fn untrack_pane(&mut self, pane: PaneId) {
        self.panes.retain(|tracked| *tracked != pane);
    }

    pub fn clear_panes(&mut self) {
        self.panes.clear();
    }

    pub fn is_tracking(&self, pane: PaneId) -> bool {
        self.panes.contains(&pane)
    }

    pub fn tracked_panes(&self) -> &[PaneId] {
        &self.panes
    }
}

/// Errors from a secure-input backend.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SecureInputError {
    /// The platform does not support secure input.
    Unsupported,
    /// The platform call failed.
    Backend(String),
}

/// Platform boundary for secure input. The model state is authoritative;
/// the backend applies it to the platform.
pub trait SecureInputBackend: Send + Sync {
    fn is_supported(&self) -> bool;
    fn set_secure_input(&self, enabled: bool) -> Result<(), SecureInputError>;
}

/// The explicit disabled implementation: secure input state is fully
/// modeled, but no platform call is made.
pub struct DisabledSecureInputBackend {
    pub reason: String,
}

impl SecureInputBackend for DisabledSecureInputBackend {
    fn is_supported(&self) -> bool {
        false
    }

    fn set_secure_input(&self, _enabled: bool) -> Result<(), SecureInputError> {
        Err(SecureInputError::Unsupported)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toggle_flips_state_and_counts() {
        let mut state = SecureInputState::new();
        assert!(!state.is_enabled());
        assert!(state.toggle());
        assert!(state.is_enabled());
        assert_eq!(state.toggles, 1);
        assert!(!state.toggle());
        assert_eq!(state.toggles, 2);
    }

    #[test]
    fn panes_are_tracked_and_cleared() {
        let mut state = SecureInputState::new();
        state.set_enabled(true);
        let a = PaneId::new(1);
        let b = PaneId::new(2);
        state.track_pane(a);
        state.track_pane(b);
        state.track_pane(a); // idempotent
        assert_eq!(state.tracked_panes().len(), 2);
        state.untrack_pane(a);
        assert_eq!(state.tracked_panes(), &[b]);
        state.set_enabled(false);
        assert!(
            state.tracked_panes().is_empty(),
            "disabling clears tracking"
        );
    }

    #[test]
    fn disabled_backend_reports_unsupported() {
        let backend = DisabledSecureInputBackend {
            reason: "no platform shim".to_string(),
        };
        assert!(!backend.is_supported());
        assert_eq!(
            backend.set_secure_input(true),
            Err(SecureInputError::Unsupported)
        );
    }
}

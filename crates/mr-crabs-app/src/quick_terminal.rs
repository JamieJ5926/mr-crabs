//! Quick terminal: a dedicated terminal window that toggles in and out of
//! view while its session stays alive (Ghostty behavior).
//!
//! The state here is a pure data holder; the window show/hide logic lives on
//! [`AppModel`] so it can manipulate both the quick-terminal state and the
//! window table without borrow conflicts.

use std::time::Instant;

use mr_crabs_terminal::GridSize;
use serde::{Deserialize, Serialize};

use crate::model::window::WindowId;

/// Quick-terminal visibility state.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct QuickTerminalState {
    pub visible: bool,
    pub window_id: Option<WindowId>,
    pub grid: GridSize,
    /// The window that was active before the quick terminal took focus.
    pub previous_window: Option<WindowId>,
    /// Total number of show/hide transitions.
    pub toggles: u64,
    #[serde(skip)]
    pub last_toggle: Option<Instant>,
}

impl QuickTerminalState {
    pub fn new(grid: GridSize) -> Self {
        Self {
            visible: false,
            window_id: None,
            grid,
            previous_window: None,
            toggles: 0,
            last_toggle: None,
        }
    }

    pub fn is_visible(&self) -> bool {
        self.visible
    }

    pub fn window(&self) -> Option<WindowId> {
        self.window_id
    }

    pub fn toggle_count(&self) -> u64 {
        self.toggles
    }
}

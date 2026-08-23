//! S10: the Mr Crabs product shell.
//!
//! This crate owns the GPUI application shell for the pure-Rust rewrite:
//! windows, tabs, recursive splits, pane focus/navigation, the command
//! palette, typed settings with atomic reload, the quick terminal,
//! menus, secure input, accessibility snapshots/actions, app intents,
//! dock behavior, updates and crash reporting interfaces (explicit
//! disabled/local implementations only, never network telemetry), a
//! platform capability model, and versioned shell-state persistence and
//! restore.
//!
//! The headless shell model lives in [`model`] and the supporting domain
//! modules ([`palette`], [`settings`], [`quick_terminal`], [`menu`],
//! [`secure_input`], [`accessibility`], [`intent`], [`dock`], [`updates`],
//! [`crash`], [`platform`], [`restore`], [`keymap`], [`action`]). The
//! [`ui`] module bridges the model to the pinned GPUI revision
//! (`03e5ad8a630c84c3990055905d0444ea0a519b7f`) and the binary target
//! `mr-crabs` ([`bin/mr-crabs.rs`]) starts one window with focus,
//! keybindings, menus, accessibility roles, and `TerminalElement`
//! rendering.
//!
//! Design invariants:
//!
//! - The model is pure Rust: no GPUI types in [`model`], so every shell
//!   contract is headlessly testable.
//! - PTY ownership is bounded (bounded reader/writer queues in
//!   `mr-crabs-pty`) and shut down deterministically: closing a pane, tab,
//!   or window shuts its sessions down with a bounded grace period, and
//!   [`AppModel`]'s `Drop` performs a bounded best-effort shutdown.
//! - Frame handoff is immutable: panes publish `Arc<FrameDelta>` and the
//!   renderer consumes the shared frame without ever locking the engine.
//! - [`AppCore`] (the S4 single-terminal core) is preserved source
//!   compatible; S10 only adds additive methods.
//! - No S6-S9 crate is referenced directly; integration is deferred to the
//!   parent.
//! - The product retains every inherited Ghostty shell behavior as modeled
//!   production state; nothing is deleted to simplify the shell.

pub mod accessibility;
pub mod action;
pub mod animated_fetch;
pub mod crash;
pub mod diagnostics;
pub mod dock;
pub mod intent;
pub mod keymap;
pub mod menu;
pub mod model;
pub mod palette;
pub mod phase;
pub mod platform;
pub mod quick_terminal;
pub mod restore;
pub mod secure_input;
pub mod settings;
pub mod ui;
pub mod updates;
pub use action::AppAction;
pub use diagnostics::{
    DiagnosticEvent, DiagnosticFrameEvent, DiagnosticPaintEvent, DiagnosticPumpEvent,
    DiagnosticRafReason, DiagnosticTrace,
};
pub use model::app_model::{ActionResult, AppModel, AppPumpStats};
pub use model::pane::{PaneId, PaneModel, PaneSession, PtySpawnConfig};
pub use model::pane_sink::{PaneProtocolSink, PaneSinkEvent};
pub use model::split::{GridRect, SplitAxis, SplitDirection, SplitTree};
pub use model::tab::{TabId, TabModel};
pub use model::window::{WindowId, WindowModel};
pub use palette::{Command, CommandMatch, CommandRegistry, PaletteState};
pub use secure_input::SecureInputState;
pub use settings::{AppSettings, SettingsError, SettingsStore};

use mr_crabs_config::AnimationDefaults;
pub use mr_crabs_element::{CellMetrics, PixelExtent, ResizeDeduper, TerminalElement};
use mr_crabs_input::{KeyboardMode, KeyboardModeOverlay};
pub use mr_crabs_terminal::{
    CursorShape, CursorState, DamageKind, FrameDelta, FrameHyperlink, FramePoint, FramePool,
    FrameRange, FrameSearchMatch, GridSize, ImageDeltaPlaceholder, RowDelta, Run, SelectionKind,
    SelectionState, TerminalViewport,
};
use mr_crabs_terminal::{
    NormalizedSnapshot, ScrollbackConfig, Terminal, TerminalError, TerminalMode,
};

/// The S4 single-terminal core: a terminal engine plus its frame pool and
/// the animation defaults it renders with. Preserved source compatible from
/// S4; S10 adds [`AppCore::resize`] and [`AppCore::set_animation_defaults`]
/// so the shell can resize panes and apply reloaded settings.
pub struct AppCore {
    terminal: Terminal,
    animation_defaults: AnimationDefaults,
    frame_pool: FramePool,
}

impl AppCore {
    pub fn new(size: GridSize) -> Result<Self, TerminalError> {
        Ok(Self {
            terminal: Terminal::new(size)?,
            animation_defaults: AnimationDefaults::default(),
            frame_pool: FramePool::new(4),
        })
    }

    pub fn feed_terminal_output(&mut self, bytes: &[u8]) -> Result<(), TerminalError> {
        self.terminal.feed(bytes)
    }

    /// Install the pane-owned protocol sink before the first feed.
    pub fn set_protocol_sink(&mut self, sink: Box<dyn mr_crabs_protocols::sink::ProtocolSink>) {
        self.terminal.set_protocol_sink(sink);
    }

    /// Current OSC 0/2 window title.
    pub fn title(&self) -> Option<&str> {
        self.terminal.title()
    }

    /// Current OSC 7 working-directory URL.
    pub fn pwd(&self) -> Option<&str> {
        self.terminal.pwd()
    }

    /// Live OSC 133 semantic-prompt state. Additive; terminal stays private.
    pub fn semantic_state(&self) -> &mr_crabs_protocols::shell::SemanticPromptState {
        self.terminal.semantic_state()
    }
    /// Live terminal modes used by keyboard/mouse input.
    pub fn modes(&self) -> Vec<TerminalMode> {
        self.terminal.modes()
    }

    pub fn has_mode(&self, mode: TerminalMode) -> bool {
        self.terminal.has_mode(mode)
    }

    pub fn keyboard_overlay(&self) -> KeyboardModeOverlay {
        KeyboardModeOverlay {
            backarrow_key_mode: self.terminal.backarrow_key_mode(),
            ignore_keypad_with_numlock: self.terminal.ignore_keypad_with_numlock(),
            modify_other_keys_2: self.terminal.modify_other_keys_2(),
            alt_esc_prefix: self.terminal.alt_esc_prefix(),
        }
    }

    pub fn keyboard_mode(&self) -> KeyboardMode {
        KeyboardMode::from_modes_with(&self.modes(), self.keyboard_overlay())
    }

    /// Resize the terminal grid. The engine marks the frame fully damaged so
    /// the next delta repaints every row at the new size.
    pub fn resize(&mut self, size: GridSize) -> Result<(), TerminalError> {
        self.terminal.resize(size)?;
        Ok(())
    }

    /// Replace the animation defaults used when building `TerminalElement`s.
    pub fn set_animation_defaults(&mut self, defaults: AnimationDefaults) {
        self.animation_defaults = defaults;
    }

    pub fn set_default_cursor_blink(&mut self, blinking: bool) {
        self.terminal.set_default_cursor_blink(blinking);
    }

    pub fn set_scrollback_lines(&mut self, max_lines: usize) {
        self.terminal.set_scrollback_config(ScrollbackConfig {
            max_lines,
            ..ScrollbackConfig::default()
        });
    }

    pub fn terminal_snapshot(&self) -> NormalizedSnapshot {
        self.terminal.snapshot()
    }

    pub const fn animation_defaults(&self) -> AnimationDefaults {
        self.animation_defaults
    }

    /// Build the next owned `FrameDelta` from the terminal's pending damage,
    /// reusing pooled allocations. The returned frame is fully owned, so the
    /// terminal lock is never held by paint.
    pub fn build_frame_delta(&mut self) -> FrameDelta {
        self.terminal.build_frame_delta(&mut self.frame_pool)
    }

    /// Return an owned frame to the pooled allocation. Only pane retirement
    /// uses this: the caller must hand back a uniquely-owned frame.
    pub fn release_frame(&mut self, frame: FrameDelta) {
        self.frame_pool.release(frame);
    }

    pub fn blit_region(
        &mut self,
        row: u16,
        col: u16,
        size: GridSize,
        cells: &[mr_crabs_terminal::Cell],
        styles: &[mr_crabs_terminal::Style],
    ) -> Result<(), TerminalError> {
        self.terminal.blit_region(row, col, size, cells, styles)
    }

    /// Build a `TerminalElement` for the current frame. Consumes the delta
    /// without locking the terminal; the element owns the frame it paints.
    pub fn terminal_element(&mut self, metrics: CellMetrics) -> TerminalElement {
        let frame = self.build_frame_delta();
        TerminalElement::new(frame, metrics)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn app_core_creation_succeeds() {
        let core = AppCore::new(GridSize::new(80, 24)).expect("core creation");
        assert_eq!(core.terminal_snapshot().size, GridSize::new(80, 24));
        assert_eq!(core.animation_defaults(), AnimationDefaults::default());
    }

    #[test]
    fn app_core_feed_produces_pooled_full_frame() {
        let mut core = AppCore::new(GridSize::new(80, 24)).expect("core creation");
        core.feed_terminal_output(b"hi").expect("feed");
        // The engine starts fully damaged: the first build covers every row
        // and comes back through the app-owned pool.
        let frame = core.build_frame_delta();
        assert_eq!(frame.size, GridSize::new(80, 24));
        assert_eq!(frame.damage, DamageKind::Full);
        assert_eq!(frame.sequence, 0);
    }

    #[test]
    fn app_core_terminal_element_consumes_owned_delta() {
        let mut core = AppCore::new(GridSize::new(80, 24)).expect("core creation");
        core.feed_terminal_output(b"x").expect("feed");
        let metrics = CellMetrics::new(7.0, 14.0).expect("metrics");
        // Constructing the element takes the delta out of the app/terminal
        // path; nothing locks the engine afterwards.
        let element = core.terminal_element(metrics);
        drop(core);
        drop(element);
    }

    #[test]
    fn app_core_resize_marks_full_damage() {
        let mut core = AppCore::new(GridSize::new(80, 24)).expect("core creation");
        core.resize(GridSize::new(120, 40)).expect("resize");
        let frame = core.build_frame_delta();
        assert_eq!(frame.size, GridSize::new(120, 40));
        assert_eq!(frame.damage, DamageKind::Full);
    }

    #[test]
    fn app_core_set_animation_defaults_replaces_defaults() {
        let mut core = AppCore::new(GridSize::new(80, 24)).expect("core creation");
        let defaults = AnimationDefaults {
            cursor_trail: false,
            ..AnimationDefaults::default()
        };
        core.set_animation_defaults(defaults);
        assert!(!core.animation_defaults().cursor_trail);
    }
}

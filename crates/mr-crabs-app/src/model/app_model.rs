//! The application model: windows, tabs, panes, and every shell domain
//! service, with the action dispatcher.
//!
//! `AppModel` is the single owner of shell state. It is pure Rust (no GPUI
//! types), so the full keyboard-only surface is headlessly testable. PTY
//! ownership is bounded and deterministic: closing a pane/tab/window shuts
//! its sessions down with a bounded grace period, and `Drop` performs a
//! bounded best-effort shutdown of everything that remains.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use mr_crabs_protocols::sink::ClipboardEvent;
use mr_crabs_pty::OutputWake;
use mr_crabs_terminal::{FrameDelta, GridSize};

use crate::diagnostics::{
    DiagnosticEvent, DiagnosticFrameEvent, DiagnosticPumpEvent, DiagnosticTrace,
};

use crate::accessibility::AccessibilitySnapshot;
use crate::action::AppAction;
use crate::crash::CrashReporter;
use crate::dock::{DockBehavior, DockOutcome};
use crate::intent::{AppIntent, IntentOutcome, IntentRouter};
use crate::keymap::KeymapResolver;
use crate::menu::MenuModel;
use crate::palette::{CommandRegistry, PaletteState};
use crate::platform::PlatformCapabilities;
use crate::quick_terminal::QuickTerminalState;
use crate::restore::{RestoreError, RestoreStore, ShellStateV1};
use mr_crabs_config::SettingKey;

use crate::secure_input::SecureInputState;
use crate::settings::{SettingsError, SettingsStore};
use crate::updates::{UpdateCheckResult, UpdateService};

use super::geometry::SurfaceGeometry;
use super::pane::{PaneModel, PtySpawnConfig, SearchApply};
use super::split::{PaneId, SplitAxis, SplitDirection};
use super::tab::{ClosePaneOutcome, TabId, TabModel};
use super::window::{TabCloseOutcome, WindowId, WindowModel, WindowPumpStats};
/// The result of dispatching one action.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ActionResult {
    pub performed: bool,
    pub note: String,
}

impl ActionResult {
    pub fn performed(note: impl Into<String>) -> Self {
        Self {
            performed: true,
            note: note.into(),
        }
    }

    pub fn ignored(note: impl Into<String>) -> Self {
        Self {
            performed: false,
            note: note.into(),
        }
    }
}

/// Aggregate pump statistics for the whole app.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct AppPumpStats {
    pub chunks: usize,
    pub bytes: usize,
    pub frames: usize,
    pub pending: bool,
    pub error: Option<mr_crabs_terminal::TerminalError>,
}

impl AppPumpStats {
    pub fn changed(self) -> bool {
        self.chunks > 0 || self.frames > 0
    }
}

/// The application shell model.
pub struct AppModel {
    pub settings: SettingsStore,
    pub windows: BTreeMap<WindowId, WindowModel>,
    pub window_order: Vec<WindowId>,
    pub active_window: Option<WindowId>,
    pub palette: PaletteState,
    pub commands: CommandRegistry,
    pub quick_terminal: QuickTerminalState,
    pub secure_input: SecureInputState,
    pub menus: MenuModel,
    pub dock: DockBehavior,
    pub platform: PlatformCapabilities,
    pub updates: Box<dyn UpdateService>,
    pub crash: Box<dyn CrashReporter>,
    pub intents: IntentRouter,
    pub restore: RestoreStore,
    /// Bumped by every structural mutation; observed by a11y snapshots.
    pub generation: u64,
    pub quit_requested: bool,
    /// Bounded grace period used for every deterministic session shutdown.
    pub shutdown_grace: Duration,
    pub last_update_check: Option<UpdateCheckResult>,
    /// The most recent `open-url` intent payload, if any.
    pub last_open_url: Option<String>,
    /// The active search query (S8): `SearchNext`/`SearchPrevious` run
    /// against this needle on the focused pane. Empty means no query.
    pub search_query: String,
    /// Disabled-by-default bounded diagnostic ring (Step 2C1). `None` when
    /// diagnostics are off; `Some(trace)` when installed by tests.
    diagnostic_trace: Option<Arc<DiagnosticTrace>>,
    /// Event-driven notification shared by every live PTY reader. `None` for
    /// headless models and tests that pump explicit fake queues.
    output_wake: Option<OutputWake>,
    next_window: u64,
    next_tab: u64,
    next_pane: u64,
}

impl AppModel {
    /// A shell on the current platform. Production panes remain pending until
    /// measured geometry commits; headless environments build detached panes.
    pub fn new() -> Self {
        Self::with_platform_settings_and_output_wake(
            PlatformCapabilities::current(),
            SettingsStore::new(),
            None,
        )
    }

    /// A shell whose PTY readers wake the application after queueing output.
    pub fn new_with_output_wake(output_wake: OutputWake) -> Self {
        Self::with_platform_settings_and_output_wake(
            PlatformCapabilities::current(),
            SettingsStore::new(),
            Some(output_wake),
        )
    }

    /// A shell with effective startup settings and an output wake channel.
    pub fn new_with_settings_and_output_wake(
        settings: SettingsStore,
        output_wake: OutputWake,
    ) -> Self {
        Self::with_platform_settings_and_output_wake(
            PlatformCapabilities::current(),
            settings,
            Some(output_wake),
        )
    }

    /// A fully headless shell: detached panes only, no platform services.
    pub fn headless() -> Self {
        Self::with_platform_settings_and_output_wake(
            PlatformCapabilities::headless(),
            SettingsStore::new(),
            None,
        )
    }

    pub fn with_platform(platform: PlatformCapabilities) -> Self {
        Self::with_platform_settings_and_output_wake(platform, SettingsStore::new(), None)
    }

    fn with_platform_settings_and_output_wake(
        platform: PlatformCapabilities,
        settings: SettingsStore,
        output_wake: Option<OutputWake>,
    ) -> Self {
        let mut commands = CommandRegistry::new();
        commands.install_shell_commands();
        let mut model = Self {
            settings,
            windows: BTreeMap::new(),
            window_order: Vec::new(),
            active_window: None,
            palette: PaletteState::new(),
            commands,
            quick_terminal: QuickTerminalState::new(GridSize::new(80, 24)),
            secure_input: SecureInputState::new(),
            menus: MenuModel::default_shell(),
            dock: DockBehavior::default_shell(),
            platform,
            updates: Box::new(crate::updates::DisabledUpdateService {
                reason: "update checks are disabled in this build; use a LocalManifestUpdateService for local manifests".to_string(),
            }),
            crash: Box::new(crate::crash::DisabledCrashReporter {
                reason: "crash reporting is disabled in this build; use LocalFileCrashReporter for local dumps".to_string(),
            }),
            intents: IntentRouter::new_bounded(64),
            restore: RestoreStore::new(),
            generation: 0,
            quit_requested: false,
            shutdown_grace: Duration::from_millis(500),
            last_update_check: None,
            last_open_url: None,
            search_query: String::new(),
            diagnostic_trace: None,
            output_wake,
            next_window: 1,
            next_tab: 1,
            next_pane: 1,
        };
        model.new_window();
        model
    }

    // ── id allocation ──

    fn alloc_window_id(&mut self) -> WindowId {
        let id = WindowId::new(self.next_window);
        self.next_window += 1;
        id
    }

    fn alloc_tab_id(&mut self) -> TabId {
        let id = TabId::new(self.next_tab);
        self.next_tab += 1;
        id
    }

    fn alloc_pane_id(&mut self) -> PaneId {
        let id = PaneId::new(self.next_pane);
        self.next_pane += 1;
        id
    }

    /// Advance the id allocators past restored ids.
    pub fn reserve_ids(&mut self, max_window: u64, max_tab: u64, max_pane: u64) {
        self.next_window = self.next_window.max(max_window + 1);
        self.next_tab = self.next_tab.max(max_tab + 1);
        self.next_pane = self.next_pane.max(max_pane + 1);
    }

    // ── accessors ──

    pub fn window(&self, id: WindowId) -> Option<&WindowModel> {
        self.windows.get(&id)
    }

    pub fn window_mut(&mut self, id: WindowId) -> Option<&mut WindowModel> {
        self.windows.get_mut(&id)
    }

    pub fn active_window(&self) -> Option<&WindowModel> {
        self.active_window.and_then(|id| self.windows.get(&id))
    }

    pub fn active_window_mut(&mut self) -> Option<&mut WindowModel> {
        self.active_window.and_then(|id| self.windows.get_mut(&id))
    }

    pub fn set_active_window(&mut self, id: WindowId) -> bool {
        if !self.windows.contains_key(&id) {
            return false;
        }
        self.active_window = Some(id);
        true
    }

    pub fn active_tab(&self) -> Option<&TabModel> {
        self.active_window().and_then(|window| window.active_tab())
    }

    pub fn active_tab_mut(&mut self) -> Option<&mut TabModel> {
        self.active_window_mut()
            .and_then(|window| window.active_tab_mut())
    }

    /// The focused pane in the active window's active tab.
    pub fn focused_pane(&self) -> Option<&PaneModel> {
        self.active_tab().and_then(|tab| tab.focused_pane())
    }

    pub fn focused_pane_mut(&mut self) -> Option<&mut PaneModel> {
        self.active_tab_mut().and_then(TabModel::focused_pane_mut)
    }

    pub fn focused_pane_id(&self) -> Option<PaneId> {
        self.active_tab().and_then(|tab| tab.focused_pane_id())
    }

    /// The focused pane's shared frame for the given window (immutable
    /// handoff; the renderer never locks the engine).
    pub fn focused_frame(&self, window_id: WindowId) -> Option<Arc<FrameDelta>> {
        let window = self.windows.get(&window_id)?;
        let tab = window.active_tab()?;
        let pane_id = tab.focused_pane_id()?;
        tab.panes.get(&pane_id).and_then(|pane| pane.frame())
    }

    /// Locate the window and tab owning a pane.
    pub fn locate_pane(&self, pane_id: PaneId) -> Option<(WindowId, TabId)> {
        for window in self.windows.values() {
            for tab in window.tabs.values() {
                if tab.tree.contains(pane_id) {
                    return Some((window.id, tab.id));
                }
            }
        }
        None
    }

    /// The shell keymap resolver derived from the current settings.
    pub fn keymap_resolver(&self) -> KeymapResolver {
        KeymapResolver::new(self.settings.current().keybindings.clone())
    }

    pub fn should_quit(&self) -> bool {
        self.quit_requested || self.windows.is_empty()
    }

    // ── pane creation ──

    /// Create a pane, deferring a real PTY until measured geometry commits.
    fn new_pane(&mut self, size: GridSize, cwd: Option<PathBuf>) -> PaneModel {
        let id = self.alloc_pane_id();
        self.new_pane_with_id_and_cwd(id, size, cwd)
    }

    /// Create a pane with an explicit id (restore path).
    pub fn new_pane_with_id(&mut self, id: PaneId, size: GridSize) -> PaneModel {
        self.new_pane_with_id_and_cwd(id, size, None)
    }

    fn new_pane_with_id_and_cwd(
        &mut self,
        id: PaneId,
        size: GridSize,
        cwd: Option<PathBuf>,
    ) -> PaneModel {
        let settings = self.settings.current();
        if self.platform.can_spawn_pty() {
            let terminfo = settings.child_terminfo();
            let mut env = BTreeMap::new();
            if let Some(dir) = terminfo.terminfo_dir {
                env.insert("TERMINFO".to_string(), dir.display().to_string());
            }
            super::shell_integration::inject_shell_integration_env(
                &mut env,
                settings.shell.as_ref().map(PathBuf::from).as_deref(),
                settings.cursor_blink,
            );
            let config = PtySpawnConfig {
                size,
                shell: settings.shell.as_ref().map(PathBuf::from),
                cwd: cwd.or_else(|| settings.working_directory.as_ref().map(PathBuf::from)),
                env,
                term: terminfo.term,
                colorterm: terminfo.colorterm,
                scrollback_lines: settings.scrollback_lines as usize,
                startup_command: None,
            };
            if let Ok(mut pane) =
                PaneModel::pending_with_output_wake(id, config, self.output_wake.clone())
            {
                pane.core.set_default_cursor_blink(settings.cursor_blink);
                pane.core
                    .set_animation_defaults(settings.animation_defaults());
                pane.set_terminfo_name(mr_crabs_config::TERM_GHOSTTY);
                return pane;
            }
        }
        let mut pane = PaneModel::detached(id, size).expect("validated grid size");
        pane.core.set_default_cursor_blink(settings.cursor_blink);
        pane.core
            .set_animation_defaults(settings.animation_defaults());
        pane
    }

    // ── window/tab/pane construction ──

    /// Open a new window with one tab and one pane at the default grid.
    pub fn new_window(&mut self) -> Option<WindowId> {
        let size = self.settings.current().default_grid;
        self.new_window_with(size)
    }

    /// Open a new window at a specific grid.
    pub fn new_window_with(&mut self, size: GridSize) -> Option<WindowId> {
        if !size.is_valid() {
            return None;
        }
        let window_id = self.alloc_window_id();
        let tab_id = self.alloc_tab_id();
        let mut pane = self.new_pane(size, None);
        let settings = self.settings.current();
        if settings.startup_fetch && !settings.startup_fetch_command.is_empty() {
            pane.set_startup_command(Some(settings.startup_fetch_command.clone()));
        }
        let pane_id = pane.id;
        let mut window = WindowModel::new(window_id, tab_id, pane_id, size).ok()?;
        window
            .tabs
            .get_mut(&tab_id)
            .expect("tab exists")
            .panes
            .insert(pane_id, pane);
        self.windows.insert(window_id, window);
        self.window_order.push(window_id);
        self.active_window = Some(window_id);
        self.generation += 1;
        Some(window_id)
    }

    /// Open a new window with one tab whose pane spawns in `cwd` (intent
    /// path).
    pub fn new_window_with_cwd(&mut self, cwd: Option<PathBuf>) -> Option<WindowId> {
        let size = self.settings.current().default_grid;
        let window_id = self.alloc_window_id();
        let tab_id = self.alloc_tab_id();
        let mut pane = self.new_pane(size, cwd);
        let settings = self.settings.current();
        if settings.startup_fetch && !settings.startup_fetch_command.is_empty() {
            pane.set_startup_command(Some(settings.startup_fetch_command.clone()));
        }
        let pane_id = pane.id;
        let mut window = WindowModel::new(window_id, tab_id, pane_id, size).ok()?;
        window
            .tabs
            .get_mut(&tab_id)
            .expect("tab exists")
            .panes
            .insert(pane_id, pane);
        self.windows.insert(window_id, window);
        self.window_order.push(window_id);
        self.active_window = Some(window_id);
        self.generation += 1;
        Some(window_id)
    }

    /// Open a new tab in a window (or a new window when none exists).
    pub fn new_tab(&mut self, window_id: WindowId) -> Option<TabId> {
        self.new_tab_with_cwd(window_id, None)
    }

    pub fn new_tab_with_cwd(&mut self, window_id: WindowId, cwd: Option<PathBuf>) -> Option<TabId> {
        // The committed measured grid is the sizing authority; an
        // unmeasured window falls back to the settings default grid (a
        // temporary creation input until Wave 2 measures before spawn).
        let size = self
            .windows
            .get(&window_id)?
            .grid()
            .unwrap_or(self.settings.current().default_grid);
        let tab_id = self.alloc_tab_id();
        let pane = self.new_pane(size, cwd);
        let pane_id = pane.id;
        let mut tab = TabModel::new(tab_id, pane_id, size).ok()?;
        tab.panes.insert(pane_id, pane);
        self.windows.get_mut(&window_id)?.add_tab(tab);
        self.active_window = Some(window_id);
        self.generation += 1;
        Some(tab_id)
    }

    // ── focus ──

    /// Focus a pane anywhere in the shell, activating its window and tab.
    pub fn focus_pane(&mut self, pane_id: PaneId) -> bool {
        let Some((window_id, _tab_id)) = self.locate_pane(pane_id) else {
            return false;
        };
        let focused = self
            .windows
            .get_mut(&window_id)
            .is_some_and(|window| window.focus_pane(pane_id));
        if focused {
            self.active_window = Some(window_id);
            self.generation += 1;
        }
        focused
    }

    /// Activate a tab anywhere in the shell.
    pub fn activate_tab(&mut self, tab_id: TabId) -> bool {
        for window in self.windows.values_mut() {
            if window.tabs.contains_key(&tab_id) && window.set_active_tab(tab_id) {
                self.active_window = Some(window.id);
                self.generation += 1;
                return true;
            }
        }
        false
    }

    // ── closing ──

    /// Close a window and every session it owns. Sets `quit_requested`
    /// when no windows remain.
    pub fn close_window(&mut self, window_id: WindowId) -> bool {
        let Some(mut window) = self.windows.remove(&window_id) else {
            return false;
        };
        for tab in window.tabs.values_mut() {
            tab.close_all(self.shutdown_grace);
        }
        window.tabs.clear();
        self.window_order.retain(|id| *id != window_id);
        if self.active_window == Some(window_id) {
            self.active_window = self.window_order.last().copied();
        }
        self.generation += 1;
        if self.windows.is_empty() {
            self.quit_requested = true;
        }
        true
    }

    /// Close a tab anywhere in the shell, cascading to window close when it
    /// was the last tab.
    pub fn close_tab_anywhere(&mut self, window_id: WindowId, tab_id: TabId) -> bool {
        let outcome = self
            .windows
            .get_mut(&window_id)
            .map(|window| window.close_tab(tab_id, self.shutdown_grace));
        match outcome {
            Some(TabCloseOutcome::Closed(_)) => {
                self.generation += 1;
                true
            }
            Some(TabCloseOutcome::LastTabClosed) => self.close_window(window_id),
            None => false,
        }
    }

    /// Close a specific pane anywhere in the shell, cascading to tab and
    /// window close when it was the last pane/tab.
    pub fn close_pane_anywhere(&mut self, pane_id: PaneId) -> bool {
        let Some((window_id, tab_id)) = self.locate_pane(pane_id) else {
            return false;
        };
        let grace = self.shutdown_grace;
        let outcome = self
            .windows
            .get_mut(&window_id)
            .and_then(|window| window.tabs.get_mut(&tab_id))
            .map(|tab| tab.close_pane(pane_id, grace));
        match outcome {
            Some(ClosePaneOutcome::Closed(_)) => {
                self.generation += 1;
                true
            }
            Some(ClosePaneOutcome::TabEmpty) => self.close_tab_anywhere(window_id, tab_id),
            None => false,
        }
    }

    /// Deterministically shut down every remaining session (quit path).
    pub fn shutdown_all(&mut self) {
        for window in self.windows.values_mut() {
            for tab in window.tabs.values_mut() {
                tab.close_all(self.shutdown_grace);
            }
        }
    }

    /// Drain every pane's bounded reader queue. Each pane clamps its own
    /// pass to 64 chunks, and exit policy is evaluated after publication.
    pub fn pump(&mut self, cap: usize) -> AppPumpStats {
        let mut stats = AppPumpStats::default();
        for window in self.windows.values_mut() {
            let WindowPumpStats {
                chunks,
                bytes,
                frames,
                pending,
                error,
            } = window.pump(cap);
            stats.chunks += chunks;
            stats.bytes += bytes;
            stats.frames += frames;
            stats.pending |= pending;
            if stats.error.is_none() {
                stats.error = error;
            }
        }
        let close_on_exit = self.settings.current().close_on_exit;
        let mut close = Vec::new();
        for window in self.windows.values() {
            for tab in window.tabs.values() {
                for pane in tab.panes.values() {
                    if let super::pane::PtyLifecycle::Exited { status } = pane.lifecycle {
                        let should_close = match close_on_exit {
                            crate::settings::CloseOnExit::Always => true,
                            crate::settings::CloseOnExit::Clean => status.code() == Some(0),
                            crate::settings::CloseOnExit::Never => false,
                        };
                        if should_close {
                            close.push(pane.id);
                        }
                    }
                }
            }
        }
        for pane_id in close {
            self.close_pane_anywhere(pane_id);
        }
        if let Some(trace) = self.diagnostic_trace.clone() {
            trace.push(DiagnosticEvent::Pump(DiagnosticPumpEvent {
                chunks: stats.chunks,
                bytes: stats.bytes,
                frames: stats.frames,
                pending: stats.pending,
            }));
            if let Some(pane) = self.focused_pane() {
                if let Some(frame) = pane.frame() {
                    trace.push(DiagnosticEvent::Frame(DiagnosticFrameEvent {
                        pane_id: pane.id,
                        sequence: frame.sequence,
                        damage: frame.damage,
                        cursor_row: frame.cursor.row,
                        cursor_col: frame.cursor.col,
                        cursor_shape: frame.cursor.shape,
                        cursor_visible: frame.cursor.visible,
                        cursor_blinking: frame.cursor.blinking,
                        cursor_wrap_pending: frame.cursor.wrap_pending,
                        alternate_screen: frame.viewport.alternate_screen,
                    }));
                }
            }
        }
        stats
    }

    /// Install a bounded diagnostic trace (tests only). Capacity clamped to >=1.
    pub fn install_diagnostic_trace(&mut self, capacity: usize) -> Arc<DiagnosticTrace> {
        let trace = Arc::new(DiagnosticTrace::new(capacity));
        self.diagnostic_trace = Some(Arc::clone(&trace));
        trace
    }

    /// Set an externally owned diagnostic trace.
    pub fn set_diagnostic_trace(&mut self, trace: Option<Arc<DiagnosticTrace>>) {
        self.diagnostic_trace = trace;
    }

    /// Access the installed trace, if any.
    pub fn diagnostic_trace(&self) -> Option<Arc<DiagnosticTrace>> {
        self.diagnostic_trace.clone()
    }

    /// Whether any pane has output queued right now.
    pub fn any_pending_output(&mut self) -> bool {
        self.windows
            .values_mut()
            .any(|window| window.has_pending_output())
    }

    // ── input ──

    /// Write bytes to a pane's session; fails closed when the pane is
    /// detached, shut down, or the bounded queue is full. The PTY reader
    /// wakes the application when echo or command output enters its bounded
    /// queue, so successful writes do not schedule speculative frames.
    pub fn write_to_pane(&mut self, pane_id: PaneId, bytes: &[u8]) -> bool {
        let Some((window_id, tab_id)) = self.locate_pane(pane_id) else {
            return false;
        };
        let Some(window) = self.windows.get_mut(&window_id) else {
            return false;
        };
        let Some(tab) = window.tabs.get_mut(&tab_id) else {
            return false;
        };
        let Some(pane) = tab.panes.get_mut(&pane_id) else {
            return false;
        };
        if pane.session.write(bytes).is_err() {
            return false;
        }
        true
    }

    /// Drain OSC 52 requests from every pane while retaining the pane identity
    /// needed to route read replies back to the originating PTY.
    pub fn drain_clipboard_requests(&mut self) -> Vec<(PaneId, ClipboardEvent)> {
        let mut requests = Vec::new();
        for window in self.windows.values_mut() {
            for tab in window.tabs.values_mut() {
                for pane in tab.panes.values_mut() {
                    requests.extend(
                        pane.protocol_sink()
                            .drain_clipboard()
                            .into_iter()
                            .map(|event| (pane.id, event)),
                    );
                }
            }
        }
        requests
    }

    /// Commit a measured surface geometry through every split-derived pane.
    /// All pane frame swaps finish before the single generation bump.
    pub fn commit_geometry(&mut self, window_id: WindowId, geometry: SurfaceGeometry) {
        let output_wake = self.output_wake.clone();
        let changed = self
            .windows
            .get_mut(&window_id)
            .map(|window| window.set_geometry_with_output_wake(geometry, output_wake))
            .unwrap_or(0);
        if changed > 0 {
            self.generation += 1;
        }
    }

    // ── quick terminal ──

    pub fn show_quick_terminal(&mut self) {
        if self.quick_terminal.visible {
            return;
        }
        self.quick_terminal.previous_window = self.active_window;
        if let Some(window_id) = self.quick_terminal.window_id
            && let Some(window) = self.windows.get_mut(&window_id)
        {
            window.visible = true;
            self.quick_terminal.visible = true;
            self.quick_terminal.toggles += 1;
            self.quick_terminal.last_toggle = Some(std::time::Instant::now());
            self.active_window = Some(window_id);
            self.generation += 1;
            return;
        }
        if let Some(window_id) = self.new_window_with(self.quick_terminal.grid)
            && let Some(window) = self.windows.get_mut(&window_id)
        {
            for tab in window.tabs.values_mut() {
                for pane in tab.panes.values_mut() {
                    pane.set_startup_command(None);
                }
            }
            window.is_quick_terminal = true;
            window.title = "Quick Terminal".to_string();
            self.quick_terminal.window_id = Some(window_id);
            self.quick_terminal.visible = true;
            self.quick_terminal.toggles += 1;
            self.quick_terminal.last_toggle = Some(std::time::Instant::now());
            self.active_window = Some(window_id);
            self.generation += 1;
        }
    }

    pub fn hide_quick_terminal(&mut self) {
        if !self.quick_terminal.visible {
            return;
        }
        self.quick_terminal.visible = false;
        self.quick_terminal.toggles += 1;
        self.quick_terminal.last_toggle = Some(std::time::Instant::now());
        if let Some(window_id) = self.quick_terminal.window_id
            && let Some(window) = self.windows.get_mut(&window_id)
        {
            // The window hides but its sessions stay alive (Ghostty
            // behavior: the quick-terminal process persists across toggles).
            window.visible = false;
        }
        if let Some(previous) = self.quick_terminal.previous_window {
            self.active_window = Some(previous);
        }
        self.generation += 1;
    }

    /// Toggle quick-terminal visibility; returns the new visible state.
    pub fn toggle_quick_terminal(&mut self) -> bool {
        if self.quick_terminal.visible {
            self.hide_quick_terminal();
        } else {
            self.show_quick_terminal();
        }
        self.quick_terminal.visible
    }

    // ── palette ──

    /// Route one keyboard event to the open palette. `key` is the shell key
    /// name (`escape`, `enter`, `up`, `down`, `backspace`, or any other
    /// key); `text` is the printable text for character keys.
    pub fn palette_key(&mut self, key: &str, text: Option<&str>) {
        match key {
            "escape" => self.palette.close(),
            "enter" | "return" => {
                self.activate_palette_selection();
            }
            "up" => self.palette.move_selection(-1),
            "down" => self.palette.move_selection(1),
            "backspace" => self.palette.backspace(&self.commands),
            _ => {
                if let Some(text) = text {
                    for ch in text.chars() {
                        self.palette.type_char(ch, &self.commands);
                    }
                }
            }
        }
    }

    /// Dispatch the palette's selected command; closes the palette on
    /// success.
    pub fn activate_palette_selection(&mut self) -> Option<String> {
        let selected = self.palette.selected().cloned()?;
        let id = selected.id.clone();
        if self.dispatch_command(&id) {
            self.palette.last_dispatched = Some(id.clone());
            self.palette.close();
            return Some(id);
        }
        None
    }

    /// Dispatch a registered command by id.
    pub fn dispatch_command(&mut self, id: &str) -> bool {
        let Some(command) = self.commands.get(id).cloned() else {
            return false;
        };
        command.run(self);
        true
    }

    /// Set the active search query (S8). An empty query clears the search
    /// state on the focused pane on the next search dispatch.
    pub fn set_search_query(&mut self, query: impl Into<String>) {
        self.search_query = query.into();
    }

    // ── intents / dock / restore ──

    /// Dispatch an app intent, recording it in the bounded intent router.
    pub fn dispatch_intent(&mut self, intent: AppIntent, tick: u64) -> IntentOutcome {
        let outcome = self.route_intent(&intent);
        self.intents.push(intent, outcome.clone(), tick);
        outcome
    }

    fn route_intent(&mut self, intent: &AppIntent) -> IntentOutcome {
        match intent {
            AppIntent::Open { cwd, new_tab } => {
                let cwd = cwd.clone();
                if *new_tab {
                    if let Some(window_id) = self.active_window {
                        self.new_tab_with_cwd(window_id, cwd);
                    } else {
                        self.new_window_with_cwd(cwd);
                    }
                } else {
                    self.new_window_with_cwd(cwd);
                }
                IntentOutcome::Performed
            }
            AppIntent::OpenUrl { url } => {
                self.last_open_url = Some(url.clone());
                IntentOutcome::Performed
            }
            AppIntent::ReloadConfig => {
                self.dispatch(AppAction::ReloadConfig);
                IntentOutcome::Performed
            }
            AppIntent::ToggleQuickTerminal => {
                self.toggle_quick_terminal();
                IntentOutcome::Performed
            }
            AppIntent::FocusTerminal => {
                if let Some(window_id) = self.window_order.last().copied() {
                    self.active_window = Some(window_id);
                    IntentOutcome::Performed
                } else {
                    IntentOutcome::NoWindow
                }
            }
            AppIntent::Quit => {
                self.dispatch(AppAction::Quit);
                IntentOutcome::Performed
            }
        }
    }

    /// Handle a `ghostty://`-style URL: parse and route.
    pub fn handle_open_url(&mut self, url: &str, tick: u64) -> IntentOutcome {
        match AppIntent::parse_url(url) {
            Ok(intent) => self.dispatch_intent(intent, tick),
            Err(error) => IntentOutcome::Ignored(error.to_string()),
        }
    }

    /// Dock reopen behavior: create a window when policy says so (or no
    /// window exists), else activate.
    pub fn handle_reopen(&mut self) -> DockOutcome {
        if self.dock.reopen_action == crate::dock::ReopenAction::NewWindow
            || self.windows.is_empty()
        {
            match self.new_window() {
                Some(window_id) => DockOutcome::NewWindowCreated(window_id),
                None => DockOutcome::NoWindows,
            }
        } else {
            DockOutcome::Activated
        }
    }

    /// Snapshot the shell for persistence.
    pub fn restore_snapshot(&self) -> ShellStateV1 {
        self.restore.snapshot(self)
    }

    /// Restore a versioned shell state, rebuilding detached panes (real
    /// sessions are spawned when the platform supports PTYs).
    pub fn apply_restore_state(&mut self, state: ShellStateV1) -> Result<(), RestoreError> {
        let mut restore = std::mem::take(&mut self.restore);
        let result = restore.apply(self, state);
        self.restore = restore;
        result
    }

    /// A headless accessibility snapshot of the shell.
    pub fn accessibility_snapshot(&self) -> AccessibilitySnapshot {
        AccessibilitySnapshot::from_model(self)
    }

    fn apply_runtime_animation_setting(
        &mut self,
        key: SettingKey,
        value: &str,
    ) -> Result<(), SettingsError> {
        self.settings.apply_runtime_value(key, value)?;
        let animation = self.settings.current().animation_defaults();
        for window in self.windows.values_mut() {
            for tab in window.tabs.values_mut() {
                for pane in tab.panes.values_mut() {
                    pane.core.set_animation_defaults(animation);
                }
            }
        }
        self.generation += 1;
        Ok(())
    }

    pub fn toggle_chat_presentation(&mut self) -> ActionResult {
        let Some(pane_id) = self.focused_pane_id() else {
            return ActionResult::ignored("no focused pane");
        };
        let Some((window_id, tab_id)) = self.locate_pane(pane_id) else {
            return ActionResult::ignored("no focused pane");
        };
        let pane = match self
            .windows
            .get_mut(&window_id)
            .and_then(|window| window.tabs.get_mut(&tab_id))
            .and_then(|tab| tab.pane_mut(pane_id))
        {
            Some(pane) => pane,
            None => return ActionResult::ignored("no focused pane"),
        };
        let eligible = pane.is_chat_eligible(self.palette.is_open(), false);
        if !eligible {
            return ActionResult::ignored("chat not eligible on this pane");
        }
        let next = match pane.preferred_mode {
            crate::model::presentation::SurfaceMode::Terminal => {
                crate::model::presentation::SurfaceMode::Chat
            }
            crate::model::presentation::SurfaceMode::Chat => {
                crate::model::presentation::SurfaceMode::Terminal
            }
        };
        pane.preferred_mode = next;
        self.generation += 1;
        ActionResult::performed(match next {
            crate::model::presentation::SurfaceMode::Chat => "chat shown",
            crate::model::presentation::SurfaceMode::Terminal => "chat hidden",
        })
    }

    // ── dispatch ──

    /// Dispatch one shell action with full cascade semantics.
    pub fn dispatch(&mut self, action: AppAction) -> ActionResult {
        match action {
            AppAction::NewWindow => match self.new_window() {
                Some(window_id) => {
                    let note = format!("window {} opened", window_id.as_u64());
                    ActionResult::performed(note)
                }
                None => ActionResult::ignored("could not open a window"),
            },
            AppAction::CloseWindow => {
                let Some(window_id) = self.active_window else {
                    return ActionResult::ignored("no active window");
                };
                self.close_window(window_id);
                let note = if self.quit_requested {
                    "window closed; no windows remain".to_string()
                } else {
                    "window closed".to_string()
                };
                ActionResult::performed(note)
            }
            AppAction::NewTab => {
                let Some(window_id) = self.active_window else {
                    return self.dispatch(AppAction::NewWindow);
                };
                match self.new_tab(window_id) {
                    Some(tab_id) => {
                        ActionResult::performed(format!("tab {} opened", tab_id.as_u64()))
                    }
                    None => ActionResult::ignored("could not open a tab"),
                }
            }
            AppAction::CloseTab => {
                let Some(window_id) = self.active_window else {
                    return ActionResult::ignored("no active window");
                };
                let Some(tab_id) = self.active_window().and_then(|window| window.active_tab) else {
                    return ActionResult::ignored("no active tab");
                };
                let closed = self.close_tab_anywhere(window_id, tab_id);
                if closed {
                    ActionResult::performed("tab closed")
                } else {
                    ActionResult::ignored("could not close tab")
                }
            }
            AppAction::NextTab | AppAction::PreviousTab => {
                let forward = action == AppAction::NextTab;
                let Some(window_id) = self.active_window else {
                    return ActionResult::ignored("no active window");
                };
                match self
                    .windows
                    .get_mut(&window_id)
                    .and_then(|window| window.cycle_tab(forward))
                {
                    Some(tab_id) => {
                        ActionResult::performed(format!("focused tab {}", tab_id.as_u64()))
                    }
                    None => ActionResult::ignored("no tabs to cycle"),
                }
            }
            AppAction::NewSplitRight | AppAction::NewSplitDown => {
                let axis = if action == AppAction::NewSplitRight {
                    SplitAxis::Horizontal
                } else {
                    SplitAxis::Vertical
                };
                let Some(window_id) = self.active_window else {
                    return ActionResult::ignored("no active window");
                };
                // Sizing authority order: the focused pane's last measured
                // grid, then the window's committed grid, then the settings
                // default grid for an unmeasured window (never a literal
                // grid).
                let size = self
                    .active_tab()
                    .and_then(|tab| tab.focused_pane())
                    .map(|pane| pane.last_size)
                    .or_else(|| {
                        self.windows
                            .get(&window_id)
                            .and_then(|window| window.grid())
                    })
                    .unwrap_or(self.settings.current().default_grid);
                let pane = self.new_pane(size, None);
                let result = self
                    .windows
                    .get_mut(&window_id)
                    .and_then(|window| window.active_tab_mut())
                    .and_then(|tab| tab.insert_split_pane(axis, pane).ok());
                match result {
                    Some(pane_id) => {
                        ActionResult::performed(format!("split opened pane {}", pane_id.as_u64()))
                    }
                    None => ActionResult::ignored("no focused pane to split"),
                }
            }
            AppAction::ClosePane => {
                let Some(pane_id) = self.focused_pane_id() else {
                    return ActionResult::ignored("no focused pane");
                };
                if self.close_pane_anywhere(pane_id) {
                    ActionResult::performed("pane closed")
                } else {
                    ActionResult::ignored("could not close pane")
                }
            }
            AppAction::NextPane | AppAction::PreviousPane => {
                let forward = action == AppAction::NextPane;
                let Some(window_id) = self.active_window else {
                    return ActionResult::ignored("no active window");
                };
                match self
                    .windows
                    .get_mut(&window_id)
                    .and_then(|window| window.cycle_pane(forward))
                {
                    Some(pane_id) => {
                        ActionResult::performed(format!("focused pane {}", pane_id.as_u64()))
                    }
                    None => ActionResult::ignored("no pane to focus"),
                }
            }
            AppAction::GotoSplitUp
            | AppAction::GotoSplitDown
            | AppAction::GotoSplitLeft
            | AppAction::GotoSplitRight => {
                let direction = match action {
                    AppAction::GotoSplitUp => SplitDirection::Up,
                    AppAction::GotoSplitDown => SplitDirection::Down,
                    AppAction::GotoSplitLeft => SplitDirection::Left,
                    _ => SplitDirection::Right,
                };
                let Some(window_id) = self.active_window else {
                    return ActionResult::ignored("no active window");
                };
                match self
                    .windows
                    .get_mut(&window_id)
                    .and_then(|window| window.goto_split(direction))
                {
                    Some(pane_id) => {
                        ActionResult::performed(format!("focused pane {}", pane_id.as_u64()))
                    }
                    None => ActionResult::ignored("no pane in that direction"),
                }
            }
            AppAction::TogglePalette => {
                let open = self.palette.toggle(&self.commands);
                ActionResult::performed(if open {
                    "palette opened"
                } else {
                    "palette closed"
                })
            }
            AppAction::ToggleQuickTerminal => {
                let visible = self.toggle_quick_terminal();
                ActionResult::performed(if visible {
                    "quick terminal shown"
                } else {
                    "quick terminal hidden"
                })
            }
            AppAction::ToggleSecureInput => {
                let enabled = self.secure_input.toggle();
                if enabled {
                    if let Some(pane_id) = self.focused_pane_id() {
                        self.secure_input.track_pane(pane_id);
                    }
                } else {
                    self.secure_input.clear_panes();
                }
                ActionResult::performed(if enabled {
                    "secure input enabled"
                } else {
                    "secure input disabled"
                })
            }
            AppAction::ReloadConfig => match self.settings.reload_from_source() {
                Ok(()) => {
                    let settings = self.settings.current();
                    let animation = settings.animation_defaults();
                    for window in self.windows.values_mut() {
                        for tab in window.tabs.values_mut() {
                            for pane in tab.panes.values_mut() {
                                pane.core.set_animation_defaults(animation);
                                pane.core.set_default_cursor_blink(settings.cursor_blink);
                                pane.core
                                    .set_scrollback_lines(settings.scrollback_lines as usize);
                            }
                        }
                    }
                    self.generation += 1;
                    ActionResult::performed("configuration reloaded")
                }
                Err(SettingsError::NoSource) => {
                    ActionResult::ignored("no config source configured")
                }
                Err(error) => ActionResult::ignored(format!("config reload skipped: {error}")),
            },
            AppAction::CheckForUpdates => {
                let result = self.updates.check();
                let note = match &result.status {
                    crate::updates::UpdateStatus::Disabled { reason } => {
                        format!("updates disabled: {reason}")
                    }
                    crate::updates::UpdateStatus::UpToDate { version } => {
                        format!("up to date ({version})")
                    }
                    crate::updates::UpdateStatus::UpdateAvailable { version, notes } => {
                        format!("update available: {version} ({notes})")
                    }
                };
                self.last_update_check = Some(result);
                ActionResult::performed(note)
            }
            AppAction::SearchNext | AppAction::SearchPrevious => {
                let forward = action == AppAction::SearchNext;
                let Some(pane_id) = self.focused_pane_id() else {
                    return ActionResult::ignored("no focused pane");
                };
                let Some((window_id, tab_id)) = self.locate_pane(pane_id) else {
                    return ActionResult::ignored("no focused pane");
                };
                let needle = self.search_query.clone();
                let outcome = self
                    .windows
                    .get_mut(&window_id)
                    .and_then(|window| window.tabs.get_mut(&tab_id))
                    .and_then(|tab| tab.panes.get_mut(&pane_id))
                    .map(|pane| pane.search(needle.as_bytes(), forward));
                match outcome {
                    Some(SearchApply::Selected { line, col }) => ActionResult::performed(format!(
                        "search {}: match at line {line}, col {col}",
                        if forward { "next" } else { "previous" }
                    )),
                    Some(SearchApply::Searching) => {
                        if let Some(wake) = &self.output_wake {
                            wake();
                        }
                        ActionResult::performed("searching")
                    }
                    Some(SearchApply::NoMatch) => {
                        ActionResult::ignored("no further match for the search query")
                    }
                    Some(SearchApply::NoNeedle) | None => {
                        ActionResult::ignored("no search query set")
                    }
                }
            }
            AppAction::SetTextAnimationNone => {
                match self.apply_runtime_animation_setting(SettingKey::TextAnimation, "none") {
                    Ok(()) => ActionResult::performed("text animation set to none"),
                    Err(error) => {
                        ActionResult::ignored(format!("animation setting update failed: {error}"))
                    }
                }
            }
            AppAction::SetTextAnimationStreaming => {
                match self.apply_runtime_animation_setting(SettingKey::TextAnimation, "streaming") {
                    Ok(()) => ActionResult::performed("text animation set to streaming"),
                    Err(error) => {
                        ActionResult::ignored(format!("animation setting update failed: {error}"))
                    }
                }
            }
            AppAction::SetTextAnimationTypewriter => {
                match self.apply_runtime_animation_setting(SettingKey::TextAnimation, "typewriter")
                {
                    Ok(()) => ActionResult::performed("text animation set to typewriter"),
                    Err(error) => {
                        ActionResult::ignored(format!("animation setting update failed: {error}"))
                    }
                }
            }
            AppAction::ToggleCursorTrail => {
                let enabled = !self.settings.current().cursor_trail;
                let value = if enabled { "true" } else { "false" };
                match self.apply_runtime_animation_setting(SettingKey::CursorTrail, value) {
                    Ok(()) => ActionResult::performed(if enabled {
                        "cursor trail enabled"
                    } else {
                        "cursor trail disabled"
                    }),
                    Err(error) => {
                        ActionResult::ignored(format!("animation setting update failed: {error}"))
                    }
                }
            }
            AppAction::ToggleChatPresentation => self.toggle_chat_presentation(),
            AppAction::Quit => {
                self.quit_requested = true;
                self.shutdown_all();
                ActionResult::performed("quit requested; all sessions shut down")
            }
        }
    }
}

impl Default for AppModel {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for AppModel {
    fn drop(&mut self) {
        // Bounded best-effort fallback so a dropped shell never leaks
        // children; explicit close paths already shut sessions down.
        self.shutdown_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::geometry::PaddingPx;
    use mr_crabs_element::{CellMetrics, PixelExtent};

    fn headless() -> AppModel {
        AppModel::headless()
    }

    #[test]
    fn headless_shell_starts_with_one_window_one_tab_one_pane() {
        let model = headless();
        assert_eq!(model.windows.len(), 1);
        assert_eq!(model.window_order.len(), 1);
        assert!(model.active_window.is_some());
        let window = model.active_window().unwrap();
        assert_eq!(window.tabs.len(), 1);
        assert_eq!(window.tabs.values().next().unwrap().pane_count(), 1);
        assert!(!model.should_quit());
        assert_eq!(model.platform.kind, crate::platform::PlatformKind::Headless);
        // Detached panes: no children, but fully modelable.
        assert!(model.focused_pane().unwrap().session.is_detached());
    }

    #[test]
    fn keyboard_only_workflow_through_the_default_keymap() {
        let mut model = headless();
        let resolver = model.keymap_resolver();

        // cmd+t: new tab.
        let action = resolver.resolve("cmd+t", "").expect("binding");
        model.dispatch(action);
        let window = model.active_window().unwrap();
        assert_eq!(window.tabs.len(), 2);

        // cmd+d: split right.
        model.dispatch(resolver.resolve("cmd+d", "").expect("binding"));
        let tab = model.active_tab().unwrap();
        assert_eq!(tab.pane_count(), 2);
        let focused_after_split = tab.focused_pane_id().unwrap();

        // ctrl+cmd+left: move focus left.
        model.dispatch(resolver.resolve("ctrl+cmd+left", "").expect("binding"));
        let tab = model.active_tab().unwrap();
        assert_ne!(tab.focused_pane_id().unwrap(), focused_after_split);

        // ctrl+cmd+right: move focus right again.
        model.dispatch(resolver.resolve("ctrl+cmd+right", "").expect("binding"));
        assert_eq!(
            model.active_tab().unwrap().focused_pane_id(),
            Some(focused_after_split)
        );

        // cmd+w: close the focused pane (cascades to tab/window when last).
        model.dispatch(resolver.resolve("cmd+w", "").expect("binding"));
        let tab = model.active_tab().unwrap();
        assert_eq!(tab.pane_count(), 1);

        // cmd+shift+p: palette.
        model.dispatch(resolver.resolve("cmd+shift+p", "").expect("binding"));
        assert!(model.palette.is_open());

        // cmd+q: quit shuts everything down deterministically.
        model.dispatch(resolver.resolve("cmd+q", "").expect("binding"));
        assert!(model.should_quit());
        for window in model.windows.values() {
            for tab in window.tabs.values() {
                for pane in tab.panes.values() {
                    assert!(pane.session.is_shut_down());
                }
            }
        }
    }

    #[test]
    fn new_tab_adds_and_focuses() {
        let mut model = headless();
        let window_id = model.active_window.unwrap();
        let tab_id = model.new_tab(window_id).expect("tab");
        let window = model.active_window().unwrap();
        assert_eq!(window.tabs.len(), 2);
        assert_eq!(window.active_tab().unwrap().id, tab_id);
        assert!(model.focused_pane().is_some());
    }

    #[test]
    fn close_tab_cascades_to_window_close_and_quit() {
        let mut model = headless();
        let window_id = model.active_window.unwrap();
        let tab_id = model.new_tab(window_id).expect("tab");
        // Closing the second tab leaves the first.
        assert!(model.close_tab_anywhere(window_id, tab_id));
        assert_eq!(model.windows.len(), 1);
        assert!(!model.should_quit());
        // Closing the last tab closes the window, which requests quit.
        let last_tab = model.active_window().unwrap().active_tab.unwrap();
        assert!(model.close_tab_anywhere(window_id, last_tab));
        assert!(model.windows.is_empty());
        assert!(model.should_quit());
    }

    #[test]
    fn close_pane_cascades_through_tab_to_window() {
        let mut model = headless();
        model.dispatch(AppAction::NewSplitRight);
        let pane_count = model.active_tab().unwrap().pane_count();
        assert_eq!(pane_count, 2);
        let focused = model.focused_pane_id().unwrap();
        assert!(model.close_pane_anywhere(focused));
        assert_eq!(model.active_tab().unwrap().pane_count(), 1);
        // Closing the last pane closes the tab and then the window.
        let last_pane = model.focused_pane_id().unwrap();
        assert!(model.close_pane_anywhere(last_pane));
        assert!(model.windows.is_empty());
        assert!(model.should_quit());
    }

    #[test]
    fn commit_geometry_updates_all_panes_and_dedupes() {
        let mut model = headless();
        model.dispatch(AppAction::NewSplitRight);
        let window_id = model.active_window.unwrap();
        let geometry = SurfaceGeometry::from_viewport(
            PixelExtent {
                width: 960.0,
                height: 640.0,
            },
            CellMetrics::new(8.0, 16.0).expect("metrics"),
            PaddingPx::default(),
        )
        .expect("measured geometry");
        assert_eq!(geometry.grid, GridSize::new(120, 40));
        model.commit_geometry(window_id, geometry);
        // Both split panes receive their split-derived rect grids (60x40
        // each at ratio 0.5), never the raw window grid.
        for tab in model.windows.get(&window_id).unwrap().tabs.values() {
            for pane in tab.panes.values() {
                assert_eq!(pane.last_size, GridSize::new(60, 40));
            }
        }
        let generation = model.generation;
        model.commit_geometry(window_id, geometry);
        assert_eq!(model.generation, generation, "identical resize is a no-op");
    }

    #[test]
    fn quick_terminal_toggle_persists_session_and_window() {
        let mut model = AppModel::with_platform(PlatformCapabilities::current());
        assert!(!model.quick_terminal.is_visible());
        let original_window = model.active_window.unwrap();
        let original_pane = model
            .window(original_window)
            .expect("normal window")
            .active_tab()
            .expect("normal tab")
            .panes
            .values()
            .next()
            .expect("normal pane");
        assert_eq!(
            original_pane.pending_startup_command(),
            Some(mr_crabs_config::DEFAULT_STARTUP_FETCH_COMMAND),
            "normal terminals keep the default startup fetch"
        );

        assert!(model.toggle_quick_terminal());
        assert!(model.quick_terminal.is_visible());
        let quick_id = model.quick_terminal.window().expect("quick window");
        assert!(model.window(quick_id).expect("window").is_quick_terminal);
        let quick_pane = model
            .window(quick_id)
            .expect("quick window")
            .active_tab()
            .expect("quick tab")
            .panes
            .values()
            .next()
            .expect("quick pane");
        assert_eq!(
            quick_pane.pending_startup_command(),
            None,
            "Quick Terminal must suppress startup fetch"
        );
        assert!(model.window(quick_id).expect("window").visible);
        assert_eq!(model.active_window, Some(quick_id));

        // Hiding keeps the session alive (Ghostty quick-terminal behavior).
        let pane_id = model
            .window(quick_id)
            .expect("window")
            .active_tab()
            .unwrap()
            .focused_pane_id()
            .unwrap();
        assert!(!model.toggle_quick_terminal());
        assert!(!model.quick_terminal.is_visible());
        assert!(!model.window(quick_id).expect("window").visible);
        assert!(
            !model
                .window(quick_id)
                .expect("window")
                .active_tab()
                .unwrap()
                .panes
                .get(&pane_id)
                .unwrap()
                .session
                .is_shut_down(),
            "quick terminal session survives hiding"
        );
        assert_eq!(
            model.active_window,
            Some(original_window),
            "focus returns to the previous window"
        );

        // Re-showing reuses the same window id and session.
        assert!(model.toggle_quick_terminal());
        assert_eq!(model.quick_terminal.window(), Some(quick_id));
        assert_eq!(model.quick_terminal.toggles, 3);
    }

    #[test]
    fn secure_input_tracks_the_focused_pane() {
        let mut model = headless();
        assert!(!model.secure_input.is_enabled());
        model.dispatch(AppAction::ToggleSecureInput);
        assert!(model.secure_input.is_enabled());
        let focused = model.focused_pane_id().unwrap();
        assert!(model.secure_input.is_tracking(focused));
        model.dispatch(AppAction::ToggleSecureInput);
        assert!(!model.secure_input.is_enabled());
        assert!(model.secure_input.tracked_panes().is_empty());
    }

    #[test]
    fn check_updates_records_a_result_without_network() {
        let mut model = headless();
        assert!(model.last_update_check.is_none());
        model.dispatch(AppAction::CheckForUpdates);
        let result = model.last_update_check.as_ref().expect("recorded");
        assert!(matches!(
            result.status,
            crate::updates::UpdateStatus::Disabled { .. }
        ));
    }

    #[test]
    fn search_commands_dispatch_through_the_keymap() {
        let mut model = headless();
        model.set_search_query("alpha");
        {
            let pane_id = model.focused_pane_id().unwrap();
            let tab = model.active_tab_mut().unwrap();
            tab.panes
                .get_mut(&pane_id)
                .unwrap()
                .feed_test_output(b"alpha\r\nbeta\r\nalpha\r\n")
                .expect("app_model fixture feed should succeed");
        }
        let resolver = model.keymap_resolver();
        // Search-next starts at the most recent match (line 2).
        let result = model.dispatch(resolver.resolve("cmd+shift+g", "").expect("binding"));
        assert!(result.performed, "{}", result.note);
        let frame = model
            .focused_frame(model.active_window.unwrap())
            .expect("frame");
        assert!(frame.selection.active);
        assert_eq!(frame.selection.start, Some((2, 0)));
        assert_eq!(frame.selection.end, Some((2, 5)));

        // Next again advances to the older match.
        let result = model.dispatch(resolver.resolve("cmd+shift+g", "").expect("binding"));
        assert!(result.performed, "{}", result.note);
        let frame = model
            .focused_frame(model.active_window.unwrap())
            .expect("frame");
        assert_eq!(frame.selection.start, Some((0, 0)));

        // Previous wraps back toward the newer match.
        let result = model.dispatch(resolver.resolve("cmd+shift+h", "").expect("binding"));
        assert!(result.performed, "{}", result.note);
        let frame = model
            .focused_frame(model.active_window.unwrap())
            .expect("frame");
        assert_eq!(frame.selection.start, Some((2, 0)));
    }

    #[test]
    fn search_commands_require_a_query_and_are_registered_in_the_palette() {
        let mut model = headless();
        // Palette registration for every action, including the search pair.
        assert!(model.commands.contains("shell.search_next"));
        assert!(model.commands.contains("shell.search_previous"));
        // Without a query the command reports ignored.
        let result = model.dispatch(AppAction::SearchNext);
        assert!(!result.performed);
        assert_eq!(result.note, "no search query set");
        // An empty query clears the previous selection.
        model.set_search_query("alpha");
        {
            let pane_id = model.focused_pane_id().unwrap();
            let tab = model.active_tab_mut().unwrap();
            tab.panes
                .get_mut(&pane_id)
                .unwrap()
                .feed_test_output(b"alpha\n")
                .expect("app_model fixture feed should succeed");
        }
        assert!(model.dispatch(AppAction::SearchNext).performed);
        model.set_search_query("");
        let result = model.dispatch(AppAction::SearchNext);
        assert!(!result.performed);
        let frame = model
            .focused_frame(model.active_window.unwrap())
            .expect("frame");
        assert!(!frame.selection.active);
    }

    #[test]
    fn quit_shuts_down_every_session() {
        let mut model = headless();
        model.dispatch(AppAction::NewSplitRight);
        model.dispatch(AppAction::Quit);
        assert!(model.should_quit());
        for window in model.windows.values() {
            for tab in window.tabs.values() {
                for pane in tab.panes.values() {
                    assert!(pane.session.is_shut_down());
                }
            }
        }
    }

    #[test]
    fn intent_open_with_new_tab_creates_a_tab() {
        let mut model = headless();
        let tabs_before = model.active_window().unwrap().tabs.len();
        let outcome = model.handle_open_url("ghostty://open?tab=new&cwd=%2Ftmp", 1);
        assert_eq!(outcome, IntentOutcome::Performed);
        assert_eq!(model.active_window().unwrap().tabs.len(), tabs_before + 1);
        assert_eq!(model.intents.records().len(), 1);
        let record = &model.intents.records()[0];
        assert_eq!(record.at, 1);
        assert!(matches!(
            record.intent,
            AppIntent::Open { new_tab: true, .. }
        ));
    }

    #[test]
    fn dock_reopen_creates_a_window_when_none_exists() {
        let mut model = headless();
        let window_id = model.active_window.unwrap();
        model.close_window(window_id);
        assert!(model.windows.is_empty());
        match model.handle_reopen() {
            DockOutcome::NewWindowCreated(_) => {}
            other => panic!("expected a new window, got {other:?}"),
        }
        assert_eq!(model.windows.len(), 1);
    }

    #[test]
    fn restore_round_trip_preserves_structure_and_focus() {
        let mut model = headless();
        model.dispatch(AppAction::NewSplitRight);
        model.dispatch(AppAction::NewTab);
        model.dispatch(AppAction::NewSplitDown);
        let _window_id = model.active_window.unwrap();
        model.dispatch(AppAction::GotoSplitLeft);

        let state = model.restore_snapshot();
        assert_eq!(state.version, 1);

        let mut restored = AppModel::headless();
        restored.apply_restore_state(state).expect("restore");
        assert_eq!(restored.windows.len(), 1);
        let original = model.active_window().unwrap();
        let restored_window = restored.active_window().unwrap();
        assert_eq!(restored_window.tabs.len(), original.tabs.len());
        assert_eq!(restored_window.tab_order.len(), original.tab_order.len());
        for (tab_id, original_tab) in &original.tabs {
            let restored_tab = restored_window.tabs.get(tab_id).expect("tab restored");
            assert_eq!(restored_tab.pane_count(), original_tab.pane_count());
            assert_eq!(restored_tab.tree, original_tab.tree);
            assert_eq!(
                restored_tab.focused_pane_id(),
                original_tab.focused_pane_id()
            );
        }
        assert_eq!(
            restored
                .active_window()
                .unwrap()
                .active_tab()
                .unwrap()
                .focused_pane_id(),
            original.active_tab().unwrap().focused_pane_id()
        );
    }

    #[test]
    fn restore_rejects_unsupported_versions() {
        let model = headless();
        let mut state = model.restore_snapshot();
        state.version = 99;
        let mut restored = AppModel::headless();
        assert_eq!(
            restored.apply_restore_state(state),
            Err(RestoreError::UnsupportedVersion(99))
        );
    }

    #[test]
    fn write_to_pane_fails_closed_on_detached_sessions() {
        let mut model = headless();
        let pane_id = model.focused_pane_id().unwrap();
        assert!(
            !model.write_to_pane(pane_id, b"x"),
            "detached sessions refuse writes"
        );
        assert!(
            !model.write_to_pane(PaneId::new(9999), b"x"),
            "unknown panes refuse writes"
        );
    }

    // ── event-driven output publication ──

    /// Install a deterministic fake session on the focused pane: bounded
    /// writer and reader queues owned by the test, so `write_to_pane`
    /// succeeds and the echo can be fed later.
    fn install_fake_session(
        model: &mut AppModel,
    ) -> (
        std::sync::mpsc::SyncSender<Vec<u8>>,
        std::sync::mpsc::Receiver<Vec<u8>>,
    ) {
        let (reader_tx, reader_rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(8);
        let (writer_tx, writer_rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(8);
        let session = crate::model::pane::PaneSession::from_receivers_with_writer(
            GridSize::new(80, 24),
            Some(reader_rx),
            None,
            Some(writer_tx),
        );
        let pane_id = model.focused_pane_id().expect("focused pane");
        model
            .active_tab_mut()
            .expect("active tab")
            .panes
            .get_mut(&pane_id)
            .expect("pane")
            .session = session;
        (reader_tx, writer_rx)
    }

    #[test]
    fn chat_toggle_preserves_live_writer_and_scrollback() {
        let mut model = headless();
        let window_id = model.active_window.expect("window");
        let pane_id = model.focused_pane_id().expect("focused pane");
        let (reader_tx, writer_rx) = install_fake_session(&mut model);
        reader_tx
            .send(b"\x1b]133;A\x07existing output".to_vec())
            .expect("feed semantic output");
        assert!(model.pump(64).changed());
        let sequence_before = model
            .focused_frame(window_id)
            .expect("focused frame")
            .sequence;

        assert!(model.dispatch(AppAction::ToggleChatPresentation).performed);
        assert!(model.write_to_pane(pane_id, b"after-toggle"));
        assert_eq!(writer_rx.try_recv(), Ok(b"after-toggle".to_vec()));
        assert_eq!(
            model
                .focused_frame(window_id)
                .expect("focused frame")
                .sequence,
            sequence_before
        );
    }

    #[test]
    fn queued_echo_publishes_text_and_cursor_without_speculative_polling() {
        let mut model = headless();
        let window_id = model.active_window.expect("window");
        let pane_id = model.focused_pane_id().expect("focused pane");
        let (reader_tx, writer_rx) = install_fake_session(&mut model);

        assert!(model.write_to_pane(pane_id, b"hi"));
        assert_eq!(writer_rx.try_recv(), Ok(b"hi".to_vec()));
        assert_eq!(
            model.pump(64),
            AppPumpStats::default(),
            "a write without queued output creates no redraw work"
        );

        reader_tx.send(b"hi".to_vec()).expect("feed echo");
        let stats = model.pump(64);
        assert!(stats.changed());
        assert_eq!(stats.frames, 1);
        assert!(!stats.pending);
        let frame = model.focused_frame(window_id).expect("frame");
        assert_eq!(frame.cursor.col, 2, "cursor advances with echoed text");

        assert!(model.write_to_pane(pane_id, b"\x1b[D"));
        assert_eq!(writer_rx.try_recv(), Ok(b"\x1b[D".to_vec()));
        assert_eq!(model.pump(64), AppPumpStats::default());
        reader_tx.send(b"\x1b[D".to_vec()).expect("feed echo");
        assert!(model.pump(64).changed());
        let frame = model.focused_frame(window_id).expect("frame");
        assert_eq!(frame.cursor.col, 1, "left arrow moves the cursor");

        assert!(model.write_to_pane(pane_id, b"\x1b[C"));
        assert_eq!(writer_rx.try_recv(), Ok(b"\x1b[C".to_vec()));
        assert_eq!(model.pump(64), AppPumpStats::default());
        reader_tx.send(b"\x1b[C".to_vec()).expect("feed echo");
        assert!(model.pump(64).changed());
        let frame = model.focused_frame(window_id).expect("frame");
        assert_eq!(frame.cursor.col, 2, "right arrow moves the cursor");
        assert_eq!(model.pump(64), AppPumpStats::default());
    }

    #[test]
    fn split_resize_via_dispatch_changes_ratios() {
        let mut model = headless();
        model.dispatch(AppAction::NewSplitRight);
        let window_id = model.active_window.unwrap();
        // Commit explicit measured geometry so the split rects derive from
        // a real surface rather than an unmeasured guess.
        let geometry = SurfaceGeometry::from_viewport(
            PixelExtent {
                width: 960.0,
                height: 640.0,
            },
            CellMetrics::new(8.0, 16.0).expect("metrics"),
            PaddingPx::default(),
        )
        .expect("measured geometry");
        model.commit_geometry(window_id, geometry);
        let grid = model
            .window(window_id)
            .unwrap()
            .grid()
            .expect("measured test geometry");
        let before = model.active_tab().unwrap().rects(grid);
        let second = PaneId::new(2);
        let before_width = before[&second].width;
        // GotoSplitLeft focuses the first pane, then grow it left.
        model.dispatch(AppAction::GotoSplitLeft);
        model.dispatch(AppAction::GotoSplitLeft); // no-op direction guard: no pane above
        let tab = model.active_tab_mut().unwrap();
        assert!(tab.resize_split(SplitDirection::Left, 0.2));
        let after = tab.rects(grid);
        assert!(after[&second].width < before_width, "second pane shrank");
    }

    #[test]
    fn focused_frame_is_an_immutable_arc() {
        let mut model = headless();
        let window_id = model.active_window.unwrap();
        let pane_id = model.focused_pane_id().unwrap();
        model
            .active_tab_mut()
            .unwrap()
            .panes
            .get_mut(&pane_id)
            .unwrap()
            .feed_test_output(b"hi")
            .expect("app_model fixture feed should succeed");
        let frame = model.focused_frame(window_id).expect("frame");
        assert_eq!(frame.size, GridSize::new(80, 24));
        // The frame is shared, not rebuilt by reading.
        let frame2 = model.focused_frame(window_id).expect("frame");
        assert!(Arc::ptr_eq(&frame, &frame2));
    }

    #[test]
    fn osc52_requests_retain_the_originating_pane() {
        let mut model = headless();
        let first = model.focused_pane_id().expect("first pane");
        model.dispatch(AppAction::NewSplitRight);
        let second = model.focused_pane_id().expect("second pane");
        assert_ne!(first, second);

        let tab = model.active_tab_mut().expect("active tab");
        tab.panes
            .get_mut(&first)
            .expect("first")
            .feed_test_output(b"\x1b]52;c;?\x1b\\")
            .expect("app_model fixture feed should succeed");
        tab.panes
            .get_mut(&second)
            .expect("second")
            .feed_test_output(b"\x1b]52;c;c2Vjb25k\x1b\\")
            .expect("app_model fixture feed should succeed");

        let requests = model.drain_clipboard_requests();
        assert_eq!(requests.len(), 2);
        assert_eq!(requests[0].0, first);
        assert_eq!(requests[0].1.kind, b'c');
        assert_eq!(requests[0].1.data, b"?");
        assert_eq!(requests[1].0, second);
        assert_eq!(requests[1].1.kind, b'c');
        assert_eq!(requests[1].1.data, b"c2Vjb25k");
    }
    // ── diagnostic trace (Step 2C1) ──

    #[test]
    fn default_app_has_no_trace() {
        let model = headless();
        assert!(model.diagnostic_trace().is_none());
    }

    #[test]
    fn trace_capacity_clamped_eviction_and_order_via_app() {
        let mut model = headless();
        let trace = model.install_diagnostic_trace(0);
        assert_eq!(trace.capacity(), 1);
        assert!(trace.is_empty());
        // With no frame yet, each pump records only Pump.
        model.pump(8);
        let snap = trace.snapshot();
        assert_eq!(snap.len(), 1);
        assert!(snap[0].as_pump().is_some());
        // snapshot does not drain
        assert_eq!(trace.len(), 1);
        assert_eq!(trace.snapshot().len(), 1);
        model.pump(8);
        assert_eq!(trace.snapshot().len(), 1);

        // With a frame present, each pump records Pump+Frame.
        let mut model2 = headless();
        // Ensure a frame exists by feeding output before installing trace.
        {
            let pid = model2.focused_pane_id().unwrap();
            model2
                .active_tab_mut()
                .unwrap()
                .panes
                .get_mut(&pid)
                .unwrap()
                .feed_test_output(b"x")
                .expect("app_model fixture feed should succeed");
        }
        let trace2 = model2.install_diagnostic_trace(4);
        model2.pump(8);
        model2.pump(8);
        // Two pumps => 4 events (Pump,Frame,Pump,Frame) exactly fills cap 4
        let snap2 = trace2.snapshot();
        assert_eq!(snap2.len(), 4);
        assert!(snap2[0].as_pump().is_some());
        assert!(snap2[1].as_frame().is_some());
        assert!(snap2[2].as_pump().is_some());
        assert!(snap2[3].as_frame().is_some());
        // Nondraining check
        assert_eq!(trace2.snapshot(), snap2);
        assert_eq!(trace2.len(), 4);
        // One more pump evicts oldest two
        model2.pump(8);
        let snap3 = trace2.snapshot();
        assert_eq!(snap3.len(), 4);
        assert!(
            snap3[0].as_pump().is_some(),
            "oldest pump evicted correctly"
        );
    }

    #[test]
    fn installed_trace_records_fake_queue_pump_fields_and_cursor() {
        let mut model = headless();
        let _window_id = model.active_window.unwrap();
        let pane_id = model.focused_pane_id().unwrap();
        // Install fake session with bounded reader queue to generate real pump stats
        let (reader_tx, _writer_rx) = {
            let (reader_tx, reader_rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(8);
            let (writer_tx, writer_rx) = std::sync::mpsc::sync_channel::<Vec<u8>>(8);
            let session = crate::model::pane::PaneSession::from_receivers_with_writer(
                GridSize::new(80, 24),
                Some(reader_rx),
                None,
                Some(writer_tx),
            );
            model
                .active_tab_mut()
                .unwrap()
                .panes
                .get_mut(&pane_id)
                .unwrap()
                .session = session;
            (reader_tx, writer_rx)
        };
        let trace = model.install_diagnostic_trace(16);
        // Queue bytes that move cursor
        reader_tx.send(b"hello".to_vec()).expect("send");
        let stats = model.pump(8);
        assert!(stats.changed());
        assert_eq!(stats.chunks, 1);
        assert_eq!(stats.bytes, 5);
        assert_eq!(stats.frames, 1);
        assert!(!stats.pending);

        let snap = trace.snapshot();
        assert_eq!(snap.len(), 2);
        let pump_ev = snap[0].as_pump().unwrap();
        assert_eq!(pump_ev.chunks, 1);
        assert_eq!(pump_ev.bytes, 5);
        assert_eq!(pump_ev.frames, 1);
        assert!(!pump_ev.pending);
        assert!(pump_ev.changed());

        let frame_ev = snap[1].as_frame().unwrap();
        assert_eq!(frame_ev.pane_id, pane_id);
        assert_eq!(frame_ev.cursor_col, 5);
        assert_eq!(frame_ev.cursor_row, 0);
        assert!(!frame_ev.cursor_wrap_pending);
        assert!(!frame_ev.alternate_screen);
        assert!(
            frame_ev.damage == mr_crabs_terminal::DamageKind::Partial
                || frame_ev.damage == mr_crabs_terminal::DamageKind::Full,
            "initial pump may be Full or Partial, got {:?}",
            frame_ev.damage
        );

        // Alternate screen flag propagated
        model
            .active_tab_mut()
            .unwrap()
            .panes
            .get_mut(&pane_id)
            .unwrap()
            .feed_test_output(b"\x1b[?1049h")
            .expect("app_model fixture feed should succeed");
        let _stats2 = model.pump(8);
        let snap2 = trace.snapshot();
        let last_frame = snap2
            .iter()
            .filter_map(|e| e.as_frame())
            .next_back()
            .unwrap();
        assert!(
            last_frame.alternate_screen,
            "alternate screen flag propagated"
        );
    }

    #[test]
    fn set_diagnostic_trace_removal_stops_recording() {
        let mut model = headless();
        let external = std::sync::Arc::new(crate::diagnostics::DiagnosticTrace::new(4));
        model.set_diagnostic_trace(Some(std::sync::Arc::clone(&external)));
        assert!(model.diagnostic_trace().is_some());
        model.pump(8);
        // No frame yet: only Pump event
        assert_eq!(external.snapshot().len(), 1);
        assert!(external.snapshot()[0].as_pump().is_some());
        model.set_diagnostic_trace(None);
        assert!(model.diagnostic_trace().is_none());
        let before = external.snapshot().len();
        model.pump(8);
        assert_eq!(
            external.snapshot().len(),
            before,
            "removed trace must not receive events"
        );
    }

    #[test]
    fn startup_fetch_gui_boot_marker_appears_via_current_platform_pump() {
        use mr_crabs_config::{ConfigOverlay, SettingKey};

        let mut overlay = ConfigOverlay::default();
        overlay
            .set(SettingKey::StartupFetch, "true")
            .expect("startup_fetch");
        overlay
            .set(SettingKey::StartupFetchCommand, "printf GUIBOOT")
            .expect("command");
        let store = crate::settings::SettingsStore::from_layers(
            overlay,
            ConfigOverlay::default(),
            ConfigOverlay::default(),
            None,
            None,
            crate::settings::SettingsSource::Json("{}".to_string()),
            None,
        );
        let mut model = AppModel::with_platform_settings_and_output_wake(
            crate::platform::PlatformCapabilities::current(),
            store,
            None,
        );
        let window_id = model.new_window().expect("window");
        let geometry = crate::model::geometry::SurfaceGeometry::from_viewport(
            PixelExtent {
                width: 1000.0,
                height: 600.0,
            },
            CellMetrics::new(10.0, 20.0).expect("metrics"),
            PaddingPx::default(),
        )
        .expect("geometry");
        model.commit_geometry(window_id, geometry);
        let pane_id = model.focused_pane_id().expect("pane");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        loop {
            let found = {
                let pane = model
                    .windows
                    .get(&window_id)
                    .unwrap()
                    .tabs
                    .values()
                    .next()
                    .unwrap()
                    .panes
                    .get(&pane_id)
                    .unwrap();
                let snap = pane.core.terminal_snapshot();
                let cols = usize::from(snap.size.cols);
                let mut f = false;
                for row in 0..usize::from(snap.size.rows) {
                    let start = row * cols;
                    let line: String = snap.cells[start..start + cols]
                        .iter()
                        .filter_map(|cell| char::from_u32(cell.content))
                        .collect();
                    if line.contains("GUIBOOT") {
                        f = true;
                        break;
                    }
                }
                f
            };
            if found {
                model.shutdown_all();
                return;
            }
            if std::time::Instant::now() > deadline {
                model.shutdown_all();
                panic!("GUI-boot startup marker never appeared");
            }
            model.pump(8);
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
    }

    // ── live animation controls ──

    #[test]
    fn live_animation_typewriter_applies_to_current_and_future_panes_and_bumps_generation() {
        use mr_crabs_config::TextAnimation;
        let mut model = headless();
        // Ensure at least two panes exist.
        model.dispatch(AppAction::NewSplitRight);
        let window_id = model.active_window.expect("window");
        let tab = model
            .windows
            .get(&window_id)
            .expect("window")
            .tabs
            .values()
            .next()
            .expect("tab");
        assert_eq!(tab.pane_count(), 2);

        let settings_gen_before = model.settings.generation;
        let model_gen_before = model.generation;

        let result = model.dispatch(AppAction::SetTextAnimationTypewriter);
        assert!(result.performed);
        assert_eq!(result.note, "text animation set to typewriter");

        // Settings snapshot reflects the runtime overlay.
        assert_eq!(model.settings.current().text_animation, "typewriter");
        assert_eq!(model.settings.generation, settings_gen_before + 1);
        assert_eq!(model.generation, model_gen_before + 1);

        // Every existing pane received the new defaults.
        for window in model.windows.values() {
            for tab in window.tabs.values() {
                for pane in tab.panes.values() {
                    assert_eq!(
                        pane.core.animation_defaults().text_animation,
                        TextAnimation::Typewriter
                    );
                }
            }
        }

        // Future panes inherit through construction.
        let size = model.settings.current().default_grid;
        let new_pane = model.new_pane_with_id(PaneId::new(9999), size);
        assert_eq!(
            new_pane.core.animation_defaults().text_animation,
            TextAnimation::Typewriter
        );
    }

    #[test]
    fn live_animation_none_then_streaming_exact_notes_and_pane_state() {
        use mr_crabs_config::TextAnimation;
        let mut model = headless();
        model.dispatch(AppAction::NewSplitRight);

        let r1 = model.dispatch(AppAction::SetTextAnimationNone);
        assert!(r1.performed);
        assert_eq!(r1.note, "text animation set to none");
        assert_eq!(model.settings.current().text_animation, "none");
        for window in model.windows.values() {
            for tab in window.tabs.values() {
                for pane in tab.panes.values() {
                    assert_eq!(
                        pane.core.animation_defaults().text_animation,
                        TextAnimation::Disabled
                    );
                }
            }
        }

        let r2 = model.dispatch(AppAction::SetTextAnimationStreaming);
        assert!(r2.performed);
        assert_eq!(r2.note, "text animation set to streaming");
        assert_eq!(model.settings.current().text_animation, "streaming");
        for window in model.windows.values() {
            for tab in window.tabs.values() {
                for pane in tab.panes.values() {
                    assert_eq!(
                        pane.core.animation_defaults().text_animation,
                        TextAnimation::Streaming
                    );
                }
            }
        }
    }

    #[test]
    fn live_animation_toggle_cursor_trail_twice_flips_and_restores() {
        let mut model = headless();
        model.dispatch(AppAction::NewSplitRight);
        let initial = model.settings.current().cursor_trail;
        let settings_gen_before = model.settings.generation;
        let model_gen_before = model.generation;

        let r1 = model.dispatch(AppAction::ToggleCursorTrail);
        assert!(r1.performed);
        let expected_first = if !initial {
            "cursor trail enabled"
        } else {
            "cursor trail disabled"
        };
        assert_eq!(r1.note, expected_first);
        assert_eq!(model.settings.current().cursor_trail, !initial);
        assert_eq!(model.settings.generation, settings_gen_before + 1);
        assert_eq!(model.generation, model_gen_before + 1);
        for window in model.windows.values() {
            for tab in window.tabs.values() {
                for pane in tab.panes.values() {
                    assert_eq!(pane.core.animation_defaults().cursor_trail, !initial);
                }
            }
        }

        let r2 = model.dispatch(AppAction::ToggleCursorTrail);
        assert!(r2.performed);
        let expected_second = if initial {
            "cursor trail enabled"
        } else {
            "cursor trail disabled"
        };
        assert_eq!(r2.note, expected_second);
        assert_eq!(model.settings.current().cursor_trail, initial);
        assert_eq!(model.settings.generation, settings_gen_before + 2);
        assert_eq!(model.generation, model_gen_before + 2);
        for window in model.windows.values() {
            for tab in window.tabs.values() {
                for pane in tab.panes.values() {
                    assert_eq!(pane.core.animation_defaults().cursor_trail, initial);
                }
            }
        }
    }

    #[test]
    fn live_animation_runtime_over_file_precedence_after_reload() {
        use mr_crabs_config::TextAnimation;
        let mut model = headless();
        model.dispatch(AppAction::NewSplitRight);
        let r = model.dispatch(AppAction::SetTextAnimationTypewriter);
        assert!(r.performed);
        assert_eq!(model.settings.current().text_animation, "typewriter");

        // Reload a file layer requesting none; runtime overlay must survive.
        model
            .settings
            .reload_json(r#"{"text_animation": "none"}"#, "test")
            .expect("reload");
        assert_eq!(model.settings.current().text_animation, "typewriter");
        for window in model.windows.values() {
            for tab in window.tabs.values() {
                for pane in tab.panes.values() {
                    assert_eq!(
                        pane.core.animation_defaults().text_animation,
                        TextAnimation::Typewriter
                    );
                }
            }
        }

        // Future panes also inherit the runtime value after the reload.
        let size = model.settings.current().default_grid;
        let new_pane = model.new_pane_with_id(PaneId::new(10001), size);
        assert_eq!(
            new_pane.core.animation_defaults().text_animation,
            TextAnimation::Typewriter
        );
    }
}

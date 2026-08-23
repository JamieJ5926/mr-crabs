//! Typed application settings with atomic reload.
//!
//! [`AppSettings`] carries the shell-visible configuration with
//! Ghostty-compatible defaults (the authoritative key/default parity lives
//! in `mr-crabs-config`; this is the typed shell view of it). Reload is
//! atomic: a new value is fully parsed and validated before it replaces the
//! current settings, so a malformed config never leaves the app half
//! reloaded. Readers take an immutable `Arc` snapshot, so renders never
//! observe a partially applied config.
//!
//! Effective values resolve as `defaults < file < CLI < runtime`. File
//! reloads replace only the file overlay; CLI and runtime overlays persist.

use std::fmt::{self, Display, Formatter};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use mr_crabs_config::{
    AnimationDefaults, ChildTerminfo, CloseOnExitPolicy, ConfigOverlay, EffectiveConfig,
    SettingKey, TextAnimation,
};
use mr_crabs_terminal::GridSize;
use serde::{Deserialize, Serialize};

use crate::keymap::KeyBindingDef;

pub use mr_crabs_config::{
    COLORTERM_TRUECOLOR, DEFAULT_FONT_FAMILY, DEFAULT_FONT_SIZE, DEFAULT_GRID_COLS,
    DEFAULT_GRID_ROWS, DEFAULT_LINE_HEIGHT_ADJUST_PERCENT, DEFAULT_PADDING_PX,
    DEFAULT_SCROLLBACK_LINES, TERM_FALLBACK, TERM_GHOSTTY, TERMINFO_ENTRY_REL,
    resolve_child_terminfo, resolve_child_terminfo_from, resolve_terminfo_dir,
    terminfo_search_paths,
};

/// When the shell process exits, whether the pane closes automatically.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CloseOnExit {
    /// Always close the pane when the child exits.
    Always,
    /// Close only when the exit status is clean (code 0).
    #[default]
    Clean,
    /// Never close automatically.
    Never,
}

impl CloseOnExit {
    fn from_policy(policy: CloseOnExitPolicy) -> Self {
        match policy {
            CloseOnExitPolicy::Always => Self::Always,
            CloseOnExitPolicy::Clean => Self::Clean,
            CloseOnExitPolicy::Never => Self::Never,
        }
    }

    fn to_policy(self) -> CloseOnExitPolicy {
        match self {
            Self::Always => CloseOnExitPolicy::Always,
            Self::Clean => CloseOnExitPolicy::Clean,
            Self::Never => CloseOnExitPolicy::Never,
        }
    }
}

fn default_font_family() -> String {
    DEFAULT_FONT_FAMILY.to_string()
}
fn default_font_size() -> f32 {
    DEFAULT_FONT_SIZE
}
fn default_line_height_adjust_percent() -> f32 {
    DEFAULT_LINE_HEIGHT_ADJUST_PERCENT
}
fn default_theme() -> String {
    mr_crabs_config::DEFAULT_THEME.to_string()
}
fn default_scrollback_lines() -> u32 {
    DEFAULT_SCROLLBACK_LINES
}
fn default_grid() -> GridSize {
    GridSize::new(DEFAULT_GRID_COLS, DEFAULT_GRID_ROWS)
}
fn default_opacity() -> f32 {
    mr_crabs_config::DEFAULT_BACKGROUND_OPACITY
}
fn default_padding() -> f32 {
    DEFAULT_PADDING_PX
}
fn default_text_animation() -> String {
    mr_crabs_config::DEFAULT_TEXT_ANIMATION.to_string()
}
fn default_duration() -> u64 {
    mr_crabs_config::DEFAULT_TEXT_ANIMATION_DURATION_MS
}
fn default_intensity() -> f32 {
    mr_crabs_config::DEFAULT_TEXT_ANIMATION_INTENSITY
}
fn default_cursor_trail_opacity() -> f32 {
    mr_crabs_config::DEFAULT_CURSOR_TRAIL_OPACITY
}
fn default_cursor_trail_duration() -> u64 {
    mr_crabs_config::DEFAULT_CURSOR_TRAIL_DURATION_MS
}
fn default_cursor_trail() -> bool {
    mr_crabs_config::DEFAULT_CURSOR_TRAIL
}
fn default_startup_fetch() -> bool {
    mr_crabs_config::DEFAULT_STARTUP_FETCH
}
fn default_startup_fetch_command() -> String {
    mr_crabs_config::DEFAULT_STARTUP_FETCH_COMMAND.to_string()
}
fn default_fetch_gif_path() -> String {
    mr_crabs_config::DEFAULT_FETCH_GIF_PATH.to_string()
}

/// Typed shell settings. Unknown JSON fields are ignored; every field
/// defaults to the Ghostty-compatible value shown in the field docs.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AppSettings {
    /// Font family used by the terminal element.
    #[serde(default = "default_font_family")]
    pub font_family: String,
    /// Font size in logical pixels.
    #[serde(default = "default_font_size")]
    pub font_size: f32,
    /// Ghostty `adjust-cell-height` percentage, applied after device-pixel rounding.
    #[serde(default = "default_line_height_adjust_percent")]
    pub line_height_adjust_percent: f32,
    /// Theme name; `"auto"` follows the system appearance.
    #[serde(default = "default_theme")]
    pub theme: String,
    /// Window background opacity in `0.0..=1.0`.
    #[serde(default = "default_opacity")]
    pub background_opacity: f32,
    /// Horizontal padding in logical pixels on each side.
    #[serde(default = "default_padding")]
    pub padding_x: f32,
    /// Vertical padding in logical pixels on each side.
    #[serde(default = "default_padding")]
    pub padding_y: f32,
    /// Whether the cursor blinks.
    #[serde(default)]
    pub cursor_blink: bool,
    /// Scrollback line limit.
    #[serde(default = "default_scrollback_lines")]
    pub scrollback_lines: u32,
    /// Explicit shell path; `None` discovers the login shell.
    #[serde(default)]
    pub shell: Option<String>,
    /// Working directory for spawned shells; `None` inherits.
    #[serde(default)]
    pub working_directory: Option<String>,
    /// Grid used for new windows before the platform reports bounds.
    #[serde(default = "default_grid")]
    pub default_grid: GridSize,
    /// When to close the pane after the child exits.
    #[serde(default)]
    pub close_on_exit: CloseOnExit,
    /// Mr Crabs cursor-trail default (on).
    #[serde(default = "default_cursor_trail")]
    pub cursor_trail: bool,
    /// Mr Crabs cursor-trail opacity.
    #[serde(default = "default_cursor_trail_opacity")]
    pub cursor_trail_opacity: f32,
    /// Cursor-trail fade duration in milliseconds.
    #[serde(default = "default_cursor_trail_duration")]
    pub cursor_trail_duration_ms: u64,
    /// Text-animation mode: `"none"`, `"streaming"`, or `"typewriter"`.
    #[serde(default = "default_text_animation")]
    pub text_animation: String,
    /// Text-animation duration in milliseconds.
    #[serde(default = "default_duration")]
    pub text_animation_duration_ms: u64,
    /// Text-animation intensity.
    #[serde(default = "default_intensity")]
    pub text_animation_intensity: f32,
    /// Permit terminal OSC 52 writes to the system clipboard.
    #[serde(default)]
    pub allow_osc52_write: bool,
    /// Permit terminal OSC 52 reads from the system clipboard.
    #[serde(default)]
    pub allow_osc52_read: bool,
    /// Whether new windows auto-run the startup fetch command.
    #[serde(default = "default_startup_fetch")]
    pub startup_fetch: bool,
    /// POSIX command run on the PTY before the interactive shell starts.
    #[serde(default = "default_startup_fetch_command")]
    pub startup_fetch_command: String,
    /// Path to a GIF for animated fetch; empty disables animation.
    #[serde(default = "default_fetch_gif_path")]
    pub fetch_gif_path: String,
    /// Shell keybindings (keyboard-only operation).
    #[serde(default)]
    pub keybindings: Vec<KeyBindingDef>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self::from_effective(
            &EffectiveConfig::defaults(),
            crate::keymap::default_keybindings(),
        )
    }
}

impl AppSettings {
    /// Parse a JSON config. Unknown fields are ignored; missing fields take
    /// the Ghostty-compatible defaults.
    pub fn from_json(json: &str) -> Result<Self, SettingsError> {
        serde_json::from_str(json).map_err(|e| SettingsError::Json(e.to_string()))
    }

    /// Materialize the typed shell view from the config-crate authority.
    pub fn from_effective(effective: &EffectiveConfig, keybindings: Vec<KeyBindingDef>) -> Self {
        Self {
            font_family: effective.font_family.clone(),
            font_size: effective.font_size,
            line_height_adjust_percent: effective.line_height_adjust_percent,
            theme: effective.theme.clone(),
            background_opacity: effective.background_opacity,
            padding_x: effective.padding_x,
            padding_y: effective.padding_y,
            cursor_blink: effective.cursor_blink,
            scrollback_lines: effective.scrollback_lines,
            shell: effective.shell.clone(),
            working_directory: effective.working_directory.clone(),
            default_grid: GridSize::new(effective.default_grid.0, effective.default_grid.1),
            close_on_exit: CloseOnExit::from_policy(effective.close_on_exit),
            cursor_trail: effective.cursor_trail,
            cursor_trail_opacity: effective.cursor_trail_opacity,
            cursor_trail_duration_ms: effective.cursor_trail_duration_ms,
            text_animation: effective.text_animation.clone(),
            text_animation_duration_ms: effective.text_animation_duration_ms,
            text_animation_intensity: effective.text_animation_intensity,
            allow_osc52_write: effective.allow_osc52_write,
            allow_osc52_read: effective.allow_osc52_read,
            startup_fetch: effective.startup_fetch,
            startup_fetch_command: effective.startup_fetch_command.clone(),
            fetch_gif_path: effective.fetch_gif_path.clone(),
            keybindings,
        }
    }

    /// Map the shell text-animation setting onto the config-crate enum.
    pub fn text_animation_kind(&self) -> TextAnimation {
        TextAnimation::parse(&self.text_animation)
    }

    /// Build the Mr Crabs animation defaults for terminal elements.
    pub fn animation_defaults(&self) -> AnimationDefaults {
        self.effective_config().animation_defaults()
    }

    pub fn effective_config(&self) -> EffectiveConfig {
        EffectiveConfig {
            font_family: self.font_family.clone(),
            font_size: self.font_size,
            line_height_adjust_percent: self.line_height_adjust_percent,
            theme: self.theme.clone(),
            background_opacity: self.background_opacity,
            padding_x: self.padding_x,
            padding_y: self.padding_y,
            cursor_blink: self.cursor_blink,
            scrollback_lines: self.scrollback_lines,
            shell: self.shell.clone(),
            working_directory: self.working_directory.clone(),
            default_grid: (self.default_grid.cols, self.default_grid.rows),
            close_on_exit: self.close_on_exit.to_policy(),
            cursor_trail: self.cursor_trail,
            cursor_trail_opacity: self.cursor_trail_opacity,
            cursor_trail_duration_ms: self.cursor_trail_duration_ms,
            text_animation: self.text_animation.clone(),
            text_animation_duration_ms: self.text_animation_duration_ms,
            text_animation_intensity: self.text_animation_intensity,
            allow_osc52_write: self.allow_osc52_write,
            allow_osc52_read: self.allow_osc52_read,
            startup_fetch: self.startup_fetch,
            startup_fetch_command: self.startup_fetch_command.clone(),
            fetch_gif_path: self.fetch_gif_path.clone(),
        }
    }

    /// `+show-config` lines for the effective values, not defaults-only.
    pub fn show_config_lines(&self, docs: bool) -> Vec<String> {
        self.effective_config().show_config_lines(docs)
    }

    /// Child TERM/COLORTERM/TERMINFO for the current packaging layout.
    pub fn child_terminfo(&self) -> ChildTerminfo {
        resolve_child_terminfo()
    }
}

/// One effective key that changed across a successful reload.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SettingsChange {
    pub key: String,
    pub previous: String,
    pub current: String,
}

/// A config source: the built-in defaults or a JSON document.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SettingsSource {
    /// Built-in Ghostty-compatible defaults.
    Defaults,
    /// An in-memory JSON document (used by tests and the intent handler).
    Json(String),
    /// A JSON file on disk.
    Path(PathBuf),
}

/// Errors that never leave settings in a half-reloaded state.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SettingsError {
    /// The JSON document could not be parsed.
    Json(String),
    /// The file could not be read.
    Io(String),
    /// A CLI or overlay value was invalid.
    Invalid(String),
    /// No reload source has been configured.
    NoSource,
}

impl Display for SettingsError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            SettingsError::Json(e) => write!(f, "invalid config json: {e}"),
            SettingsError::Io(e) => write!(f, "config io error: {e}"),
            SettingsError::Invalid(e) => write!(f, "invalid config: {e}"),
            SettingsError::NoSource => write!(f, "no config source configured"),
        }
    }
}

impl std::error::Error for SettingsError {}

/// Parsed CLI: optional config file plus explicit field overrides.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct CliOverrides {
    pub config_file: Option<PathBuf>,
    pub overlay: ConfigOverlay,
    pub keybindings: Option<Vec<KeyBindingDef>>,
    pub show_config: bool,
    pub show_default: bool,
    pub docs: bool,
    pub version: bool,
    pub help: bool,
}
impl CliOverrides {
    /// Parse Ghostty-style `--flag=value`, `--flag value`, and boolean `--flag`.
    pub fn parse(args: &[String]) -> Result<Self, SettingsError> {
        let mut cli = Self::default();
        let mut index = 0;
        while index < args.len() {
            let arg = args[index].as_str();
            if arg == "+show-config" {
                cli.show_config = true;
                index += 1;
                continue;
            }
            if arg == "--version" || arg == "+version" {
                cli.version = true;
                index += 1;
                continue;
            }
            if arg == "--docs" {
                cli.docs = true;
                index += 1;
                continue;
            }
            if arg == "--default" {
                cli.show_default = true;
                index += 1;
                continue;
            }
            if arg == "--help" || arg == "-h" {
                cli.help = true;
                index += 1;
                continue;
            }
            if arg == "--config-file" {
                let value = args.get(index + 1).ok_or_else(|| {
                    SettingsError::Invalid("missing value for --config-file".into())
                })?;
                cli.config_file = Some(PathBuf::from(value));
                index += 2;
                continue;
            }
            if let Some(value) = arg.strip_prefix("--config-file=") {
                cli.config_file = Some(PathBuf::from(value));
                index += 1;
                continue;
            }
            if let Some((flag, value)) = split_flag(arg) {
                apply_cli_flag(&mut cli, flag, Some(value))?;
                index += 1;
                continue;
            }
            if let Some(flag) = arg.strip_prefix("--") {
                if let Some(key) = SettingKey::from_flag(flag) {
                    if key.is_boolean() {
                        let next = args.get(index + 1).map(String::as_str);
                        if next
                            .is_some_and(|value| !value.starts_with('-') && !value.starts_with('+'))
                        {
                            apply_cli_flag(&mut cli, flag, next)?;
                            index += 2;
                        } else {
                            apply_cli_flag(&mut cli, flag, Some("true"))?;
                            index += 1;
                        }
                    } else {
                        let value = args.get(index + 1).ok_or_else(|| {
                            SettingsError::Invalid(format!("missing value for --{flag}"))
                        })?;
                        apply_cli_flag(&mut cli, flag, Some(value))?;
                        index += 2;
                    }
                    continue;
                }
                if flag == "keybindings" {
                    let value = args.get(index + 1).ok_or_else(|| {
                        SettingsError::Invalid("missing value for --keybindings".into())
                    })?;
                    cli.keybindings = Some(parse_keybindings_json(value)?);
                    index += 2;
                    continue;
                }
                return Err(SettingsError::Invalid(format!("unknown flag --{flag}")));
            }
            if arg.starts_with('+') || arg.starts_with('-') {
                return Err(SettingsError::Invalid(format!("unknown argument {arg}")));
            }
            index += 1;
        }
        Ok(cli)
    }
}

fn split_flag(arg: &str) -> Option<(&str, &str)> {
    let rest = arg.strip_prefix("--")?;
    rest.split_once('=')
}

fn apply_cli_flag(
    cli: &mut CliOverrides,
    flag: &str,
    value: Option<&str>,
) -> Result<(), SettingsError> {
    if flag == "keybindings" {
        let value =
            value.ok_or_else(|| SettingsError::Invalid("missing --keybindings value".into()))?;
        cli.keybindings = Some(parse_keybindings_json(value)?);
        return Ok(());
    }
    let key = SettingKey::from_flag(flag)
        .ok_or_else(|| SettingsError::Invalid(format!("unknown flag --{flag}")))?;
    let value = match value {
        Some(value) => value,
        None if key.is_boolean() => "true",
        None => {
            return Err(SettingsError::Invalid(format!(
                "missing value for --{flag}"
            )));
        }
    };
    cli.overlay.set(key, value).map_err(SettingsError::Invalid)
}

fn parse_keybindings_json(value: &str) -> Result<Vec<KeyBindingDef>, SettingsError> {
    serde_json::from_str(value).map_err(|error| SettingsError::Json(error.to_string()))
}

/// Partial JSON overlay. Missing keys inherit the lower layer.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
struct PartialAppSettings {
    font_family: Option<String>,
    font_size: Option<f32>,
    line_height_adjust_percent: Option<f32>,
    theme: Option<String>,
    background_opacity: Option<f32>,
    padding_x: Option<f32>,
    padding_y: Option<f32>,
    cursor_blink: Option<bool>,
    scrollback_lines: Option<u32>,
    shell: Option<String>,
    working_directory: Option<String>,
    default_grid: Option<GridSize>,
    close_on_exit: Option<CloseOnExit>,
    cursor_trail: Option<bool>,
    cursor_trail_opacity: Option<f32>,
    cursor_trail_duration_ms: Option<u64>,
    text_animation: Option<String>,
    text_animation_duration_ms: Option<u64>,
    text_animation_intensity: Option<f32>,
    allow_osc52_write: Option<bool>,
    allow_osc52_read: Option<bool>,
    startup_fetch: Option<bool>,
    startup_fetch_command: Option<String>,
    fetch_gif_path: Option<String>,
    keybindings: Option<Vec<KeyBindingDef>>,
}

impl PartialAppSettings {
    fn into_layers(self) -> (ConfigOverlay, Option<Vec<KeyBindingDef>>) {
        let overlay = ConfigOverlay {
            font_family: self.font_family,
            font_size: self.font_size,
            line_height_adjust_percent: self.line_height_adjust_percent,
            theme: self.theme,
            background_opacity: self.background_opacity,
            padding_x: self.padding_x,
            padding_y: self.padding_y,
            cursor_blink: self.cursor_blink,
            scrollback_lines: self.scrollback_lines,
            shell: self.shell,
            working_directory: self.working_directory,
            default_grid: self.default_grid.map(|grid| (grid.cols, grid.rows)),
            close_on_exit: self.close_on_exit.map(CloseOnExit::to_policy),
            cursor_trail: self.cursor_trail,
            cursor_trail_opacity: self.cursor_trail_opacity,
            cursor_trail_duration_ms: self.cursor_trail_duration_ms,
            text_animation: self.text_animation,
            text_animation_duration_ms: self.text_animation_duration_ms,
            text_animation_intensity: self.text_animation_intensity,
            allow_osc52_write: self.allow_osc52_write,
            allow_osc52_read: self.allow_osc52_read,
            startup_fetch: self.startup_fetch,
            startup_fetch_command: self.startup_fetch_command,
            fetch_gif_path: self.fetch_gif_path,
        };
        (overlay, self.keybindings)
    }
}

fn parse_file_overlay(
    json: &str,
) -> Result<(ConfigOverlay, Option<Vec<KeyBindingDef>>), SettingsError> {
    let parsed: PartialAppSettings =
        serde_json::from_str(json).map_err(|error| SettingsError::Json(error.to_string()))?;
    Ok(parsed.into_layers())
}

fn resolve_keybindings(
    file: Option<&[KeyBindingDef]>,
    cli: Option<&[KeyBindingDef]>,
    runtime: Option<&[KeyBindingDef]>,
) -> Vec<KeyBindingDef> {
    if let Some(bindings) = runtime {
        return bindings.to_vec();
    }
    if let Some(bindings) = cli {
        return bindings.to_vec();
    }
    if let Some(bindings) = file {
        return bindings.to_vec();
    }
    crate::keymap::default_keybindings()
}

fn diff_settings(previous: &AppSettings, current: &AppSettings) -> Vec<SettingsChange> {
    let before = previous.effective_config();
    let after = current.effective_config();
    let mut changes = Vec::new();
    for key in SettingKey::ALL {
        let previous_value = before.display_value(key);
        let current_value = after.display_value(key);
        if previous_value != current_value {
            changes.push(SettingsChange {
                key: key.flag().to_string(),
                previous: previous_value,
                current: current_value,
            });
        }
    }
    if previous.keybindings != current.keybindings {
        changes.push(SettingsChange {
            key: "keybindings".to_string(),
            previous: format!("{} bindings", previous.keybindings.len()),
            current: format!("{} bindings", current.keybindings.len()),
        });
    }
    changes
}

/// Atomic settings holder. `current()` returns an immutable snapshot;
/// reloads parse fully and only then swap the `Arc`, bumping the generation.
#[derive(Clone, Debug)]
pub struct SettingsStore {
    current: Arc<AppSettings>,
    file: ConfigOverlay,
    cli: ConfigOverlay,
    runtime: ConfigOverlay,
    file_keybindings: Option<Vec<KeyBindingDef>>,
    cli_keybindings: Option<Vec<KeyBindingDef>>,
    runtime_keybindings: Option<Vec<KeyBindingDef>>,
    source: SettingsSource,
    path: Option<PathBuf>,
    /// Monotonic generation; bumped only by successful reloads.
    pub generation: u64,
    /// How many successful reloads happened.
    pub reload_count: u64,
    /// The most recent reload error, if any (settings were left unchanged).
    pub last_error: Option<SettingsError>,
    /// Effective keys that changed on the last successful reload.
    last_changes: Vec<SettingsChange>,
}

impl SettingsStore {
    pub fn new() -> Self {
        Self {
            current: Arc::new(AppSettings::default()),
            file: ConfigOverlay::default(),
            cli: ConfigOverlay::default(),
            runtime: ConfigOverlay::default(),
            file_keybindings: None,
            cli_keybindings: None,
            runtime_keybindings: None,
            source: SettingsSource::Defaults,
            path: None,
            generation: 0,
            reload_count: 0,
            last_error: None,
            last_changes: Vec::new(),
        }
    }

    /// Build from already-resolved layers (CLI startup path).
    pub fn from_layers(
        file: ConfigOverlay,
        cli: ConfigOverlay,
        runtime: ConfigOverlay,
        file_keybindings: Option<Vec<KeyBindingDef>>,
        cli_keybindings: Option<Vec<KeyBindingDef>>,
        source: SettingsSource,
        path: Option<PathBuf>,
    ) -> Self {
        let mut store = Self::new();
        store.file = file;
        store.cli = cli;
        store.runtime = runtime;
        store.file_keybindings = file_keybindings;
        store.cli_keybindings = cli_keybindings;
        store.source = source;
        store.path = path;
        store.current = Arc::new(store.materialize());
        store
    }

    pub fn from_cli(cli: &CliOverrides) -> Result<Self, SettingsError> {
        let (file, file_keybindings, source, path) = if let Some(path) = &cli.config_file {
            let contents = std::fs::read_to_string(path)
                .map_err(|error| SettingsError::Io(error.to_string()))?;
            let (overlay, keybindings) = parse_file_overlay(&contents)?;
            (
                overlay,
                keybindings,
                SettingsSource::Path(path.clone()),
                Some(path.clone()),
            )
        } else {
            (
                ConfigOverlay::default(),
                None,
                SettingsSource::Defaults,
                None,
            )
        };
        Ok(Self::from_layers(
            file,
            cli.overlay.clone(),
            ConfigOverlay::default(),
            file_keybindings,
            cli.keybindings.clone(),
            source,
            path,
        ))
    }

    /// Immutable settings snapshot; never observes a partially applied
    /// reload.
    pub fn current(&self) -> Arc<AppSettings> {
        self.current.clone()
    }

    pub fn source(&self) -> &SettingsSource {
        &self.source
    }

    pub fn last_changes(&self) -> &[SettingsChange] {
        &self.last_changes
    }

    pub fn cli_overlay(&self) -> &ConfigOverlay {
        &self.cli
    }

    pub fn runtime_overlay(&self) -> &ConfigOverlay {
        &self.runtime
    }

    pub fn file_overlay(&self) -> &ConfigOverlay {
        &self.file
    }

    /// Set the on-disk reload source without reading it yet.
    pub fn set_path(&mut self, path: PathBuf) {
        self.path = Some(path.clone());
        self.source = SettingsSource::Path(path);
    }

    /// Reload from whichever source is configured.
    pub fn reload_from_source(&mut self) -> Result<(), SettingsError> {
        match self.source.clone() {
            SettingsSource::Defaults => Err(SettingsError::NoSource),
            SettingsSource::Json(json) => self.reload_json(&json, "json source"),
            SettingsSource::Path(path) => self.reload_path(&path),
        }
    }

    /// Atomically reload the file overlay from a JSON document. CLI and
    /// runtime overlays are left in place. On error the current settings
    /// are left untouched and the error is recorded.
    pub fn reload_json(&mut self, json: &str, source: &str) -> Result<(), SettingsError> {
        let (file, file_keybindings) = parse_file_overlay(json).inspect_err(|error| {
            self.last_error = Some(error.clone());
        })?;
        self.file = file;
        self.file_keybindings = file_keybindings;
        self.commit(SettingsSource::Json(json.to_string()), source)
    }

    /// Atomically reload from a JSON file. The file is read and parsed
    /// before anything is swapped.
    pub fn reload_path(&mut self, path: &Path) -> Result<(), SettingsError> {
        let contents = std::fs::read_to_string(path).map_err(|error| {
            let error = SettingsError::Io(error.to_string());
            self.last_error = Some(error.clone());
            error
        })?;
        let (file, file_keybindings) = parse_file_overlay(&contents).inspect_err(|error| {
            self.last_error = Some(error.clone());
        })?;
        self.file = file;
        self.file_keybindings = file_keybindings;
        self.path = Some(path.to_path_buf());
        self.commit(
            SettingsSource::Path(path.to_path_buf()),
            &path.display().to_string(),
        )
    }

    /// Apply a runtime overlay. Runtime keys survive later file reloads.
    pub fn apply_runtime(
        &mut self,
        overlay: ConfigOverlay,
    ) -> Result<Vec<SettingsChange>, SettingsError> {
        let mut runtime = self.runtime.clone();
        runtime.merge(overlay);
        self.runtime = runtime;
        self.commit(self.source.clone(), "runtime")
            .map(|()| self.last_changes.clone())
    }

    pub fn apply_runtime_value(
        &mut self,
        key: SettingKey,
        value: &str,
    ) -> Result<Vec<SettingsChange>, SettingsError> {
        let mut overlay = ConfigOverlay::default();
        overlay.set(key, value).map_err(SettingsError::Invalid)?;
        self.apply_runtime(overlay)
    }

    fn materialize(&self) -> AppSettings {
        let effective = EffectiveConfig::resolve(&self.file, &self.cli, &self.runtime);
        AppSettings::from_effective(
            &effective,
            resolve_keybindings(
                self.file_keybindings.as_deref(),
                self.cli_keybindings.as_deref(),
                self.runtime_keybindings.as_deref(),
            ),
        )
    }

    fn commit(&mut self, source: SettingsSource, _label: &str) -> Result<(), SettingsError> {
        let settings = self.materialize();
        let changes = diff_settings(&self.current, &settings);
        self.current = Arc::new(settings);
        self.source = source;
        self.generation += 1;
        self.reload_count += 1;
        self.last_error = None;
        self.last_changes = changes;
        Ok(())
    }
}

impl Default for SettingsStore {
    fn default() -> Self {
        Self::new()
    }
}

/// Load the effective layers for `+show-config` without starting the GUI.
pub fn load_effective_from_cli(cli: &CliOverrides) -> Result<AppSettings, SettingsError> {
    if cli.show_default {
        let effective = EffectiveConfig::resolve(
            &ConfigOverlay::default(),
            &cli.overlay,
            &ConfigOverlay::default(),
        );
        return Ok(AppSettings::from_effective(
            &effective,
            resolve_keybindings(None, cli.keybindings.as_deref(), None),
        ));
    }
    Ok(SettingsStore::from_cli(cli)?.current().as_ref().clone())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_gspot_ghostty_typography() {
        let settings = AppSettings::default();
        assert_eq!(settings.font_family, "JetBrains Mono");
        assert_eq!(settings.font_size, 18.0);
        assert_eq!(settings.line_height_adjust_percent, 5.0);
        assert_eq!(settings.padding_x, 10.0);
        assert_eq!(settings.padding_y, 10.0);
        assert_eq!(settings.scrollback_lines, DEFAULT_SCROLLBACK_LINES);
        assert!(settings.cursor_trail, "cursor trail defaults on");
        assert_eq!(
            settings.text_animation_kind(),
            TextAnimation::Streaming,
            "text animation defaults to streaming"
        );
        // Animation defaults remain independent of terminal typography.
        let anim = settings.animation_defaults();
        assert_eq!(anim.cursor_trail_opacity, 0.35);
        assert_eq!(anim.cursor_trail_duration.as_millis(), 250);
        assert_eq!(anim.text_animation_duration.as_millis(), 120);
        assert_eq!(anim.text_animation_intensity, 1.0);
        assert!(!settings.keybindings.is_empty());
    }

    #[test]
    fn partial_json_overrides_only_named_fields() {
        let json = r#"{"font_size": 14.0, "cursor_blink": true}"#;
        let settings = AppSettings::from_json(json).expect("valid json");
        assert_eq!(settings.font_size, 14.0);
        assert!(settings.cursor_blink);
        assert_eq!(
            settings.font_family, DEFAULT_FONT_FAMILY,
            "unset fields keep defaults"
        );
        assert_eq!(settings.default_grid, GridSize::new(80, 24));
    }

    #[test]
    fn unknown_fields_are_ignored() {
        let json = r#"{"font_size": 13.0, "not_a_setting": 42}"#;
        let settings = AppSettings::from_json(json).expect("unknown fields ignored");
        assert_eq!(settings.font_size, 13.0);
    }

    #[test]
    fn animation_kind_mapping() {
        let base = AppSettings::default();
        let none = AppSettings {
            text_animation: "none".into(),
            ..base.clone()
        };
        assert_eq!(none.text_animation_kind(), TextAnimation::Disabled);
        let typewriter = AppSettings {
            text_animation: "typewriter".into(),
            ..base.clone()
        };
        assert_eq!(typewriter.text_animation_kind(), TextAnimation::Typewriter);
        let unknown = AppSettings {
            text_animation: "glowy".into(),
            ..base
        };
        assert_eq!(unknown.text_animation_kind(), TextAnimation::Streaming);
    }

    #[test]
    fn reload_is_atomic_on_error() {
        let mut store = SettingsStore::new();
        let original = store.current();
        assert_eq!(store.generation, 0);
        assert!(store.reload_json("{not json", "test").is_err());
        // Failed reloads leave the settings and generation untouched.
        assert_eq!(*store.current(), *original);
        assert_eq!(store.generation, 0);
        assert!(store.last_error.is_some());
        assert!(store.last_changes().is_empty());
    }

    #[test]
    fn successful_reload_swaps_snapshot_and_bumps_generation() {
        let mut store = SettingsStore::new();
        store
            .reload_json(r#"{"font_size": 15.0}"#, "test")
            .expect("valid reload");
        assert_eq!(store.generation, 1);
        assert_eq!(store.reload_count, 1);
        assert_eq!(store.current().font_size, 15.0);
        // Old snapshots keep the old value: immutable handoff.
        let old = AppSettings::default();
        assert_eq!(old.font_size, DEFAULT_FONT_SIZE);
        // A second successful reload bumps again.
        store
            .reload_json(r#"{"font_size": 16.0}"#, "test")
            .expect("valid reload");
        assert_eq!(store.generation, 2);
        assert_eq!(store.current().font_size, 16.0);
    }

    #[test]
    fn reload_from_source_without_source_errors_cleanly() {
        let mut store = SettingsStore::new();
        assert_eq!(store.reload_from_source(), Err(SettingsError::NoSource));
        assert_eq!(store.generation, 0);
    }

    #[test]
    fn close_on_exit_variants_serialize_exactly() {
        assert_eq!(
            serde_json::to_string(&CloseOnExit::Always).expect("serialize"),
            "\"always\""
        );
        assert_eq!(
            serde_json::to_string(&CloseOnExit::Clean).expect("serialize"),
            "\"clean\""
        );
        assert_eq!(
            serde_json::to_string(&CloseOnExit::Never).expect("serialize"),
            "\"never\""
        );
        assert_eq!(CloseOnExit::default(), CloseOnExit::Clean);
    }

    #[test]
    fn close_on_exit_deserializes_from_settings() {
        for (value, expected) in [
            ("always", CloseOnExit::Always),
            ("clean", CloseOnExit::Clean),
            ("never", CloseOnExit::Never),
        ] {
            let settings = AppSettings::from_json(&format!(r#"{{"close_on_exit":"{value}"}}"#))
                .expect("valid close_on_exit");
            assert_eq!(settings.close_on_exit, expected);
        }
    }

    #[test]
    fn layered_precedence_file_then_cli_then_runtime() {
        let dir = std::env::temp_dir().join(format!(
            "mr-crabs-settings-prec-{}-{}",
            std::process::id(),
            unique_stamp()
        ));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let path = dir.join("config.json");
        std::fs::write(
            &path,
            r#"{"font_size": 14.0, "theme": "dark", "cursor_blink": true}"#,
        )
        .expect("write file");

        let cli = CliOverrides::parse(&[
            format!("--config-file={}", path.display()),
            "--font-size".into(),
            "16".into(),
            "--window-padding-x=3".into(),
        ])
        .expect("cli");
        let mut store = SettingsStore::from_cli(&cli).expect("store");
        assert_eq!(store.current().font_size, 16.0, "cli beats file");
        assert_eq!(store.current().theme, "dark");
        assert!(store.current().cursor_blink);
        assert_eq!(store.current().padding_x, 3.0);

        store
            .apply_runtime_value(SettingKey::Theme, "light")
            .expect("runtime");
        assert_eq!(store.current().theme, "light");
        assert_eq!(store.current().font_size, 16.0);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn malformed_reload_rolls_back_and_keeps_runtime() {
        let mut store = SettingsStore::new();
        store
            .reload_json(r#"{"theme":"ok","font_size":12.0}"#, "good")
            .expect("seed file");
        store
            .apply_runtime_value(SettingKey::FontFamily, "Iosevka")
            .expect("runtime");
        let generation = store.generation;
        let snapshot = store.current();
        assert!(store.reload_json("{", "bad").is_err());
        assert_eq!(store.generation, generation);
        assert_eq!(*store.current(), *snapshot);
        assert_eq!(store.current().theme, "ok");
        assert_eq!(store.current().font_family, "Iosevka");
        assert!(store.last_error.is_some());
    }

    #[test]
    fn runtime_overrides_survive_file_reload() {
        let mut store = SettingsStore::new();
        store
            .reload_json(r#"{"font_size":11.0,"theme":"one"}"#, "one")
            .expect("file one");
        let changes = store
            .apply_runtime_value(SettingKey::FontSize, "22")
            .expect("runtime");
        assert!(changes.iter().any(|change| change.key == "font-size"));
        store
            .reload_json(r#"{"font_size":13.0,"theme":"two"}"#, "two")
            .expect("file two");
        assert_eq!(store.current().font_size, 22.0, "runtime persists");
        assert_eq!(store.current().theme, "two", "unshadowed file key updates");
        assert!(
            store
                .last_changes()
                .iter()
                .any(|change| change.key == "theme" && change.current == "two")
        );
        assert!(
            !store
                .last_changes()
                .iter()
                .any(|change| change.key == "font-size"),
            "runtime-shadowed font-size is unchanged"
        );
    }

    #[test]
    fn show_config_prints_complete_effective_layers() {
        let mut store = SettingsStore::new();
        store
            .reload_json(r#"{"font_size":21.0,"theme":"vapor"}"#, "file")
            .expect("file");
        store
            .apply_runtime_value(SettingKey::CursorTrail, "true")
            .expect("runtime");
        let text = store.current().show_config_lines(false).join("\n");
        for key in SettingKey::ALL {
            assert!(
                text.contains(&format!("{} =", key.flag())),
                "missing {}",
                key.flag()
            );
        }
        assert!(text.contains("font-size = 21"));
        assert!(text.contains("theme = vapor"));
        assert!(text.contains("cursor-trail = true"));
        assert!(text.contains("cursor-trail-duration = 250ms"));
        assert!(!text.contains("JetBrains Mono") || text.contains("font-family = JetBrains Mono"));
    }

    #[test]
    fn terminfo_exists_in_dev_layout_and_falls_back() {
        let present = AppSettings::default().child_terminfo();
        // Workspace checkout ships resources/terminfo/78/xterm-ghostty.
        let dev =
            std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../resources/terminfo");
        assert!(dev.join(TERMINFO_ENTRY_REL).is_file());
        let from_dev = resolve_child_terminfo_from(std::slice::from_ref(&dev));
        assert_eq!(from_dev.term, TERM_GHOSTTY);
        assert_eq!(from_dev.colorterm, COLORTERM_TRUECOLOR);
        assert_eq!(from_dev.terminfo_dir.as_deref(), Some(dev.as_path()));
        assert!(present.colorterm == COLORTERM_TRUECOLOR);

        let missing = std::env::temp_dir().join(format!(
            "mr-crabs-app-terminfo-missing-{}-{}",
            std::process::id(),
            unique_stamp()
        ));
        std::fs::create_dir_all(missing.join("78")).expect("dir");
        let fallback = resolve_child_terminfo_from(std::slice::from_ref(&missing));
        assert_eq!(fallback.term, TERM_FALLBACK);
        assert!(fallback.terminfo_dir.is_none());
        let _ = std::fs::remove_dir_all(missing);
    }

    #[test]
    fn cli_overrides_every_user_facing_field() {
        let args = [
            "--font-family=Iosevka",
            "--font-size=12",
            "--adjust-cell-height=8%",
            "--theme=dark",
            "--background-opacity=0.5",
            "--window-padding-x=2",
            "--window-padding-y=3",
            "--cursor-style-blink=true",
            "--scrollback-limit=2000",
            "--shell=/bin/zsh",
            "--working-directory=/tmp",
            "--initial-window-size=100x40",
            "--close-on-exit=never",
            "--cursor-trail",
            "--cursor-trail-opacity=0.2",
            "--text-animation=typewriter",
            "--text-animation-duration=90ms",
            "--text-animation-intensity=0.4",
        ]
        .into_iter()
        .map(String::from)
        .collect::<Vec<_>>();
        let cli = CliOverrides::parse(&args).expect("parse");
        let settings = load_effective_from_cli(&cli).expect("effective");
        assert_eq!(settings.font_family, "Iosevka");
        assert_eq!(settings.font_size, 12.0);
        assert_eq!(settings.line_height_adjust_percent, 8.0);
        assert_eq!(settings.theme, "dark");
        assert_eq!(settings.background_opacity, 0.5);
        assert_eq!(settings.padding_x, 2.0);
        assert_eq!(settings.padding_y, 3.0);
        assert!(settings.cursor_blink);
        assert_eq!(settings.scrollback_lines, 2000);
        assert_eq!(settings.shell.as_deref(), Some("/bin/zsh"));
        assert_eq!(settings.working_directory.as_deref(), Some("/tmp"));
        assert_eq!(settings.default_grid, GridSize::new(100, 40));
        assert_eq!(settings.close_on_exit, CloseOnExit::Never);
        assert!(settings.cursor_trail);
        assert_eq!(settings.cursor_trail_opacity, 0.2);
        assert_eq!(settings.text_animation, "typewriter");
        assert_eq!(settings.text_animation_duration_ms, 90);
        assert_eq!(settings.text_animation_intensity, 0.4);
    }

    #[test]
    fn cli_help_long_sets_only_help() {
        let cli = CliOverrides::parse(&["--help".into()]).expect("parse --help");
        assert!(cli.help);
        assert!(!cli.show_config);
        assert!(!cli.version);
        assert!(!cli.docs);
        assert!(!cli.show_default);
        assert!(cli.config_file.is_none());
        assert!(cli.overlay.is_empty());
    }

    #[test]
    fn cli_help_short_sets_only_help() {
        let cli = CliOverrides::parse(&["-h".into()]).expect("parse -h");
        assert!(cli.help);
        assert!(!cli.show_config);
        assert!(!cli.version);
        assert!(!cli.docs);
        assert!(!cli.show_default);
        assert!(cli.config_file.is_none());
        assert!(cli.overlay.is_empty());
    }

    #[test]
    fn cli_help_alongside_other_flags_preserves_both() {
        let cli = CliOverrides::parse(&["--help".into(), "--font-size".into(), "14".into()])
            .expect("parse --help with value flag");
        assert!(cli.help);
        // Other flag still applies; help does not suppress parsing.
        let settings = load_effective_from_cli(&cli).expect("effective");
        assert_eq!(settings.font_size, 14.0);
    }

    #[test]
    fn cli_help_short_alongside_config_flag() {
        let cli = CliOverrides::parse(&["-h".into(), "--theme".into(), "dark".into()])
            .expect("parse -h with flag");
        assert!(cli.help);
        let settings = load_effective_from_cli(&cli).expect("effective");
        assert_eq!(settings.theme, "dark");
    }

    #[test]
    fn cli_help_rejects_plus_help() {
        let err = CliOverrides::parse(&["+help".into()]).expect_err("+help must be invalid");
        assert!(matches!(err, SettingsError::Invalid(_)));
        assert!(err.to_string().contains("+help"));
    }

    #[test]
    fn cli_help_rejects_help_value_form() {
        let err = CliOverrides::parse(&["--help=value".into()]).expect_err("--help=value invalid");
        assert!(matches!(err, SettingsError::Invalid(_)));
    }

    #[test]
    fn cli_help_rejects_help_equals_empty() {
        let err = CliOverrides::parse(&["--help=".into()]).expect_err("--help= invalid");
        assert!(matches!(err, SettingsError::Invalid(_)));
    }

    #[test]
    fn cli_help_positional_args_remain_ignored() {
        let cli = CliOverrides::parse(&["--help".into(), "positional".into(), "-h".into()])
            .expect("positional ignored");
        assert!(cli.help);
        assert!(!cli.show_config);
    }

    #[test]
    fn cli_help_unknown_flags_still_error() {
        let err = CliOverrides::parse(&["--help".into(), "--definitely-unknown".into()])
            .expect_err("unknown flag still errors with --help");
        assert!(matches!(err, SettingsError::Invalid(_)));
    }

    #[test]
    fn cli_help_does_not_set_unrelated_cli_overrides_when_alone() {
        for flag in ["--help", "-h"] {
            let cli = CliOverrides::parse(&[flag.into()]).expect("parse help");
            assert!(cli.help, "{flag} sets help");
            assert!(!cli.show_config, "{flag} leaves show_config false");
            assert!(!cli.version, "{flag} leaves version false");
            assert!(!cli.docs, "{flag} leaves docs false");
            assert!(!cli.show_default, "{flag} leaves show_default false");
            assert!(cli.keybindings.is_none(), "{flag} leaves keybindings none");
            assert!(cli.overlay.is_empty(), "{flag} leaves overlay empty");
        }
    }

    #[test]
    fn startup_fetch_json_overrides_and_defaults() {
        let settings = AppSettings::from_json(
            r#"{"startup_fetch": false, "startup_fetch_command": "fastfetch"}"#,
        )
        .expect("valid json");
        assert!(!settings.startup_fetch);
        assert_eq!(settings.startup_fetch_command, "fastfetch");

        let defaults = AppSettings::default();
        assert!(defaults.startup_fetch);
        assert_eq!(defaults.startup_fetch_command, "rustfetch");
        assert_eq!(defaults.fetch_gif_path, "fetch/default.gif");

        let effective = defaults.effective_config();
        assert!(effective.startup_fetch);
        assert_eq!(effective.startup_fetch_command, "rustfetch");
        assert_eq!(effective.fetch_gif_path, "fetch/default.gif");
    }

    fn unique_stamp() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos()
    }
}

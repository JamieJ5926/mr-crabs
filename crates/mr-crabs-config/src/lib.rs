use std::path::{Path, PathBuf};
use std::time::Duration;

pub const PRODUCT_NAME: &str = "Mr Crabs";
pub const BUNDLE_IDENTIFIER: &str = "dev.jamie.mr-crabs";

/// G-Spot/Ghostty terminal font family.
pub const DEFAULT_FONT_FAMILY: &str = "JetBrains Mono";
/// G-Spot/Ghostty terminal font size in logical pixels.
pub const DEFAULT_FONT_SIZE: f32 = 18.0;
/// Ghostty `adjust-cell-height` percentage used by G-Spot.
pub const DEFAULT_LINE_HEIGHT_ADJUST_PERCENT: f32 = 5.0;
/// Ghostty window padding in logical pixels on each side.
pub const DEFAULT_PADDING_PX: f32 = 10.0;
/// Ghostty-compatible default scrollback limit (10,000 lines).
pub const DEFAULT_SCROLLBACK_LINES: u32 = 10_000;
pub const DEFAULT_GRID_COLS: u16 = 80;
pub const DEFAULT_GRID_ROWS: u16 = 24;
pub const DEFAULT_THEME: &str = "auto";
pub const DEFAULT_BACKGROUND_OPACITY: f32 = 1.0;
pub const DEFAULT_CURSOR_BLINK: bool = false;
pub const DEFAULT_CURSOR_TRAIL: bool = true;
pub const DEFAULT_CURSOR_TRAIL_OPACITY: f32 = 0.35;
pub const DEFAULT_CURSOR_TRAIL_DURATION_MS: u64 = 250;
pub const DEFAULT_TEXT_ANIMATION: &str = "streaming";
pub const DEFAULT_TEXT_ANIMATION_DURATION_MS: u64 = 120;
pub const DEFAULT_TEXT_ANIMATION_INTENSITY: f32 = 1.0;
/// Whether new windows auto-run the startup fetch command.
pub const DEFAULT_STARTUP_FETCH: bool = true;
/// POSIX command run on the PTY before the interactive shell starts.
pub const DEFAULT_STARTUP_FETCH_COMMAND: &str = "rustfetch";

pub const TERM_GHOSTTY: &str = "xterm-ghostty";
pub const TERM_FALLBACK: &str = "xterm-256color";
pub const COLORTERM_TRUECOLOR: &str = "truecolor";
/// Compiled terminfo path relative to a terminfo database directory.
pub const TERMINFO_ENTRY_REL: &str = "78/xterm-ghostty";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TextAnimation {
    Disabled,
    Streaming,
    Typewriter,
}

impl TextAnimation {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Disabled => "none",
            Self::Streaming => "streaming",
            Self::Typewriter => "typewriter",
        }
    }

    pub fn parse(value: &str) -> Self {
        match value {
            "none" | "disabled" => Self::Disabled,
            "typewriter" => Self::Typewriter,
            _ => Self::Streaming,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AnimationDefaults {
    pub cursor_trail: bool,
    pub cursor_trail_opacity: f32,
    pub cursor_trail_duration: Duration,
    pub text_animation: TextAnimation,
    pub text_animation_duration: Duration,
    pub text_animation_intensity: f32,
}

impl Default for AnimationDefaults {
    fn default() -> Self {
        Self {
            // Plain terminal by default: both effects are opt-in. The
            // animation feature code stays available for explicit config.
            cursor_trail: DEFAULT_CURSOR_TRAIL,
            cursor_trail_opacity: DEFAULT_CURSOR_TRAIL_OPACITY,
            cursor_trail_duration: Duration::from_millis(DEFAULT_CURSOR_TRAIL_DURATION_MS),
            text_animation: TextAnimation::parse(DEFAULT_TEXT_ANIMATION),
            text_animation_duration: Duration::from_millis(DEFAULT_TEXT_ANIMATION_DURATION_MS),
            text_animation_intensity: DEFAULT_TEXT_ANIMATION_INTENSITY,
        }
    }
}

/// When the shell process exits, whether the pane closes automatically.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CloseOnExitPolicy {
    Always,
    #[default]
    Clean,
    Never,
}

impl CloseOnExitPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Always => "always",
            Self::Clean => "clean",
            Self::Never => "never",
        }
    }

    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "always" => Ok(Self::Always),
            "clean" => Ok(Self::Clean),
            "never" => Ok(Self::Never),
            other => Err(format!("invalid close-on-exit value {other:?}")),
        }
    }
}

/// Canonical user-facing setting keys (Ghostty kebab-case).
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum SettingKey {
    FontFamily,
    FontSize,
    LineHeightAdjustPercent,
    Theme,
    BackgroundOpacity,
    PaddingX,
    PaddingY,
    CursorBlink,
    ScrollbackLines,
    Shell,
    WorkingDirectory,
    DefaultGrid,
    CloseOnExit,
    CursorTrail,
    CursorTrailOpacity,
    CursorTrailDurationMs,
    TextAnimation,
    TextAnimationDurationMs,
    TextAnimationIntensity,
    AllowOsc52Write,
    AllowOsc52Read,
    StartupFetch,
    StartupFetchCommand,
}

impl SettingKey {
    pub const ALL: [Self; 23] = [
        Self::FontFamily,
        Self::FontSize,
        Self::LineHeightAdjustPercent,
        Self::Theme,
        Self::BackgroundOpacity,
        Self::PaddingX,
        Self::PaddingY,
        Self::CursorBlink,
        Self::ScrollbackLines,
        Self::Shell,
        Self::WorkingDirectory,
        Self::DefaultGrid,
        Self::CloseOnExit,
        Self::CursorTrail,
        Self::CursorTrailOpacity,
        Self::CursorTrailDurationMs,
        Self::TextAnimation,
        Self::TextAnimationDurationMs,
        Self::TextAnimationIntensity,
        Self::AllowOsc52Write,
        Self::AllowOsc52Read,
        Self::StartupFetch,
        Self::StartupFetchCommand,
    ];

    pub fn flag(self) -> &'static str {
        match self {
            Self::FontFamily => "font-family",
            Self::FontSize => "font-size",
            Self::LineHeightAdjustPercent => "adjust-cell-height",
            Self::Theme => "theme",
            Self::BackgroundOpacity => "background-opacity",
            Self::PaddingX => "window-padding-x",
            Self::PaddingY => "window-padding-y",
            Self::CursorBlink => "cursor-style-blink",
            Self::ScrollbackLines => "scrollback-limit",
            Self::Shell => "shell",
            Self::WorkingDirectory => "working-directory",
            Self::DefaultGrid => "initial-window-size",
            Self::CloseOnExit => "close-on-exit",
            Self::CursorTrail => "cursor-trail",
            Self::CursorTrailOpacity => "cursor-trail-opacity",
            Self::CursorTrailDurationMs => "cursor-trail-duration",
            Self::TextAnimation => "text-animation",
            Self::TextAnimationDurationMs => "text-animation-duration",
            Self::TextAnimationIntensity => "text-animation-intensity",
            Self::AllowOsc52Write => "clipboard-write",
            Self::AllowOsc52Read => "clipboard-read",
            Self::StartupFetch => "startup-fetch",
            Self::StartupFetchCommand => "startup-fetch-command",
        }
    }

    pub fn from_flag(name: &str) -> Option<Self> {
        match name {
            "font-family" => Some(Self::FontFamily),
            "font-size" => Some(Self::FontSize),
            "adjust-cell-height" | "line-height-adjust-percent" => {
                Some(Self::LineHeightAdjustPercent)
            }
            "theme" => Some(Self::Theme),
            "background-opacity" => Some(Self::BackgroundOpacity),
            "window-padding-x" | "padding-x" => Some(Self::PaddingX),
            "window-padding-y" | "padding-y" => Some(Self::PaddingY),
            "cursor-style-blink" | "cursor-blink" => Some(Self::CursorBlink),
            "scrollback-limit" | "scrollback-lines" => Some(Self::ScrollbackLines),
            "shell" | "command" => Some(Self::Shell),
            "working-directory" => Some(Self::WorkingDirectory),
            "initial-window-size" | "default-grid" => Some(Self::DefaultGrid),
            "close-on-exit" => Some(Self::CloseOnExit),
            "cursor-trail" => Some(Self::CursorTrail),
            "cursor-trail-opacity" => Some(Self::CursorTrailOpacity),
            "text-animation" => Some(Self::TextAnimation),
            "cursor-trail-duration" => Some(Self::CursorTrailDurationMs),
            "text-animation-duration" | "text-animation-duration-ms" => {
                Some(Self::TextAnimationDurationMs)
            }
            "text-animation-intensity" => Some(Self::TextAnimationIntensity),
            "clipboard-write" | "allow-osc52-write" => Some(Self::AllowOsc52Write),
            "clipboard-read" | "allow-osc52-read" => Some(Self::AllowOsc52Read),
            "startup-fetch" => Some(Self::StartupFetch),
            "startup-fetch-command" => Some(Self::StartupFetchCommand),
            _ => None,
        }
    }

    pub fn is_boolean(self) -> bool {
        matches!(
            self,
            Self::CursorBlink
                | Self::CursorTrail
                | Self::AllowOsc52Write
                | Self::AllowOsc52Read
                | Self::StartupFetch
        )
    }

    pub fn docs(self) -> &'static str {
        match self {
            Self::FontFamily => "Font family used by the terminal element.",
            Self::FontSize => "Font size in logical pixels.",
            Self::LineHeightAdjustPercent => {
                "Ghostty adjust-cell-height percentage applied after rounding."
            }
            Self::Theme => "Theme name; \"auto\" follows the system appearance.",
            Self::BackgroundOpacity => "Window background opacity in 0.0..=1.0.",
            Self::PaddingX => "Horizontal window padding in logical pixels.",
            Self::PaddingY => "Vertical window padding in logical pixels.",
            Self::CursorBlink => "Whether the cursor blinks.",
            Self::ScrollbackLines => "Scrollback line limit.",
            Self::Shell => "Explicit shell path; empty discovers the login shell.",
            Self::WorkingDirectory => "Working directory for spawned shells.",
            Self::DefaultGrid => "Initial window grid as COLSxROWS.",
            Self::CloseOnExit => "When to close a pane after the child exits.",
            Self::CursorTrail => "Opt-in cursor trail effect.",
            Self::CursorTrailDurationMs => "Duration of the cursor trail fade.",
            Self::CursorTrailOpacity => "Cursor trail opacity.",
            Self::TextAnimation => "Text animation mode: none, streaming, or typewriter.",
            Self::TextAnimationDurationMs => "Text animation duration in milliseconds.",
            Self::TextAnimationIntensity => "Text animation intensity.",
            Self::AllowOsc52Write => "Allow OSC 52 writes to the system clipboard.",
            Self::AllowOsc52Read => "Allow OSC 52 reads from the system clipboard.",
            Self::StartupFetch => "Run the startup fetch command in new windows.",
            Self::StartupFetchCommand => {
                "POSIX command run on the PTY before the interactive shell starts."
            }
        }
    }
}

/// Partial overlay. `None` means "inherit the lower layer".
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ConfigOverlay {
    pub font_family: Option<String>,
    pub font_size: Option<f32>,
    pub line_height_adjust_percent: Option<f32>,
    pub theme: Option<String>,
    pub background_opacity: Option<f32>,
    pub padding_x: Option<f32>,
    pub padding_y: Option<f32>,
    pub cursor_blink: Option<bool>,
    pub scrollback_lines: Option<u32>,
    pub shell: Option<String>,
    pub working_directory: Option<String>,
    pub default_grid: Option<(u16, u16)>,
    pub close_on_exit: Option<CloseOnExitPolicy>,
    pub cursor_trail: Option<bool>,
    pub cursor_trail_opacity: Option<f32>,
    pub cursor_trail_duration_ms: Option<u64>,
    pub text_animation: Option<String>,
    pub text_animation_duration_ms: Option<u64>,
    pub text_animation_intensity: Option<f32>,
    pub allow_osc52_write: Option<bool>,
    pub allow_osc52_read: Option<bool>,
    pub startup_fetch: Option<bool>,
    pub startup_fetch_command: Option<String>,
}

impl ConfigOverlay {
    pub fn is_empty(&self) -> bool {
        *self == Self::default()
    }

    pub fn merge(&mut self, over: Self) {
        if over.font_family.is_some() {
            self.font_family = over.font_family;
        }
        if over.font_size.is_some() {
            self.font_size = over.font_size;
        }
        if over.line_height_adjust_percent.is_some() {
            self.line_height_adjust_percent = over.line_height_adjust_percent;
        }
        if over.theme.is_some() {
            self.theme = over.theme;
        }
        if over.background_opacity.is_some() {
            self.background_opacity = over.background_opacity;
        }
        if over.padding_x.is_some() {
            self.padding_x = over.padding_x;
        }
        if over.padding_y.is_some() {
            self.padding_y = over.padding_y;
        }
        if over.cursor_blink.is_some() {
            self.cursor_blink = over.cursor_blink;
        }
        if over.scrollback_lines.is_some() {
            self.scrollback_lines = over.scrollback_lines;
        }
        if over.shell.is_some() {
            self.shell = over.shell;
        }
        if over.working_directory.is_some() {
            self.working_directory = over.working_directory;
        }
        if over.default_grid.is_some() {
            self.default_grid = over.default_grid;
        }
        if over.close_on_exit.is_some() {
            self.close_on_exit = over.close_on_exit;
        }
        if over.cursor_trail.is_some() {
            self.cursor_trail = over.cursor_trail;
        }
        if over.cursor_trail_opacity.is_some() {
            self.cursor_trail_opacity = over.cursor_trail_opacity;
        }
        if over.cursor_trail_duration_ms.is_some() {
            self.cursor_trail_duration_ms = over.cursor_trail_duration_ms;
        }
        if over.text_animation.is_some() {
            self.text_animation = over.text_animation;
        }
        if over.text_animation_duration_ms.is_some() {
            self.text_animation_duration_ms = over.text_animation_duration_ms;
        }
        if over.text_animation_intensity.is_some() {
            self.text_animation_intensity = over.text_animation_intensity;
        }
        if over.allow_osc52_write.is_some() {
            self.allow_osc52_write = over.allow_osc52_write;
        }
        if over.allow_osc52_read.is_some() {
            self.allow_osc52_read = over.allow_osc52_read;
        }
        if over.startup_fetch.is_some() {
            self.startup_fetch = over.startup_fetch;
        }
        if over.startup_fetch_command.is_some() {
            self.startup_fetch_command = over.startup_fetch_command;
        }
    }

    pub fn apply_into(&self, dst: &mut EffectiveConfig) {
        if let Some(v) = &self.font_family {
            dst.font_family = v.clone();
        }
        if let Some(v) = self.font_size {
            dst.font_size = v;
        }
        if let Some(v) = self.line_height_adjust_percent {
            dst.line_height_adjust_percent = v;
        }
        if let Some(v) = &self.theme {
            dst.theme = v.clone();
        }
        if let Some(v) = self.background_opacity {
            dst.background_opacity = v;
        }
        if let Some(v) = self.padding_x {
            dst.padding_x = v;
        }
        if let Some(v) = self.padding_y {
            dst.padding_y = v;
        }
        if let Some(v) = self.cursor_blink {
            dst.cursor_blink = v;
        }
        if let Some(v) = self.scrollback_lines {
            dst.scrollback_lines = v;
        }
        if let Some(v) = &self.shell {
            dst.shell = if v.is_empty() { None } else { Some(v.clone()) };
        }
        if let Some(v) = &self.working_directory {
            dst.working_directory = if v.is_empty() { None } else { Some(v.clone()) };
        }
        if let Some(v) = self.default_grid {
            dst.default_grid = v;
        }
        if let Some(v) = self.close_on_exit {
            dst.close_on_exit = v;
        }
        if let Some(v) = self.cursor_trail {
            dst.cursor_trail = v;
        }
        if let Some(v) = self.cursor_trail_opacity {
            dst.cursor_trail_opacity = v;
        }
        if let Some(v) = self.cursor_trail_duration_ms {
            dst.cursor_trail_duration_ms = v;
        }
        if let Some(v) = &self.text_animation {
            dst.text_animation = v.clone();
        }
        if let Some(v) = self.text_animation_duration_ms {
            dst.text_animation_duration_ms = v;
        }
        if let Some(v) = self.text_animation_intensity {
            dst.text_animation_intensity = v;
        }
        if let Some(v) = self.allow_osc52_write {
            dst.allow_osc52_write = v;
        }
        if let Some(v) = self.allow_osc52_read {
            dst.allow_osc52_read = v;
        }
        if let Some(v) = self.startup_fetch {
            dst.startup_fetch = v;
        }
        if let Some(v) = &self.startup_fetch_command {
            dst.startup_fetch_command = v.clone();
            if v.is_empty() {
                dst.startup_fetch = false;
            }
        }
    }

    pub fn set(&mut self, key: SettingKey, value: &str) -> Result<(), String> {
        match key {
            SettingKey::FontFamily => self.font_family = Some(value.to_string()),
            SettingKey::FontSize => self.font_size = Some(parse_f32(value, key.flag())?),
            SettingKey::LineHeightAdjustPercent => {
                self.line_height_adjust_percent = Some(parse_percent(value)?)
            }
            SettingKey::Theme => self.theme = Some(parse_theme(value)?.to_string()),
            SettingKey::BackgroundOpacity => {
                self.background_opacity = Some(parse_unit_f32(value, key.flag())?)
            }
            SettingKey::PaddingX => self.padding_x = Some(parse_f32(value, key.flag())?),
            SettingKey::PaddingY => self.padding_y = Some(parse_f32(value, key.flag())?),
            SettingKey::CursorBlink => self.cursor_blink = Some(parse_bool(value)?),
            SettingKey::ScrollbackLines => {
                self.scrollback_lines = Some(parse_u32(value, key.flag())?)
            }
            SettingKey::Shell => self.shell = Some(value.to_string()),
            SettingKey::WorkingDirectory => self.working_directory = Some(value.to_string()),
            SettingKey::DefaultGrid => self.default_grid = Some(parse_pair(value, key.flag())?),
            SettingKey::CloseOnExit => self.close_on_exit = Some(CloseOnExitPolicy::parse(value)?),
            SettingKey::CursorTrail => self.cursor_trail = Some(parse_bool(value)?),
            SettingKey::CursorTrailOpacity => {
                self.cursor_trail_opacity = Some(parse_f32(value, key.flag())?)
            }
            SettingKey::CursorTrailDurationMs => {
                let trimmed = value.strip_suffix("ms").unwrap_or(value);
                self.cursor_trail_duration_ms = Some(parse_u64(trimmed, key.flag())?);
            }
            SettingKey::TextAnimation => self.text_animation = Some(value.to_string()),
            SettingKey::TextAnimationDurationMs => {
                let trimmed = value.strip_suffix("ms").unwrap_or(value);
                self.text_animation_duration_ms = Some(parse_u64(trimmed, key.flag())?);
            }
            SettingKey::TextAnimationIntensity => {
                self.text_animation_intensity = Some(parse_f32(value, key.flag())?)
            }
            SettingKey::AllowOsc52Write => self.allow_osc52_write = Some(parse_bool(value)?),
            SettingKey::AllowOsc52Read => self.allow_osc52_read = Some(parse_bool(value)?),
            SettingKey::StartupFetch => self.startup_fetch = Some(parse_bool(value)?),
            SettingKey::StartupFetchCommand => self.startup_fetch_command = Some(value.to_string()),
        }
        Ok(())
    }
}

/// Fully resolved values for every config-owned setting.
#[derive(Clone, Debug, PartialEq)]
pub struct EffectiveConfig {
    pub font_family: String,
    pub font_size: f32,
    pub line_height_adjust_percent: f32,
    pub theme: String,
    pub background_opacity: f32,
    pub padding_x: f32,
    pub padding_y: f32,
    pub cursor_blink: bool,
    pub scrollback_lines: u32,
    pub shell: Option<String>,
    pub working_directory: Option<String>,
    pub default_grid: (u16, u16),
    pub close_on_exit: CloseOnExitPolicy,
    pub cursor_trail: bool,
    pub cursor_trail_opacity: f32,
    pub cursor_trail_duration_ms: u64,
    pub text_animation: String,
    pub text_animation_duration_ms: u64,
    pub text_animation_intensity: f32,
    pub allow_osc52_write: bool,
    pub allow_osc52_read: bool,
    pub startup_fetch: bool,
    pub startup_fetch_command: String,
}

impl Default for EffectiveConfig {
    fn default() -> Self {
        Self::defaults()
    }
}

impl EffectiveConfig {
    pub fn defaults() -> Self {
        Self {
            font_family: DEFAULT_FONT_FAMILY.to_string(),
            font_size: DEFAULT_FONT_SIZE,
            line_height_adjust_percent: DEFAULT_LINE_HEIGHT_ADJUST_PERCENT,
            theme: DEFAULT_THEME.to_string(),
            background_opacity: DEFAULT_BACKGROUND_OPACITY,
            padding_x: DEFAULT_PADDING_PX,
            padding_y: DEFAULT_PADDING_PX,
            cursor_blink: DEFAULT_CURSOR_BLINK,
            scrollback_lines: DEFAULT_SCROLLBACK_LINES,
            shell: None,
            working_directory: None,
            default_grid: (DEFAULT_GRID_COLS, DEFAULT_GRID_ROWS),
            close_on_exit: CloseOnExitPolicy::Clean,
            cursor_trail: DEFAULT_CURSOR_TRAIL,
            cursor_trail_opacity: DEFAULT_CURSOR_TRAIL_OPACITY,
            cursor_trail_duration_ms: DEFAULT_CURSOR_TRAIL_DURATION_MS,
            text_animation: DEFAULT_TEXT_ANIMATION.to_string(),
            text_animation_duration_ms: DEFAULT_TEXT_ANIMATION_DURATION_MS,
            text_animation_intensity: DEFAULT_TEXT_ANIMATION_INTENSITY,
            allow_osc52_write: false,
            allow_osc52_read: false,
            startup_fetch: DEFAULT_STARTUP_FETCH,
            startup_fetch_command: DEFAULT_STARTUP_FETCH_COMMAND.to_string(),
        }
    }

    /// `defaults < file < cli < runtime`.
    pub fn resolve(file: &ConfigOverlay, cli: &ConfigOverlay, runtime: &ConfigOverlay) -> Self {
        let mut effective = Self::defaults();
        file.apply_into(&mut effective);
        cli.apply_into(&mut effective);
        runtime.apply_into(&mut effective);
        if effective.startup_fetch_command.is_empty() {
            effective.startup_fetch = false;
        }
        effective
    }

    pub fn display_value(&self, key: SettingKey) -> String {
        match key {
            SettingKey::FontFamily => self.font_family.clone(),
            SettingKey::FontSize => format!("{}", self.font_size),
            SettingKey::LineHeightAdjustPercent => {
                format!("{}%", self.line_height_adjust_percent)
            }
            SettingKey::Theme => self.theme.clone(),
            SettingKey::BackgroundOpacity => format!("{}", self.background_opacity),
            SettingKey::PaddingX => format!("{}", self.padding_x),
            SettingKey::PaddingY => format!("{}", self.padding_y),
            SettingKey::CursorBlink => format!("{}", self.cursor_blink),
            SettingKey::ScrollbackLines => format!("{}", self.scrollback_lines),
            SettingKey::Shell => self.shell.clone().unwrap_or_default(),
            SettingKey::WorkingDirectory => self.working_directory.clone().unwrap_or_default(),
            SettingKey::DefaultGrid => {
                format!("{}x{}", self.default_grid.0, self.default_grid.1)
            }
            SettingKey::CursorTrailDurationMs => {
                format!("{}ms", self.cursor_trail_duration_ms)
            }
            SettingKey::CloseOnExit => self.close_on_exit.as_str().to_string(),
            SettingKey::CursorTrail => format!("{}", self.cursor_trail),
            SettingKey::CursorTrailOpacity => format!("{}", self.cursor_trail_opacity),
            SettingKey::TextAnimation => self.text_animation.clone(),
            SettingKey::TextAnimationDurationMs => {
                format!("{}ms", self.text_animation_duration_ms)
            }
            SettingKey::TextAnimationIntensity => {
                format!("{}", self.text_animation_intensity)
            }
            SettingKey::AllowOsc52Write => format!("{}", self.allow_osc52_write),
            SettingKey::AllowOsc52Read => format!("{}", self.allow_osc52_read),
            SettingKey::StartupFetch => format!("{}", self.startup_fetch),
            SettingKey::StartupFetchCommand => self.startup_fetch_command.clone(),
        }
    }

    pub fn animation_defaults(&self) -> AnimationDefaults {
        AnimationDefaults {
            cursor_trail: self.cursor_trail,
            cursor_trail_opacity: self.cursor_trail_opacity,
            cursor_trail_duration: Duration::from_millis(self.cursor_trail_duration_ms),
            text_animation: TextAnimation::parse(&self.text_animation),
            text_animation_duration: Duration::from_millis(self.text_animation_duration_ms),
            text_animation_intensity: self.text_animation_intensity,
        }
    }

    pub fn show_config_lines(&self, docs: bool) -> Vec<String> {
        let mut lines = Vec::new();
        for key in SettingKey::ALL {
            if docs {
                lines.push(format!("# {}", key.docs()));
            }
            lines.push(format!("{} = {}", key.flag(), self.display_value(key)));
        }
        lines
    }
}

/// Child environment for a spawned shell.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ChildTerminfo {
    pub term: String,
    pub colorterm: String,
    pub terminfo_dir: Option<PathBuf>,
}

impl ChildTerminfo {
    pub fn env_pairs(&self) -> Vec<(String, String)> {
        let mut pairs = vec![
            ("TERM".to_string(), self.term.clone()),
            ("COLORTERM".to_string(), self.colorterm.clone()),
        ];
        if let Some(dir) = &self.terminfo_dir {
            pairs.push(("TERMINFO".to_string(), dir.display().to_string()));
        }
        pairs
    }
}

/// Resolve TERM/COLORTERM/TERMINFO for a child process.
///
/// `TERMINFO` is set only when a candidate directory contains
/// `78/xterm-ghostty`. Missing entry falls back to `TERM=xterm-256color`.
pub fn resolve_child_terminfo() -> ChildTerminfo {
    resolve_child_terminfo_from(&terminfo_search_paths())
}

pub fn resolve_child_terminfo_from(candidates: &[PathBuf]) -> ChildTerminfo {
    for candidate in candidates {
        if let Some(dir) = resolve_terminfo_dir(candidate) {
            return ChildTerminfo {
                term: TERM_GHOSTTY.to_string(),
                colorterm: COLORTERM_TRUECOLOR.to_string(),
                terminfo_dir: Some(dir),
            };
        }
    }
    ChildTerminfo {
        term: TERM_FALLBACK.to_string(),
        colorterm: COLORTERM_TRUECOLOR.to_string(),
        terminfo_dir: None,
    }
}

pub fn resolve_terminfo_dir(path: &Path) -> Option<PathBuf> {
    if path.join(TERMINFO_ENTRY_REL).is_file() {
        return Some(path.to_path_buf());
    }
    let nested = path.join("terminfo");
    if nested.join(TERMINFO_ENTRY_REL).is_file() {
        return Some(nested);
    }
    None
}

/// Dev checkout, cargo target, and current packaging layouts.
pub fn terminfo_search_paths() -> Vec<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(value) = std::env::var("TERMINFO") {
        if !value.is_empty() {
            dirs.push(PathBuf::from(value));
        }
    }
    if let Ok(value) = std::env::var("GHOSTTY_RESOURCES_DIR") {
        if !value.is_empty() {
            let root = PathBuf::from(value);
            dirs.push(root.join("terminfo"));
            dirs.push(root);
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            dirs.push(dir.join("terminfo"));
            dirs.push(dir.join("resources/terminfo"));
            dirs.push(dir.join("../Resources/terminfo"));
            dirs.push(dir.join("../resources/terminfo"));
            dirs.push(dir.join("../share/terminfo"));
            dirs.push(dir.join("../share/ghostty/terminfo"));
            dirs.push(dir.join("../../resources/terminfo"));
            dirs.push(dir.join("../../../resources/terminfo"));
        }
    }
    dirs.push(PathBuf::from("resources/terminfo"));
    dirs.push(PathBuf::from("terminfo"));
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    dirs.push(manifest.join("../../resources/terminfo"));
    dirs.push(manifest.join("../resources/terminfo"));
    dirs.push(manifest.join("resources/terminfo"));
    dirs
}

fn parse_bool(value: &str) -> Result<bool, String> {
    match value {
        "1" | "true" | "True" | "TRUE" | "yes" | "Yes" | "on" | "ON" => Ok(true),
        "0" | "false" | "False" | "FALSE" | "no" | "No" | "off" | "OFF" => Ok(false),
        other => Err(format!("invalid boolean {other:?}")),
    }
}

fn parse_f32(value: &str, flag: &str) -> Result<f32, String> {
    let parsed = value
        .parse::<f32>()
        .map_err(|_| format!("invalid {flag} value {value:?}"))?;
    parsed
        .is_finite()
        .then_some(parsed)
        .ok_or_else(|| format!("invalid {flag} value {value:?}"))
}

fn parse_unit_f32(value: &str, flag: &str) -> Result<f32, String> {
    let parsed = parse_f32(value, flag)?;
    (0.0..=1.0)
        .contains(&parsed)
        .then_some(parsed)
        .ok_or_else(|| format!("invalid {flag} value {value:?}, expected 0.0..=1.0"))
}

fn parse_theme(value: &str) -> Result<&str, String> {
    match value {
        "auto" | "dark" | "light" => Ok(value),
        _ => Err(format!(
            "invalid theme value {value:?}, expected auto, dark, or light"
        )),
    }
}

fn parse_u32(value: &str, flag: &str) -> Result<u32, String> {
    value
        .parse::<u32>()
        .map_err(|_| format!("invalid {flag} value {value:?}"))
}

fn parse_u64(value: &str, flag: &str) -> Result<u64, String> {
    value
        .parse::<u64>()
        .map_err(|_| format!("invalid {flag} value {value:?}"))
}

fn parse_percent(value: &str) -> Result<f32, String> {
    let trimmed = value.strip_suffix('%').unwrap_or(value);
    parse_f32(trimmed, "adjust-cell-height")
}

fn parse_pair(value: &str, flag: &str) -> Result<(u16, u16), String> {
    let (left, right) = value
        .split_once('x')
        .or_else(|| value.split_once('X'))
        .or_else(|| value.split_once(','))
        .ok_or_else(|| format!("invalid {flag} value {value:?}, expected WxH"))?;
    let a = left
        .trim()
        .parse::<u16>()
        .map_err(|_| format!("invalid {flag} width {left:?}"))?;
    let b = right
        .trim()
        .parse::<u16>()
        .map_err(|_| format!("invalid {flag} height {right:?}"))?;
    Ok((a, b))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn layered_precedence_is_defaults_file_cli_runtime() {
        let file = ConfigOverlay {
            font_size: Some(14.0),
            theme: Some("dark".into()),
            cursor_trail: Some(true),
            ..ConfigOverlay::default()
        };
        let cli = ConfigOverlay {
            font_size: Some(16.0),
            padding_x: Some(4.0),
            ..ConfigOverlay::default()
        };
        let runtime = ConfigOverlay {
            theme: Some("solar".into()),
            ..ConfigOverlay::default()
        };

        let effective = EffectiveConfig::resolve(&file, &cli, &runtime);
        assert_eq!(effective.font_size, 16.0, "cli wins over file");
        assert_eq!(effective.theme, "solar", "runtime wins over file");
        assert_eq!(effective.padding_x, 4.0, "cli overlays defaults");
        assert!(effective.cursor_trail, "file overlays defaults");
        assert_eq!(effective.font_family, DEFAULT_FONT_FAMILY);
        assert_eq!(effective.default_grid, (80, 24));
    }

    #[test]
    fn runtime_survives_replacing_the_file_overlay() {
        let mut file = ConfigOverlay {
            font_size: Some(11.0),
            theme: Some("one".into()),
            ..ConfigOverlay::default()
        };
        let runtime = ConfigOverlay {
            font_size: Some(22.0),
            ..ConfigOverlay::default()
        };
        let first = EffectiveConfig::resolve(&file, &ConfigOverlay::default(), &runtime);
        assert_eq!(first.font_size, 22.0);
        assert_eq!(first.theme, "one");

        file.font_size = Some(13.0);
        file.theme = Some("two".into());
        let second = EffectiveConfig::resolve(&file, &ConfigOverlay::default(), &runtime);
        assert_eq!(
            second.font_size, 22.0,
            "runtime persists across file reload"
        );
        assert_eq!(second.theme, "two", "unshadowed file keys update");
    }

    #[test]
    fn show_config_lists_every_effective_key() {
        let lines = EffectiveConfig::defaults().show_config_lines(false);
        let text = lines.join("\n");
        for key in SettingKey::ALL {
            assert!(
                text.contains(&format!("{} =", key.flag())),
                "missing {}",
                key.flag()
            );
        }
        assert!(text.contains("cursor-trail-duration = 250ms"));
        assert!(text.contains("font-family = JetBrains Mono"));
        assert!(text.contains("text-animation = streaming"));
    }

    #[test]
    fn theme_and_background_opacity_validate_supported_paint_values() {
        let mut overlay = ConfigOverlay::default();
        overlay
            .set(SettingKey::Theme, "light")
            .expect("light theme");
        overlay
            .set(SettingKey::BackgroundOpacity, "0.75")
            .expect("opacity");
        assert_eq!(overlay.theme.as_deref(), Some("light"));
        assert_eq!(overlay.background_opacity, Some(0.75));
        assert!(overlay.set(SettingKey::Theme, "unknown").is_err());
        assert!(overlay.set(SettingKey::BackgroundOpacity, "1.1").is_err());
        assert!(overlay.set(SettingKey::BackgroundOpacity, "NaN").is_err());
    }

    #[test]
    fn terminfo_exists_in_dev_layout() {
        let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../resources/terminfo");
        assert!(
            dir.join(TERMINFO_ENTRY_REL).is_file(),
            "dev terminfo entry missing at {}",
            dir.display()
        );
        let resolved = resolve_child_terminfo_from(std::slice::from_ref(&dir));
        assert_eq!(resolved.term, TERM_GHOSTTY);
        assert_eq!(resolved.colorterm, COLORTERM_TRUECOLOR);
        assert_eq!(resolved.terminfo_dir.as_deref(), Some(dir.as_path()));
        let env = resolved.env_pairs();
        assert!(env.iter().any(|(k, v)| k == "TERM" && v == TERM_GHOSTTY));
        assert!(
            env.iter()
                .any(|(k, v)| k == "COLORTERM" && v == COLORTERM_TRUECOLOR)
        );
        assert!(env.iter().any(|(k, _)| k == "TERMINFO"));
    }

    #[test]
    fn terminfo_falls_back_when_entry_absent() {
        let root = std::env::temp_dir().join(format!(
            "mr-crabs-terminfo-missing-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("time")
                .as_nanos()
        ));
        fs::create_dir_all(root.join("78")).expect("temp terminfo dir");
        let resolved = resolve_child_terminfo_from(std::slice::from_ref(&root));
        assert_eq!(resolved.term, TERM_FALLBACK);
        assert_eq!(resolved.colorterm, COLORTERM_TRUECOLOR);
        assert!(resolved.terminfo_dir.is_none());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn terminfo_accepts_resources_root_and_packaging_layout() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("time")
            .as_nanos();
        let root = std::env::temp_dir().join(format!(
            "mr-crabs-terminfo-pack-{}-{}",
            std::process::id(),
            stamp
        ));
        let entry = root.join("Resources/terminfo").join(TERMINFO_ENTRY_REL);
        fs::create_dir_all(entry.parent().expect("parent")).expect("packaging layout");
        fs::write(&entry, b"ghostty").expect("entry");
        let resolved = resolve_child_terminfo_from(&[root.join("Resources")]);
        assert_eq!(resolved.term, TERM_GHOSTTY);
        assert_eq!(
            resolved.terminfo_dir.as_deref(),
            Some(root.join("Resources/terminfo").as_path())
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn cli_set_parses_canonical_and_aliased_values() {
        let mut overlay = ConfigOverlay::default();
        overlay
            .set(SettingKey::LineHeightAdjustPercent, "7%")
            .expect("percent");
        overlay
            .set(SettingKey::DefaultGrid, "100x40")
            .expect("grid");
        overlay.set(SettingKey::CursorTrail, "yes").expect("bool");
        overlay
            .set(SettingKey::CloseOnExit, "never")
            .expect("close");
        overlay
            .set(SettingKey::TextAnimationDurationMs, "90ms")
            .expect("duration");
        assert_eq!(overlay.line_height_adjust_percent, Some(7.0));
        assert_eq!(overlay.default_grid, Some((100, 40)));
        assert_eq!(overlay.cursor_trail, Some(true));
        assert_eq!(overlay.close_on_exit, Some(CloseOnExitPolicy::Never));
        assert_eq!(overlay.text_animation_duration_ms, Some(90));
    }

    #[test]
    fn osc52_permissions_default_deny_and_layer_explicitly() {
        let defaults = EffectiveConfig::defaults();
        assert!(!defaults.allow_osc52_write);
        assert!(!defaults.allow_osc52_read);

        let mut runtime = ConfigOverlay::default();
        runtime
            .set(SettingKey::AllowOsc52Write, "true")
            .expect("write permission");
        runtime
            .set(SettingKey::AllowOsc52Read, "on")
            .expect("read permission");
        let effective = EffectiveConfig::resolve(
            &ConfigOverlay::default(),
            &ConfigOverlay::default(),
            &runtime,
        );
        assert!(effective.allow_osc52_write);
        assert!(effective.allow_osc52_read);
    }

    #[test]
    fn startup_fetch_round_trips_and_empty_command_disables() {
        let defaults = EffectiveConfig::defaults();
        assert!(defaults.startup_fetch);
        assert_eq!(defaults.startup_fetch_command, "rustfetch");

        let mut overlay = ConfigOverlay::default();
        overlay
            .set(SettingKey::StartupFetch, "false")
            .expect("bool");
        overlay
            .set(SettingKey::StartupFetchCommand, "neofetch")
            .expect("command");
        let effective = EffectiveConfig::resolve(
            &overlay,
            &ConfigOverlay::default(),
            &ConfigOverlay::default(),
        );
        assert!(!effective.startup_fetch);
        assert_eq!(effective.startup_fetch_command, "neofetch");

        // An explicitly empty command disables the feature.
        let mut empty = ConfigOverlay::default();
        empty
            .set(SettingKey::StartupFetchCommand, "")
            .expect("empty command");
        let effective =
            EffectiveConfig::resolve(&empty, &ConfigOverlay::default(), &ConfigOverlay::default());
        assert!(!effective.startup_fetch);
        assert_eq!(effective.startup_fetch_command, "");
    }

    #[test]
    fn startup_fetch_cross_layer_empty_command_forces_disabled_even_when_enabled() {
        let mut file = ConfigOverlay::default();
        file.set(SettingKey::StartupFetch, "true").expect("true");
        file.set(SettingKey::StartupFetchCommand, "fastfetch")
            .expect("cmd");
        let mut cli = ConfigOverlay::default();
        cli.set(SettingKey::StartupFetchCommand, "").expect("empty");
        let effective = EffectiveConfig::resolve(&file, &cli, &ConfigOverlay::default());
        assert_eq!(effective.startup_fetch_command, "");
        assert!(
            !effective.startup_fetch,
            "empty final command must force startup_fetch=false after all overlays"
        );

        let mut runtime = ConfigOverlay::default();
        runtime
            .set(SettingKey::StartupFetch, "true")
            .expect("runtime true");
        let effective2 = EffectiveConfig::resolve(&file, &cli, &runtime);
        assert_eq!(effective2.startup_fetch_command, "");
        assert!(
            !effective2.startup_fetch,
            "runtime true must not override empty-command normalization"
        );
    }
}

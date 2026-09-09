//! The Mr Crabs product binary (`mr-crabs`).
//!
//! Boots the pinned GPUI macOS platform, builds the
//! headless shell model, installs menus and keybindings from the shell
//! keymap/menu model, registers every shell action, and opens one window
//! whose root view renders the focused pane's `TerminalElement`.
//!
//! PTY lifecycle is owned by the model (`AppModel`), bounded by the
//! `mr-crabs-pty` queues, and shut down deterministically when panes, tabs,
//! or windows close (and on quit). This binary launches no other GUI and
//! performs no network I/O: update checks and crash reporting are the
//! explicit disabled/local implementations from the shell model.
//!
//! PTY readers notify one bounded/coalesced foreground task after output is
//! queued. That task pumps the model and refreshes GPUI; no frame or timer
//! polling is used to discover shell output.

use gpui::{App, AppContext as _, QuitMode};
use std::borrow::Cow;
use std::io::Write;

use gpui_platform::application;

use mr_crabs_app::action::AppAction;
use mr_crabs_app::animated_fetch;
use mr_crabs_app::model::app_model::AppModel;
use mr_crabs_app::restore::RestoreStore;
use mr_crabs_app::settings::{
    ANIMATION_OSC_KEY, ANIMATION_PRESETS, AnimationTuiMode, CliOverrides, SettingsError,
    SettingsStore, animation_menu_text, animation_preset, load_effective_from_cli,
};
use mr_crabs_app::ui::{self, AppShell};
use mr_crabs_config::{EffectiveConfig, SettingKey};

fn embedded_terminal_fonts() -> Vec<Cow<'static, [u8]>> {
    vec![
        Cow::Borrowed(&include_bytes!("../../assets/fonts/JetBrainsMono-Variable.ttf")[..]),
        Cow::Borrowed(&include_bytes!("../../assets/fonts/JetBrainsMono-Bold.ttf")[..]),
        Cow::Borrowed(&include_bytes!("../../assets/fonts/JetBrainsMono-Italic-Variable.ttf")[..]),
        Cow::Borrowed(&include_bytes!("../../assets/fonts/SymbolsNerdFontMono-Regular.ttf")[..]),
    ]
}

fn help_text() -> String {
    let version = env!("CARGO_PKG_VERSION");
    let defaults = EffectiveConfig::defaults();
    let mut out = String::new();
    out.push_str(&format!("Mr Crabs {version}\n"));
    out.push_str("Usage: mr-crabs [OPTIONS]\n");
    out.push('\n');
    out.push_str("Options:\n");
    out.push_str("  -h, --help\n");
    out.push_str("  --version, +version\n");
    out.push_str("  +show-config\n");
    out.push_str("  --docs\n");
    out.push_str("  --default\n");
    out.push_str("  --config-file <PATH>\n");
    out.push_str("  --keybindings <JSON>\n");
    out.push('\n');
    out.push_str("Animation testing:\n");
    out.push_str("  --animation [list|<name>]\n");
    out.push_str("  --animation=<name>\n");
    out.push_str("  +animation\n");
    out.push_str("  +animation [menu|list|<name>]\n");
    out.push_str("  +animation=<name>\n");
    out.push_str("    +animation <name> switches only the calling window immediately.\n");
    out.push_str(&format!(
        "    Bare --animation and --animation list print the menu. Named presets: {}.\n",
        ANIMATION_PRESETS
            .iter()
            .map(|preset| preset.name)
            .collect::<Vec<_>>()
            .join(", ")
    ));
    out.push_str("    Bare +animation and +animation menu open the interactive animation TUI.\n");
    out.push_str("    +animation list prints the plain-text menu.\n");
    out.push('\n');
    out.push_str("Fetch replay:\n");
    out.push_str("  +fetch\n");
    out.push_str("    Replay the composed fetch in the calling window. Requires the mr-crabs binary\n");
    out.push_str("    directory on the child PATH (set automatically for new panes).\n");

    out.push_str("  Inside Mr Crabs, press Cmd+Shift+P and run:\n");
    out.push_str(&format!(
        "    {}\n",
        AppAction::SetTextAnimationNone.title()
    ));
    out.push_str(&format!(
        "    {}\n",
        AppAction::SetTextAnimationStreaming.title()
    ));
    out.push_str(&format!(
        "    {}\n",
        AppAction::SetTextAnimationTypewriter.title()
    ));
    out.push_str(&format!("    {}\n", AppAction::ToggleCursorTrail.title()));
    out.push_str(
        "  Palette changes last for this process. Use CLI or JSON config for startup defaults.\n",
    );
    out.push('\n');
    out.push_str("Configuration flags:\n");
    for key in SettingKey::ALL {
        let flag = key.flag();
        let docs = key.docs();
        let default = defaults.display_value(key);
        if key.is_boolean() {
            out.push_str(&format!(
                "  --{flag}[=<true|false>]  {docs} (default: {default})\n"
            ));
        } else {
            out.push_str(&format!(
                "  --{flag} <VALUE>  {docs} (default: {default})\n"
            ));
        }
    }
    out
}

fn cli_output(args: &[String]) -> Result<Option<String>, SettingsError> {
    let cli = CliOverrides::parse(args)?;
    if cli.help {
        return Ok(Some(help_text()));
    }
    if cli.version {
        return Ok(Some(format!("Mr Crabs {}", env!("CARGO_PKG_VERSION"))));
    }
    if cli.animation_menu {
        return Ok(Some(animation_menu_text()));
    }
    if !cli.show_config {
        return Ok(None);
    }

    let settings = load_effective_from_cli(&cli)?;
    let mut lines = Vec::new();
    if cli.docs {
        lines.push("# Mr Crabs effective configuration".to_string());
    }
    lines.extend(settings.show_config_lines(cli.docs));
    Ok(Some(lines.join("\n")))
}

#[derive(Debug, PartialEq)]
enum AnimationAction {
    None,
    Tui { menu: bool },
    Emit(String),
    InvalidName(String),
}

fn animation_action(args: &[String]) -> Result<AnimationAction, SettingsError> {
    let cli = CliOverrides::parse(args)?;
    match cli.animation_tui {
        AnimationTuiMode::None => {}
        AnimationTuiMode::Bare => return Ok(AnimationAction::Tui { menu: false }),
        AnimationTuiMode::Menu => return Ok(AnimationAction::Tui { menu: true }),
    }
    let Some(name) = cli.animation_action else {
        return Ok(AnimationAction::None);
    };
    if let Some(preset) = animation_preset(&name) {
        Ok(AnimationAction::Emit(animation_emit_bytes(preset.name)))
    } else {
        Ok(AnimationAction::InvalidName(name))
    }
}

fn animation_emit_bytes(name: &str) -> String {
    format!("\x1b]1337;{ANIMATION_OSC_KEY}={name}\x07")
}

fn unknown_animation_message(name: &str) -> String {
    format!(
        "Mr Crabs: invalid config: unknown animation {name}; expected one of {}",
        ANIMATION_PRESETS
            .iter()
            .map(|preset| preset.name)
            .collect::<Vec<_>>()
            .join(", ")
    )
}

fn startup_settings(args: &[String]) -> Result<SettingsStore, SettingsError> {
    let cli = CliOverrides::parse(args)?;
    SettingsStore::from_cli(&cli)
}

/// Restore entry hook: point the model at the default shell-state file and
/// load the persisted layout (if any) into the fresh model. A missing file
/// is a first launch, not an error. A corrupt or unsupported file is
/// reported and ignored so the app still starts fresh; the file is left
/// untouched for a later run to retry.
fn restore_shell_state(model: &mut AppModel) {
    let Some(path) = RestoreStore::default_path() else {
        return;
    };
    model.restore = RestoreStore::at(path.clone());
    restore_shell_state_from(model, &path);
}

fn restore_shell_state_from(model: &mut AppModel, path: &std::path::Path) {
    if !path.exists() {
        return;
    }
    let loaded = model.restore.load(path);
    match loaded {
        Ok(state) => {
            if let Err(error) = model.apply_restore_state(state) {
                eprintln!(
                    "Mr Crabs: ignoring shell state at {}: {error}",
                    path.display()
                );
            }
        }
        Err(error) => {
            eprintln!(
                "Mr Crabs: ignoring shell state at {}: {error}",
                path.display()
            );
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if animated_fetch::should_run_animated_fetch(&args) {
        animated_fetch::run_animated_fetch_and_exit();
    }
    match cli_output(&args) {
        Ok(Some(output)) => {
            println!("{output}");
            return;
        }
        Ok(None) => {}
        Err(error) => {
            eprintln!("Mr Crabs: {error}");
            std::process::exit(2);
        }
    }

    match animation_action(&args) {
        Ok(AnimationAction::None) => {}
        Ok(AnimationAction::Tui { menu }) => {
            match mr_crabs_app::animation_tui::run_animation_tui(menu) {
                Ok(_) => {}
                Err(error) => {
                    eprintln!("Mr Crabs: {error}");
                    std::process::exit(2);
                }
            }
            return;
        }
        Ok(AnimationAction::Emit(bytes)) => {
            match std::fs::OpenOptions::new().write(true).open("/dev/tty") {
                Ok(mut tty) => {
                    if let Err(error) = tty.write_all(bytes.as_bytes()) {
                        eprintln!("Mr Crabs: cannot write animation command to /dev/tty: {error}");
                        std::process::exit(2);
                    }
                }
                Err(error) => {
                    eprintln!("Mr Crabs: cannot open /dev/tty: {error}");
                    std::process::exit(2);
                }
            }
            return;
        }
        Ok(AnimationAction::InvalidName(name)) => {
            println!("{}", animation_menu_text());
            eprintln!("{}", unknown_animation_message(&name));
            std::process::exit(2);
        }
        Err(error) => {
            eprintln!("Mr Crabs: {error}");
            std::process::exit(2);
        }
    }

    let settings = match startup_settings(&args) {
        Ok(settings) => settings,
        Err(error) => {
            eprintln!("Mr Crabs: {error}");
            std::process::exit(2);
        }
    };

    application().run(|cx: &mut App| {
        cx.set_quit_mode(QuitMode::LastWindowClosed);
        cx.set_app_identity("dev.jamie.mr-crabs", "Mr Crabs");
        cx.text_system()
            .add_fonts(embedded_terminal_fonts())
            .expect("bundled terminal fonts must register");

        let (output_wake, dirty) = ui::new_output_wake();
        let model = cx.new(|_| AppModel::new_with_settings_and_output_wake(settings, output_wake));
        model.update(cx, |model, _| restore_shell_state(model));
        let shell = cx.new(|_| AppShell::new(model.clone()));
        ui::install_wake(cx, model.clone(), shell.clone(), dirty);

        cx.set_menus(ui::menus::gpui_menus(&model.read(cx).menus));
        let bindings = ui::actions::key_bindings(&model.read(cx).settings.current().keybindings);
        cx.bind_keys(bindings);
        AppShell::register_actions(&shell, cx);

        shell.update(cx, |shell, cx| {
            shell.install_window_closed_handler(cx);
            shell.sync_windows(cx);
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_and_effective_config_are_headless_cli_paths() {
        let version = cli_output(&["--version".to_string()])
            .expect("parse")
            .expect("version");
        assert!(version.starts_with("Mr Crabs "));

        let config = cli_output(&[
            "+show-config".to_string(),
            "--font-size=21".to_string(),
            "--docs".to_string(),
        ])
        .expect("parse")
        .expect("config");
        assert!(config.contains("font-size = 21"));
        assert!(config.contains("cursor-trail = true"));
        assert!(config.contains("cursor-trail-duration = 250ms"));
        assert!(config.contains("text-animation = streaming"));
        assert!(config.contains("text-animation-duration = 120ms"));
        assert!(config.contains("text-animation-intensity = 1"));
        assert!(config.contains("startup-fetch = true"));
    }

    #[test]
    fn help_contains_required_headings_and_persistence_sentence() {
        let help = cli_output(&["--help".to_string()])
            .expect("parse")
            .expect("help");
        assert!(
            help.starts_with("Mr Crabs "),
            "must start with version heading"
        );
        assert!(help.contains("Usage: mr-crabs [OPTIONS]"));
        assert!(help.contains("Options:"));
        assert!(help.contains("-h, --help"));
        assert!(help.contains("--version, +version"));
        assert!(help.contains("+show-config"));
        assert!(help.contains("--docs"));
        assert!(help.contains("--default"));
        assert!(help.contains("--config-file <PATH>"));
        assert!(help.contains("--keybindings <JSON>"));
        assert!(help.contains("Animation testing:"));
        assert!(help.contains("Inside Mr Crabs, press Cmd+Shift+P and run:"));
        assert!(help.contains("Configuration flags:"));
        assert!(
            help.contains("Palette changes last for this process. Use CLI or JSON config for startup defaults."),
            "persistence sentence missing"
        );
    }

    #[test]
    fn help_lists_every_canonical_flag_and_animation_titles_exactly_once() {
        let help = cli_output(&["--help".to_string()])
            .expect("parse")
            .expect("help");
        for key in SettingKey::ALL {
            let flag = key.flag();
            let needle = format!("--{flag}");
            let count: usize = help
                .lines()
                .map(|line| {
                    let mut n = 0;
                    let mut search = 0;
                    while let Some(pos) = line[search..].find(&needle) {
                        let abs = search + pos;
                        let after = abs + needle.len();
                        let next = line[after..].chars().next();
                        if next != Some('-') {
                            n += 1;
                        }
                        search = abs + needle.len();
                    }
                    n
                })
                .sum();
            assert_eq!(
                count, 1,
                "flag --{flag} must appear exactly once in help, found {count}"
            );
            assert!(help.contains(key.docs()), "docs for {flag} missing");
            let default = EffectiveConfig::defaults().display_value(key);
            assert!(
                help.contains(&format!("(default: {default})")),
                "default for {flag} missing"
            );
            if key.is_boolean() {
                assert!(
                    help.contains(&format!("--{flag}[=<true|false>]")),
                    "boolean flag --{flag} must use [=<true|false>] form"
                );
            } else {
                assert!(
                    help.contains(&format!("--{flag} <VALUE>")),
                    "non-boolean flag --{flag} must use <VALUE> form"
                );
            }
        }
        assert!(help.contains(AppAction::SetTextAnimationNone.title()));
        assert!(help.contains(AppAction::SetTextAnimationStreaming.title()));
        assert!(help.contains(AppAction::SetTextAnimationTypewriter.title()));
        assert!(help.contains(AppAction::ToggleCursorTrail.title()));
        for title in [
            AppAction::SetTextAnimationNone.title(),
            AppAction::SetTextAnimationStreaming.title(),
            AppAction::SetTextAnimationTypewriter.title(),
            AppAction::ToggleCursorTrail.title(),
        ] {
            assert_eq!(
                help.matches(title).count(),
                1,
                "title {title:?} must appear exactly once"
            );
        }
    }

    #[test]
    fn help_has_precedence_over_version_and_show_config() {
        let help_only = cli_output(&["--help".to_string()])
            .expect("parse")
            .expect("help");
        let help_with_version = cli_output(&["--help".to_string(), "--version".to_string()])
            .expect("parse")
            .expect("help");
        assert_eq!(help_with_version, help_only, "help must win over version");

        let help_with_show = cli_output(&[
            "--help".to_string(),
            "+show-config".to_string(),
            "--font-size=21".to_string(),
        ])
        .expect("parse")
        .expect("help");
        assert_eq!(help_with_show, help_only, "help must win over +show-config");

        // -h alias
        let short = cli_output(&["-h".to_string()])
            .expect("parse")
            .expect("help");
        assert_eq!(short, help_only, "-h must produce same help");
    }

    #[test]
    fn help_bypasses_missing_config_file() {
        let help = cli_output(&[
            "--help".to_string(),
            "--config-file".to_string(),
            "/definitely/missing/mr-crabs.json".to_string(),
        ])
        .expect("help must bypass missing file")
        .expect("help");
        assert!(help.contains("Usage: mr-crabs [OPTIONS]"));
        assert!(help.contains("Mr Crabs "));

        let short = cli_output(&[
            "-h".to_string(),
            "--config-file".to_string(),
            "/definitely/missing/mr-crabs.json".to_string(),
        ])
        .expect("short help must bypass missing file")
        .expect("help");
        assert!(short.contains("Usage: mr-crabs [OPTIONS]"));
    }

    #[test]
    fn help_after_complete_parse_still_rejects_unknown_flag() {
        let err = cli_output(&["--help".to_string(), "--definitely-unknown".to_string()])
            .expect_err("unknown flag with help must still error");
        let msg = err.to_string();
        assert!(msg.contains("unknown flag"), "unexpected error: {msg}");
    }

    #[test]
    fn animation_flag_prints_menu_with_every_preset() {
        let menu = cli_output(&["--animation".to_string()])
            .expect("parse")
            .expect("menu");
        for preset in ANIMATION_PRESETS {
            assert!(
                menu.contains(preset.name),
                "menu missing {}: {menu}",
                preset.name
            );
            assert!(
                menu.contains(preset.description),
                "menu missing description for {}: {menu}",
                preset.name
            );
        }
        assert!(menu.contains("mr-crabs --animation <name>"));
        assert!(menu.contains("Cmd+Shift+P"));
    }

    #[test]
    fn help_contains_animation_flag() {
        let help = cli_output(&["--help".to_string()])
            .expect("parse")
            .expect("help");
        assert!(
            help.contains("--animation [list|<name>]"),
            "help missing space form: {help}"
        );
        assert!(
            help.contains("--animation=<name>"),
            "help missing equals form: {help}"
        );
        assert!(
            help.contains("Bare --animation and --animation list print the menu."),
            "help missing bare/list menu behavior: {help}"
        );
        for preset in ANIMATION_PRESETS {
            assert!(
                help.contains(preset.name),
                "help missing preset {}: {help}",
                preset.name
            );
        }
    }

    #[test]
    fn help_contains_plus_animation_tui_menu_and_scope_sentences() {
        let help = cli_output(&["--help".to_string()])
            .expect("parse")
            .expect("help");
        assert!(
            help.contains("+animation [menu|list|<name>]"),
            "help missing +animation space form: {help}"
        );
        assert!(
            help.contains("+animation=<name>"),
            "help missing +animation equals form: {help}"
        );
        assert!(
            help.contains("+animation <name> switches only the calling window immediately."),
            "help missing +animation window-scope sentence: {help}"
        );
        assert!(
            help.contains(
                "Bare +animation and +animation menu open the interactive animation TUI."
            ),
            "help missing +animation TUI sentence: {help}"
        );
        assert!(
            help.contains("+animation list prints the plain-text menu."),
            "help missing +animation list sentence: {help}"
        );
    }

    #[test]
    fn plus_animation_list_prints_plain_menu() {
        for args in [
            vec!["+animation".to_string(), "list".to_string()],
            vec!["+animation=list".to_string()],
        ] {
            let menu = cli_output(&args).expect("parse").expect("menu");
            assert_eq!(menu, animation_menu_text());
        }
    }

    #[test]
    fn plus_animation_tui_and_named_forms_do_not_print_menu() {
        for args in [
            vec!["+animation".to_string()],
            vec!["+animation".to_string(), "menu".to_string()],
            vec!["+animation=menu".to_string()],
            vec!["+animation".to_string(), "typewriter".to_string()],
            vec!["+animation=typewriter".to_string()],
        ] {
            assert!(
                cli_output(&args).expect("parse").is_none(),
                "TUI/named forms must not print the menu: {args:?}"
            );
        }
    }

    #[test]
    fn animation_action_routes_bare_and_menu_to_tui() {
        assert_eq!(
            animation_action(&["+animation".to_string()]).expect("parse"),
            AnimationAction::Tui { menu: false },
            "bare +animation opens the TUI without the preset menu"
        );
        for args in [
            vec!["+animation".to_string(), "menu".to_string()],
            vec!["+animation=menu".to_string()],
        ] {
            assert_eq!(
                animation_action(&args).expect("parse"),
                AnimationAction::Tui { menu: true },
                "+animation menu opens the TUI on the preset menu: {args:?}"
            );
        }
    }

    #[test]
    fn plus_animation_modes_are_last_wins() {
        let preset_then_bare = animation_action(&[
            "+animation=typewriter".to_string(),
            "+animation".to_string(),
        ])
        .expect("parse");
        assert_eq!(
            preset_then_bare,
            AnimationAction::Tui { menu: false },
            "bare +animation after a preset wins"
        );

        let bare_then_preset =
            animation_action(&["+animation".to_string(), "+animation=streaming".to_string()])
                .expect("parse");
        assert_eq!(
            bare_then_preset,
            AnimationAction::Emit(animation_emit_bytes("streaming")),
            "preset after bare +animation wins"
        );

        let tui_then_list = cli_output(&[
            "+animation".to_string(),
            "+animation".to_string(),
            "list".to_string(),
        ])
        .expect("parse");
        assert_eq!(tui_then_list, Some(animation_menu_text()));

        let list_then_menu = animation_action(&[
            "+animation=list".to_string(),
            "+animation".to_string(),
            "menu".to_string(),
        ])
        .expect("parse");
        assert_eq!(
            list_then_menu,
            AnimationAction::Tui { menu: true },
            "+animation menu after +animation list wins"
        );

        let menu_then_preset = animation_action(&[
            "+animation".to_string(),
            "menu".to_string(),
            "+animation=cursor-trail".to_string(),
        ])
        .expect("parse");
        assert_eq!(
            menu_then_preset,
            AnimationAction::Emit(animation_emit_bytes("cursor-trail")),
            "named preset after +animation menu wins"
        );
    }

    #[test]
    fn animation_action_emits_exact_osc_bytes() {
        let action =
            animation_action(&["+animation".to_string(), "typewriter".to_string()]).expect("parse");
        assert_eq!(
            action,
            AnimationAction::Emit("\x1b]1337;mr_crabs_animation=typewriter\x07".to_string())
        );

        let equals = animation_action(&["+animation=streaming".to_string()]).expect("parse");
        assert_eq!(
            equals,
            AnimationAction::Emit("\x1b]1337;mr_crabs_animation=streaming\x07".to_string())
        );
    }

    #[test]
    fn animation_action_emits_exact_osc_bytes_for_every_preset() {
        for preset in ANIMATION_PRESETS {
            let args = vec!["+animation".to_string(), preset.name.to_string()];
            match animation_action(&args).expect("parse") {
                AnimationAction::Emit(bytes) => assert_eq!(
                    bytes,
                    format!("\x1b]1337;{ANIMATION_OSC_KEY}={}\x07", preset.name),
                    "exact OSC 1337 bytes for {}",
                    preset.name
                ),
                other => panic!("expected Emit for {}: {other:?}", preset.name),
            }
        }
    }

    #[test]
    fn animation_action_absent_for_non_live_forms() {
        assert_eq!(animation_action(&[]).expect("parse"), AnimationAction::None);
        assert_eq!(
            animation_action(&["--animation".to_string(), "typewriter".to_string()])
                .expect("parse"),
            AnimationAction::None,
            "--animation stays a startup-config path, not an emit action"
        );
        assert_eq!(
            animation_action(&["+animation".to_string(), "list".to_string()]).expect("parse"),
            AnimationAction::None,
            "+animation list stays a plain-text print path, not a live action"
        );
    }

    #[test]
    fn animation_action_keeps_invalid_name_distinct() {
        let action =
            animation_action(&["+animation".to_string(), "wiggle".to_string()]).expect("parse");
        assert_eq!(action, AnimationAction::InvalidName("wiggle".to_string()));
    }

    #[test]
    fn unknown_animation_message_matches_registry_error_text() {
        assert_eq!(
            unknown_animation_message("wiggle"),
            "Mr Crabs: invalid config: unknown animation wiggle; expected one of none, streaming, typewriter, cursor-trail, all"
        );
    }

    fn restore_temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "mr-crabs-restore-bin-{}-{}-{}",
            name,
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ))
    }

    #[test]
    fn restore_entry_applies_supported_layout_from_disk() {
        let mut original = AppModel::headless();
        original.dispatch(AppAction::NewSplitRight);
        original.dispatch(AppAction::NewTab);
        let dir = restore_temp_path("round-trip");
        let path = dir.join("shell-state.json");
        let mut saver = RestoreStore::at(path.clone());
        saver.save(&original).expect("save");

        let mut fresh = AppModel::headless();
        fresh.restore = RestoreStore::at(path.clone());
        restore_shell_state_from(&mut fresh, &path);
        assert_eq!(fresh.windows.len(), 1);
        let before = original.active_window().expect("original window");
        let after = fresh.active_window().expect("restored window");
        assert_eq!(after.tabs.len(), before.tabs.len());
        assert_eq!(after.tab_order.len(), before.tab_order.len());
        for (tab_id, before_tab) in &before.tabs {
            let after_tab = after.tabs.get(tab_id).expect("tab");
            assert_eq!(after_tab.tree, before_tab.tree);
            assert_eq!(after_tab.focused_pane_id(), before_tab.focused_pane_id());
        }
        assert_eq!(fresh.restore.restore_count, 1);
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn restore_entry_starts_fresh_on_invalid_state() {
        let dir = restore_temp_path("invalid");
        std::fs::create_dir_all(&dir).expect("mkdir");
        // Corrupt JSON.
        let corrupt = dir.join("corrupt.json");
        std::fs::write(&corrupt, b"{ not json").expect("write");
        let mut model = AppModel::headless();
        model.restore = RestoreStore::at(corrupt.clone());
        restore_shell_state_from(&mut model, &corrupt);
        assert_eq!(model.windows.len(), 1, "corrupt file keeps fresh model");
        assert_eq!(model.restore.restore_count, 0);
        // Unsupported version.
        let versioned = dir.join("versioned.json");
        let mut source = AppModel::headless();
        source.dispatch(AppAction::NewSplitRight);
        let mut saver = RestoreStore::at(versioned.clone());
        saver.save(&source).expect("save");
        let mut raw: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&versioned).expect("read"))
                .expect("parse");
        raw["version"] = serde_json::Value::from(999u64);
        std::fs::write(&versioned, serde_json::to_string(&raw).expect("render"))
            .expect("write");
        let mut model = AppModel::headless();
        model.restore = RestoreStore::at(versioned.clone());
        restore_shell_state_from(&mut model, &versioned);
        assert_eq!(model.windows.len(), 1, "versioned file keeps fresh model");
        assert_eq!(model.restore.restore_count, 0);
        // Missing file is a first launch.
        let mut model = AppModel::headless();
        model.restore = RestoreStore::at(dir.join("absent.json"));
        restore_shell_state_from(&mut model, &dir.join("absent.json"));
        assert_eq!(model.windows.len(), 1);
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }
}

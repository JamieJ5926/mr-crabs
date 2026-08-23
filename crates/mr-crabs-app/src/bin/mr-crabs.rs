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

use std::borrow::Cow;

use gpui::{App, AppContext as _, QuitMode};
use gpui_platform::application;

use mr_crabs_app::action::AppAction;
use mr_crabs_app::animated_fetch;
use mr_crabs_app::model::app_model::AppModel;
use mr_crabs_app::settings::{CliOverrides, SettingsError, SettingsStore, load_effective_from_cli};
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

fn startup_settings(args: &[String]) -> Result<SettingsStore, SettingsError> {
    let cli = CliOverrides::parse(args)?;
    SettingsStore::from_cli(&cli)
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
        assert!(config.contains("startup-fetch-command = \"$MR_CRABS_BIN\" +animated-fetch"));
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
}

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

use gpui::{App, AppContext as _};
use gpui_platform::application;

use mr_crabs_app::model::app_model::AppModel;
use mr_crabs_app::settings::{CliOverrides, SettingsError, SettingsStore, load_effective_from_cli};
use mr_crabs_app::ui::{self, AppShell};

fn embedded_terminal_fonts() -> Vec<Cow<'static, [u8]>> {
    vec![
        Cow::Borrowed(&include_bytes!("../../assets/fonts/JetBrainsMono-Variable.ttf")[..]),
        Cow::Borrowed(&include_bytes!("../../assets/fonts/JetBrainsMono-Bold.ttf")[..]),
        Cow::Borrowed(&include_bytes!("../../assets/fonts/JetBrainsMono-Italic-Variable.ttf")[..]),
        Cow::Borrowed(&include_bytes!("../../assets/fonts/SymbolsNerdFontMono-Regular.ttf")[..]),
    ]
}

fn cli_output(args: &[String]) -> Result<Option<String>, SettingsError> {
    let cli = CliOverrides::parse(args)?;
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
        cx.set_app_identity("dev.jamie.mr-crabs", "Mr Crabs");
        cx.text_system()
            .add_fonts(embedded_terminal_fonts())
            .expect("bundled terminal fonts must register");

        let (output_wake, dirty) = ui::new_output_wake();
        let model = cx.new(|_| AppModel::new_with_settings_and_output_wake(settings, output_wake));
        let shell = cx.new(|_| AppShell::new(model.clone()));
        ui::install_wake(cx, model.clone(), dirty);

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
        assert!(config.contains("cursor-trail = false"));
        assert!(config.contains("cursor-trail-duration = 250ms"));
        assert!(config.contains("text-animation = none"));
        assert!(config.contains("text-animation-duration = 120ms"));
        assert!(config.contains("text-animation-intensity = 1"));
    }
}

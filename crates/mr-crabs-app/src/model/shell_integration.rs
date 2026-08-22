//! Inject Ghostty-compatible OSC 133 shell-integration environment.
//!
//! Fail-closed: unknown shells, Apple `/bin/bash`, missing resources, and
//! remote/ssh wrapping are skipped. `GHOSTTY_BIN_DIR` is omitted so
//! `$GHOSTTY_BIN_DIR/ghostty +ssh` cannot wrap ssh into a missing binary.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use mr_crabs_protocols::shell::{
    Shell, ShellIntegrationFeatures, detect_shell, features_env_string,
};
use mr_crabs_pty::CommandBuilder;

/// Overlay shell-integration variables onto `env` when the resolved shell is
/// a supported local shell and the bundled resources directory exists.
pub fn inject_shell_integration_env(
    env: &mut BTreeMap<String, String>,
    explicit_shell: Option<&Path>,
    cursor_blink: bool,
) {
    let resolved = CommandBuilder::discover_shell(explicit_shell);
    let exe = resolved.to_string_lossy();
    let Some(kind) = detect_shell(&exe) else {
        return;
    };
    let Some(resources) = ghostty_resources_dir() else {
        return;
    };
    let integration_root = resources.join("shell-integration");
    if !integration_root.is_dir() {
        return;
    }

    env.insert(
        "GHOSTTY_RESOURCES_DIR".to_string(),
        resources.display().to_string(),
    );
    let features = ShellIntegrationFeatures {
        cursor: true,
        path: true,
        title: true,
        sudo: false,
        ssh_env: false,
        ssh_terminfo: false,
    };
    if let Some(value) = features_env_string(features, cursor_blink) {
        env.insert("GHOSTTY_SHELL_FEATURES".to_string(), value);
    }

    match kind {
        Shell::Zsh => {
            let zdotdir = integration_root.join("zsh");
            if !zdotdir.join(".zshenv").is_file() {
                return;
            }
            let user_zdotdir = env
                .get("ZDOTDIR")
                .cloned()
                .or_else(|| std::env::var("ZDOTDIR").ok());
            if let Some(user) = user_zdotdir {
                env.insert("GHOSTTY_ZSH_ZDOTDIR".to_string(), user);
            }
            env.insert("ZDOTDIR".to_string(), zdotdir.display().to_string());
        }
        Shell::Fish | Shell::Elvish | Shell::Nushell | Shell::Bash => {
            // zsh-first slice: other bundled shells stay Hidden (no OSC 133).
        }
    }
}

fn ghostty_resources_dir() -> Option<PathBuf> {
    let mut dirs = Vec::new();
    if let Ok(value) = std::env::var("GHOSTTY_RESOURCES_DIR") {
        if !value.is_empty() {
            dirs.push(PathBuf::from(value));
        }
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            dirs.push(dir.join("../Resources/ghostty"));
            dirs.push(dir.join("../resources/ghostty"));
            dirs.push(dir.join("resources/ghostty"));
            dirs.push(dir.join("../../resources/ghostty"));
            dirs.push(dir.join("../../../resources/ghostty"));
        }
    }
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    dirs.push(manifest.join("../../resources/ghostty"));
    dirs.push(manifest.join("../resources/ghostty"));
    dirs.push(PathBuf::from("resources/ghostty"));
    dirs.into_iter()
        .find(|path| path.join("shell-integration").is_dir())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_shell_does_not_inject() {
        let mut env = BTreeMap::new();
        inject_shell_integration_env(&mut env, Some(Path::new("/bin/sh")), false);
        assert!(!env.contains_key("GHOSTTY_RESOURCES_DIR"));
    }

    #[test]
    fn apple_bash_does_not_inject_on_macos() {
        if !cfg!(target_os = "macos") {
            return;
        }
        let mut env = BTreeMap::new();
        inject_shell_integration_env(&mut env, Some(Path::new("/bin/bash")), false);
        assert!(!env.contains_key("GHOSTTY_SHELL_FEATURES"));
    }

    #[test]
    fn zsh_injects_zdotdir_without_bin_dir() {
        if detect_shell("/bin/zsh") != Some(Shell::Zsh) {
            return;
        }
        let Some(_) = ghostty_resources_dir() else {
            return;
        };
        let mut env = BTreeMap::new();
        inject_shell_integration_env(&mut env, Some(Path::new("/bin/zsh")), false);
        if env.is_empty() {
            return;
        }
        assert!(env.contains_key("GHOSTTY_RESOURCES_DIR"));
        assert_eq!(
            env.get("GHOSTTY_SHELL_FEATURES").map(String::as_str),
            Some("cursor:steady,path,title")
        );
        assert!(
            env.get("ZDOTDIR")
                .is_some_and(|value| value.ends_with("shell-integration/zsh"))
        );
        assert!(!env.contains_key("GHOSTTY_BIN_DIR"));
    }
}

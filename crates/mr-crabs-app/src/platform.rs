//! Platform capability model: what this shell build can do on the current
//! platform. Capabilities are facts, not promises; the shell consults them
//! before attempting platform services (PTY spawn, dock, menus, ...).

use serde::{Deserialize, Serialize};

/// The platform family the shell is running on.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PlatformKind {
    MacOS,
    Linux,
    Windows,
    /// Headless test environments: no platform services at all.
    Headless,
}

/// Facts about the current platform.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct PlatformCapabilities {
    pub kind: PlatformKind,
    pub menu_bar: bool,
    pub dock: bool,
    pub secure_input_backend: bool,
    pub accessibility: bool,
    /// Floating/popup windows (quick terminal).
    pub floating_windows: bool,
    pub clipboard: bool,
    pub file_dialogs: bool,
    pub url_scheme_handler: bool,
    /// Whether real PTY sessions can be spawned.
    pub pty_spawn: bool,
    /// Whether the platform provides a system update channel. This shell
    /// never uses it: update checks are disabled or local by design.
    pub system_updates: bool,
}

impl PlatformCapabilities {
    /// The macOS product profile. Update checks stay disabled/local by
    /// design (`system_updates` is still false) — see `updates`.
    pub fn macos() -> Self {
        Self {
            kind: PlatformKind::MacOS,
            menu_bar: true,
            dock: true,
            secure_input_backend: false,
            accessibility: true,
            floating_windows: true,
            clipboard: true,
            file_dialogs: true,
            url_scheme_handler: true,
            pty_spawn: true,
            system_updates: false,
        }
    }

    /// The headless test profile: no platform services.
    pub fn headless() -> Self {
        Self {
            kind: PlatformKind::Headless,
            menu_bar: false,
            dock: false,
            secure_input_backend: false,
            accessibility: false,
            floating_windows: false,
            clipboard: false,
            file_dialogs: false,
            url_scheme_handler: false,
            pty_spawn: false,
            system_updates: false,
        }
    }

    /// The capabilities of the host this binary was compiled for.
    pub fn current() -> Self {
        #[cfg(target_os = "macos")]
        {
            Self::macos()
        }
        #[cfg(not(target_os = "macos"))]
        {
            // Non-macOS shells are out of the current product scope; report
            // a conservative profile rather than claiming services that are
            // not implemented.
            Self {
                kind: if cfg!(target_os = "linux") {
                    PlatformKind::Linux
                } else if cfg!(target_os = "windows") {
                    PlatformKind::Windows
                } else {
                    PlatformKind::Headless
                },
                menu_bar: false,
                dock: false,
                secure_input_backend: false,
                accessibility: false,
                floating_windows: false,
                clipboard: false,
                file_dialogs: false,
                url_scheme_handler: false,
                pty_spawn: false,
                system_updates: false,
            }
        }
    }

    pub fn can_spawn_pty(&self) -> bool {
        self.pty_spawn
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn headless_profile_has_no_services() {
        let caps = PlatformCapabilities::headless();
        assert_eq!(caps.kind, PlatformKind::Headless);
        assert!(!caps.menu_bar && !caps.dock && !caps.accessibility);
        assert!(!caps.can_spawn_pty());
        assert!(!caps.system_updates);
    }

    #[test]
    fn macos_profile_has_product_services_but_no_network_updates() {
        let caps = PlatformCapabilities::macos();
        assert_eq!(caps.kind, PlatformKind::MacOS);
        assert!(caps.menu_bar && caps.dock && caps.accessibility && caps.floating_windows);
        assert!(caps.can_spawn_pty());
        assert!(!caps.system_updates, "updates are disabled/local by design");
        assert!(
            !caps.secure_input_backend,
            "no platform shim yet; model state only"
        );
    }
}

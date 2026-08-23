use serde::{Deserialize, Serialize};

/// Data-free paint signal for a pane surface.
///
/// `ExternalChat` does not own transcript, input, or key routing. Mr Crabs
/// always keeps PTY, VT, and terminal input on the same process.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceMode {
    #[default]
    Terminal,
    ExternalChat,
}

pub fn is_eligible_for_external_chat(
    alt_screen: bool,
    mouse_reporting: bool,
    has_trusted_osc133: bool,
) -> bool {
    !alt_screen && !mouse_reporting && has_trusted_osc133
}

pub fn effective_mode(preferred: SurfaceMode, eligible: bool) -> SurfaceMode {
    if eligible {
        preferred
    } else {
        SurfaceMode::Terminal
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surface_mode_default_is_terminal() {
        assert_eq!(SurfaceMode::default(), SurfaceMode::Terminal);
    }

    #[test]
    fn effective_mode_fail_closed() {
        assert_eq!(
            effective_mode(SurfaceMode::ExternalChat, false),
            SurfaceMode::Terminal
        );
        assert_eq!(
            effective_mode(SurfaceMode::Terminal, false),
            SurfaceMode::Terminal
        );
        assert_eq!(
            effective_mode(SurfaceMode::ExternalChat, true),
            SurfaceMode::ExternalChat
        );
    }

    #[test]
    fn eligibility_requires_shell_semantics_without_fullscreen_modes() {
        assert!(!is_eligible_for_external_chat(true, false, true));
        assert!(!is_eligible_for_external_chat(false, true, true));
        assert!(!is_eligible_for_external_chat(false, false, false));
        assert!(is_eligible_for_external_chat(false, false, true));
    }
}

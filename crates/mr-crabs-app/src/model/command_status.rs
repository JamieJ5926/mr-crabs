//! Live command status for pane chrome.
//!
//! Boundary: this module maps [`SemanticPromptState`] and
//! [`CommandBlockSnapshot`] into presentation data. It does not parse OSC,
//! does not own the input dock overlay, and does not paint gutters.
//! [`super::input_dock`] stays the live prompt-row overlay.

use std::time::{Duration, Instant};

use mr_crabs_protocols::semantic_prompt::{CommandBlockPhase, CommandBlockSnapshot};
use mr_crabs_protocols::shell::{SemanticContent, SemanticPromptState};

use crate::accessibility_policy::AccessibilityPolicy;

const MOTION_CROSSFADE: Duration = Duration::from_millis(120);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OutputKind {
    Unknown,
    Running,
    Finished,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LiveCommandStatus {
    Hidden,
    AtPrompt,
    Editing,
    Output { kind: OutputKind },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExitOutcome {
    None,
    Success,
    Failure,
    Unknown,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StatusView {
    pub status: LiveCommandStatus,
    pub exit: ExitOutcome,
    pub duration_ms: Option<u64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StatusTransition {
    pub from: StatusView,
    pub to: StatusView,
    pub started_at: Instant,
    pub duration: Duration,
}

impl StatusView {
    pub const HIDDEN: Self = Self {
        status: LiveCommandStatus::Hidden,
        exit: ExitOutcome::None,
        duration_ms: None,
    };

    /// Map live cursor fields when no typed snapshot should override them.
    pub fn from_live(semantic: &SemanticPromptState, ever_seen_osc133: bool) -> Self {
        if !ever_seen_osc133 {
            return Self::HIDDEN;
        }
        match semantic.content {
            SemanticContent::None => Self::HIDDEN,
            SemanticContent::Prompt => Self {
                status: LiveCommandStatus::AtPrompt,
                exit: ExitOutcome::None,
                duration_ms: None,
            },
            SemanticContent::Input => Self {
                status: LiveCommandStatus::Editing,
                exit: ExitOutcome::None,
                duration_ms: None,
            },
            SemanticContent::Output => Self {
                status: LiveCommandStatus::Output {
                    kind: OutputKind::Unknown,
                },
                exit: ExitOutcome::None,
                duration_ms: None,
            },
        }
    }

    /// Overlay status from D's snapshot when OSC 133 has been seen.
    /// Running/Finished come from `snapshot.phase`, never sticky `last_exit_code`.
    pub fn from_snapshot(snapshot: &CommandBlockSnapshot, ever_seen_osc133: bool) -> Self {
        if !ever_seen_osc133 {
            return Self::HIDDEN;
        }
        match snapshot.phase {
            CommandBlockPhase::Prompt => Self {
                status: LiveCommandStatus::AtPrompt,
                exit: ExitOutcome::None,
                duration_ms: None,
            },
            CommandBlockPhase::Input => Self {
                status: LiveCommandStatus::Editing,
                exit: ExitOutcome::None,
                duration_ms: None,
            },
            CommandBlockPhase::Running => Self {
                status: LiveCommandStatus::Output {
                    kind: OutputKind::Running,
                },
                exit: ExitOutcome::None,
                duration_ms: None,
            },
            CommandBlockPhase::Finished => Self {
                status: LiveCommandStatus::Output {
                    kind: OutputKind::Finished,
                },
                exit: exit_from_code(snapshot.exit_code),
                duration_ms: snapshot.duration_ms,
            },
        }
    }

    /// Prefer snapshot phase when OSC 133 has been seen.
    pub fn from_semantic(
        _semantic: &SemanticPromptState,
        snapshot: &CommandBlockSnapshot,
        ever_seen_osc133: bool,
    ) -> Self {
        if !ever_seen_osc133 {
            return Self::HIDDEN;
        }
        Self::from_snapshot(snapshot, ever_seen_osc133)
    }
}

fn exit_from_code(code: Option<i32>) -> ExitOutcome {
    match code {
        Some(0) => ExitOutcome::Success,
        Some(_) => ExitOutcome::Failure,
        None => ExitOutcome::Unknown,
    }
}

/// Reduce Motion snaps (`duration` zero). Motion allowed uses a short
/// crossfade only for AtPrompt/Editing -> Output and Output -> AtPrompt.
/// Hidden, AtPrompt <-> Editing, Output data-only changes, and any other
/// edge snap. Never calls `detect()`.
pub fn reduce(
    policy: AccessibilityPolicy,
    from: StatusView,
    to: StatusView,
    started_at: Instant,
) -> StatusTransition {
    let duration = if from == to || !policy.allows_motion() || !animates_status_edge(from.status, to.status)
    {
        Duration::ZERO
    } else {
        MOTION_CROSSFADE
    };
    StatusTransition {
        from,
        to,
        started_at,
        duration,
    }
}

fn animates_status_edge(from: LiveCommandStatus, to: LiveCommandStatus) -> bool {
    matches!(
        (from, to),
        (
            LiveCommandStatus::AtPrompt | LiveCommandStatus::Editing,
            LiveCommandStatus::Output { .. }
        ) | (LiveCommandStatus::Output { .. }, LiveCommandStatus::AtPrompt)
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use mr_crabs_protocols::semantic_prompt::{
        Action, CommandBlockId, CommandBlockPhase, CommandBlockSnapshot, SemanticPrompt,
    };
    use mr_crabs_protocols::shell::SemanticPromptState;

    fn apply(state: &mut SemanticPromptState, action: Action, options: &str, now_ms: u64) {
        let cmd = SemanticPrompt {
            action,
            options_unvalidated: options.to_owned(),
        };
        state.apply_at(&cmd, 0, 0, now_ms);
    }

    fn view(state: &SemanticPromptState, ever: bool) -> StatusView {
        StatusView::from_semantic(state, state.command_block_snapshot(), ever)
    }

    #[test]
    fn hidden_when_never_seen_osc133() {
        let mut state = SemanticPromptState::new();
        apply(&mut state, Action::FreshLineNewPrompt, "", 0);
        assert_eq!(view(&state, false), StatusView::HIDDEN);
    }

    #[test]
    fn at_prompt_after_a() {
        let mut state = SemanticPromptState::new();
        apply(&mut state, Action::FreshLineNewPrompt, "", 0);
        let v = view(&state, true);
        assert_eq!(v.status, LiveCommandStatus::AtPrompt);
        assert_eq!(v.exit, ExitOutcome::None);
        assert_eq!(v.duration_ms, None);
    }

    #[test]
    fn editing_after_b() {
        let mut state = SemanticPromptState::new();
        apply(&mut state, Action::FreshLineNewPrompt, "", 0);
        apply(&mut state, Action::EndPromptStartInput, "", 10);
        let v = view(&state, true);
        assert_eq!(v.status, LiveCommandStatus::Editing);
        assert_eq!(v.exit, ExitOutcome::None);
    }

    #[test]
    fn running_from_snapshot_phase_not_sticky_exit() {
        let mut state = SemanticPromptState::new();
        apply(&mut state, Action::FreshLineNewPrompt, "", 0);
        apply(&mut state, Action::EndPromptStartInput, "", 10);
        apply(&mut state, Action::EndInputStartOutput, "", 20);
        apply(&mut state, Action::EndCommand, "1", 40);
        apply(&mut state, Action::FreshLineNewPrompt, "", 50);
        apply(&mut state, Action::EndInputStartOutput, "", 60);
        assert_eq!(state.last_exit_code, Some(1));
        let v = view(&state, true);
        assert_eq!(
            v.status,
            LiveCommandStatus::Output {
                kind: OutputKind::Running
            }
        );
        assert_eq!(v.exit, ExitOutcome::None);
        assert_eq!(state.command_block_snapshot().phase, CommandBlockPhase::Running);
        assert_eq!(state.command_block_snapshot().exit_code, None);
    }

    #[test]
    fn finished_success() {
        let mut state = SemanticPromptState::new();
        apply(&mut state, Action::FreshLineNewPrompt, "", 0);
        apply(&mut state, Action::EndInputStartOutput, "", 100);
        apply(&mut state, Action::EndCommand, "0", 250);
        let v = view(&state, true);
        assert_eq!(
            v.status,
            LiveCommandStatus::Output {
                kind: OutputKind::Finished
            }
        );
        assert_eq!(v.exit, ExitOutcome::Success);
        assert_eq!(v.duration_ms, Some(150));
    }

    #[test]
    fn finished_failure() {
        let mut state = SemanticPromptState::new();
        apply(&mut state, Action::FreshLineNewPrompt, "", 0);
        apply(&mut state, Action::EndInputStartOutput, "", 10);
        apply(&mut state, Action::EndCommand, "3", 40);
        let v = view(&state, true);
        assert_eq!(
            v.status,
            LiveCommandStatus::Output {
                kind: OutputKind::Finished
            }
        );
        assert_eq!(v.exit, ExitOutcome::Failure);
        assert_eq!(v.duration_ms, Some(30));
    }

    #[test]
    fn finished_missing_exit_is_unknown_not_failure() {
        let mut state = SemanticPromptState::new();
        apply(&mut state, Action::FreshLineNewPrompt, "", 0);
        apply(&mut state, Action::EndInputStartOutput, "", 10);
        apply(&mut state, Action::EndCommand, "", 40);
        let snap = state.command_block_snapshot();
        assert_eq!(snap.exit_code, None);
        assert!(!snap.failed);
        let v = view(&state, true);
        assert_eq!(
            v.status,
            LiveCommandStatus::Output {
                kind: OutputKind::Finished
            }
        );
        assert_eq!(v.exit, ExitOutcome::Unknown);
    }

    #[test]
    fn live_output_without_snapshot_override_is_unknown_kind() {
        let mut state = SemanticPromptState::new();
        apply(&mut state, Action::EndInputStartOutput, "", 0);
        let v = StatusView::from_live(&state, true);
        assert_eq!(
            v.status,
            LiveCommandStatus::Output {
                kind: OutputKind::Unknown
            }
        );
        assert_eq!(v.exit, ExitOutcome::None);
    }

    #[test]
    fn snapshot_duration_none_hides_copy() {
        let snap = CommandBlockSnapshot {
            id: CommandBlockId::Generated(1),
            phase: CommandBlockPhase::Finished,
            prompt_kind: None,
            input_start: None,
            exit_code: Some(0),
            failed: false,
            duration_ms: None,
            cmdline: None,
        };
        let v = StatusView::from_snapshot(&snap, true);
        assert_eq!(v.duration_ms, None);
        assert_eq!(v.exit, ExitOutcome::Success);
    }

    fn at_prompt() -> StatusView {
        StatusView {
            status: LiveCommandStatus::AtPrompt,
            exit: ExitOutcome::None,
            duration_ms: None,
        }
    }

    fn editing() -> StatusView {
        StatusView {
            status: LiveCommandStatus::Editing,
            exit: ExitOutcome::None,
            duration_ms: None,
        }
    }

    fn output(kind: OutputKind) -> StatusView {
        StatusView {
            status: LiveCommandStatus::Output { kind },
            exit: ExitOutcome::None,
            duration_ms: None,
        }
    }

    fn motion() -> AccessibilityPolicy {
        AccessibilityPolicy::from_flags(false, false)
    }

    fn reduced() -> AccessibilityPolicy {
        AccessibilityPolicy::from_flags(true, false)
    }

    #[test]
    fn hidden_to_prompt_zero() {
        let t = reduce(motion(), StatusView::HIDDEN, at_prompt(), Instant::now());
        assert_eq!(t.duration, Duration::ZERO);
    }

    #[test]
    fn prompt_to_editing_zero() {
        let t = reduce(motion(), at_prompt(), editing(), Instant::now());
        assert_eq!(t.duration, Duration::ZERO);
        let back = reduce(motion(), editing(), at_prompt(), Instant::now());
        assert_eq!(back.duration, Duration::ZERO);
    }

    #[test]
    fn output_running_to_finished_zero() {
        let running = output(OutputKind::Running);
        let finished = StatusView {
            status: LiveCommandStatus::Output {
                kind: OutputKind::Finished,
            },
            exit: ExitOutcome::Success,
            duration_ms: Some(40),
        };
        let t = reduce(motion(), running, finished, Instant::now());
        assert_eq!(t.duration, Duration::ZERO);
    }

    #[test]
    fn editing_to_output_nonzero() {
        let t = reduce(
            motion(),
            editing(),
            output(OutputKind::Running),
            Instant::now(),
        );
        assert_eq!(t.duration, MOTION_CROSSFADE);
    }

    #[test]
    fn output_to_at_prompt_nonzero() {
        let t = reduce(
            motion(),
            output(OutputKind::Finished),
            at_prompt(),
            Instant::now(),
        );
        assert_eq!(t.duration, MOTION_CROSSFADE);
    }

    #[test]
    fn reduce_motion_zero_duration() {
        let from = at_prompt();
        let to = output(OutputKind::Running);
        let now = Instant::now();
        let reduced = reduce(reduced(), from, to, now);
        assert_eq!(reduced.duration, Duration::ZERO);
        let allowed = reduce(motion(), from, to, now);
        assert_eq!(allowed.duration, MOTION_CROSSFADE);
        let same = reduce(motion(), from, from, now);
        assert_eq!(same.duration, Duration::ZERO);
    }
}

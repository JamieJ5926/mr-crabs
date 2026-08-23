use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceMode {
    #[default]
    Terminal,
    Chat,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationSource {
    HostInput,
    #[default]
    PtySnapshot,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConversationKind {
    Input,
    Output,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ConversationEvent {
    pub id: u64,
    pub kind: ConversationKind,
    pub text: String,
    pub source: ConversationSource,
}

impl ConversationEvent {
    pub fn new(id: u64, kind: ConversationKind, text: String, source: ConversationSource) -> Self {
        Self {
            id,
            kind,
            text,
            source,
        }
    }
}

pub fn is_eligible_for_chat(
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

pub fn project_conversation_events(
    events: &[ConversationEvent],
    eligible: bool,
    mode: SurfaceMode,
) -> Vec<ConversationEvent> {
    if !eligible || mode != SurfaceMode::Chat {
        return Vec::new();
    }
    events.to_vec()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn surface_mode_default_is_terminal() {
        assert_eq!(SurfaceMode::default(), SurfaceMode::Terminal);
    }

    #[test]
    fn conversation_source_default_is_pty_snapshot() {
        assert_eq!(
            ConversationSource::default(),
            ConversationSource::PtySnapshot
        );
    }

    #[test]
    fn conversation_event_is_immutable_clone() {
        let a = ConversationEvent::new(
            1,
            ConversationKind::Input,
            "hi".into(),
            ConversationSource::HostInput,
        );
        let b = a.clone();
        assert_eq!(a, b);
        assert_eq!(a.id, 1);
    }

    #[test]
    fn effective_mode_fail_closed() {
        assert_eq!(
            effective_mode(SurfaceMode::Chat, false),
            SurfaceMode::Terminal
        );
        assert_eq!(
            effective_mode(SurfaceMode::Terminal, false),
            SurfaceMode::Terminal
        );
        assert_eq!(effective_mode(SurfaceMode::Chat, true), SurfaceMode::Chat);
    }

    #[test]
    fn eligibility_requires_shell_semantics_without_fullscreen_modes() {
        assert!(!is_eligible_for_chat(true, false, true));
        assert!(!is_eligible_for_chat(false, true, true));
        assert!(!is_eligible_for_chat(false, false, false));
        assert!(is_eligible_for_chat(false, false, true));
    }

    #[test]
    fn projection_empty_when_not_eligible_or_terminal() {
        let ev = vec![ConversationEvent::new(
            1,
            ConversationKind::Input,
            "x".into(),
            ConversationSource::HostInput,
        )];
        assert!(project_conversation_events(&ev, false, SurfaceMode::Chat).is_empty());
        assert!(project_conversation_events(&ev, true, SurfaceMode::Terminal).is_empty());
        assert_eq!(
            project_conversation_events(&ev, true, SurfaceMode::Chat),
            ev
        );
    }
}

use std::collections::VecDeque;

use mr_crabs_protocols::semantic_prompt::{
    Action, Option as SemanticOption, OptionValue, SemanticPrompt,
};

use super::presentation::{ConversationEvent, ConversationKind, ConversationSource};

const INPUT_CAP: usize = 200;
const DRAFT_CAP_BYTES: usize = 64 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AgentLaunchSpec {
    pub argv: Vec<String>,
}

impl Default for AgentLaunchSpec {
    fn default() -> Self {
        Self {
            argv: vec!["omp".to_owned()],
        }
    }
}

impl AgentLaunchSpec {
    pub fn command_line(&self, prompt: &str) -> Result<Vec<u8>, ChatSubmitError> {
        if self.argv.is_empty() {
            return Err(ChatSubmitError::MissingCommand);
        }
        if prompt.is_empty() {
            return Err(ChatSubmitError::EmptyDraft);
        }
        if contains_control(prompt) || self.argv.iter().any(|part| contains_control(part)) {
            return Err(ChatSubmitError::ControlCharacter);
        }

        let mut command = String::new();
        for part in self
            .argv
            .iter()
            .map(String::as_str)
            .chain(std::iter::once(prompt))
        {
            if !command.is_empty() {
                command.push(' ');
            }
            quote_posix(part, &mut command);
        }
        command.push('\r');
        Ok(command.into_bytes())
    }
}

fn quote_posix(value: &str, out: &mut String) {
    out.push('\'');
    for ch in value.chars() {
        if ch == '\'' {
            out.push_str("'\\''");
        } else {
            out.push(ch);
        }
    }
    out.push('\'');
}
fn contains_control(value: &str) -> bool {
    value.chars().any(char::is_control)
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum AgentSessionState {
    #[default]
    Idle,
    Launching {
        input_id: u64,
    },
    Running {
        input_id: u64,
    },
    Exited {
        code: Option<i32>,
    },
}

impl AgentSessionState {
    pub(crate) fn keeps_chat_available(self) -> bool {
        matches!(self, Self::Launching { .. } | Self::Running { .. })
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ChatSubmitError {
    EmptyDraft,
    MissingCommand,
    ControlCharacter,
    AgentLaunching,
    PtyWrite,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PreparedChatSubmit {
    pub(crate) bytes: Vec<u8>,
    event: ConversationEvent,
    launch: bool,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ChatSession {
    state: AgentSessionState,
    draft: String,
    inputs: VecDeque<ConversationEvent>,
    next_event_id: u64,
}

impl Default for ChatSession {
    fn default() -> Self {
        Self {
            state: AgentSessionState::Idle,
            draft: String::new(),
            inputs: VecDeque::new(),
            next_event_id: 0,
        }
    }
}

impl ChatSession {
    pub fn state(&self) -> AgentSessionState {
        self.state
    }

    pub fn draft(&self) -> &str {
        &self.draft
    }

    pub fn insert(&mut self, text: &str) {
        for ch in text.chars() {
            let normalized = match ch {
                '\n' | '\r' | '\t' => Some(' '),
                _ if !ch.is_control() => Some(ch),
                _ => None,
            };
            if let Some(ch) = normalized
                && self.draft.len() + ch.len_utf8() <= DRAFT_CAP_BYTES
            {
                self.draft.push(ch);
            }
        }
    }

    pub fn backspace(&mut self) {
        self.draft.pop();
    }

    pub fn events(&self) -> impl Iterator<Item = &ConversationEvent> {
        self.inputs.iter()
    }

    pub fn prepare_launch(
        &self,
        spec: &AgentLaunchSpec,
    ) -> Result<PreparedChatSubmit, ChatSubmitError> {
        if !matches!(
            self.state,
            AgentSessionState::Idle | AgentSessionState::Exited { .. }
        ) {
            return Err(ChatSubmitError::AgentLaunching);
        }
        let bytes = spec.command_line(&self.draft)?;
        Ok(self.prepared(bytes, true))
    }

    pub fn prepare_follow_up(&self, bytes: Vec<u8>) -> Result<PreparedChatSubmit, ChatSubmitError> {
        if self.draft.is_empty() {
            return Err(ChatSubmitError::EmptyDraft);
        }
        if !matches!(self.state, AgentSessionState::Running { .. }) {
            return Err(ChatSubmitError::AgentLaunching);
        }
        Ok(self.prepared(bytes, false))
    }

    fn prepared(&self, bytes: Vec<u8>, launch: bool) -> PreparedChatSubmit {
        PreparedChatSubmit {
            bytes,
            event: ConversationEvent::new(
                self.next_event_id,
                ConversationKind::Input,
                self.draft.clone(),
                ConversationSource::HostInput,
            ),
            launch,
        }
    }

    pub fn commit_submit(&mut self, prepared: PreparedChatSubmit) {
        let input_id = prepared.event.id;
        self.inputs.push_back(prepared.event);
        while self.inputs.len() > INPUT_CAP {
            self.inputs.pop_front();
        }
        self.next_event_id = self.next_event_id.saturating_add(1);
        self.draft.clear();
        if prepared.launch {
            self.state = AgentSessionState::Launching { input_id };
        }
    }

    pub fn apply_semantic(&mut self, command: &SemanticPrompt) {
        match command.action {
            Action::EndInputStartOutput => {
                if let AgentSessionState::Launching { input_id } = self.state {
                    self.state = AgentSessionState::Running { input_id };
                }
            }
            Action::EndCommand => {
                if matches!(
                    self.state,
                    AgentSessionState::Launching { .. } | AgentSessionState::Running { .. }
                ) {
                    let code = match command.read_option(SemanticOption::ExitCode) {
                        Some(OptionValue::ExitCode(code)) => Some(code),
                        _ => None,
                    };
                    self.state = AgentSessionState::Exited { code };
                }
            }
            _ => {}
        }
    }
    pub(crate) fn outer_pty_exited(&mut self, code: Option<i32>) {
        self.state = AgentSessionState::Exited { code };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_command_quotes_every_argument_and_prompt() {
        let spec = AgentLaunchSpec {
            argv: vec!["omp".into(), "--profile".into(), "work space".into()],
        };
        assert_eq!(
            spec.command_line("say 'hello'").unwrap(),
            b"'omp' '--profile' 'work space' 'say '\\''hello'\\'''\r"
        );
    }

    #[test]
    fn launch_command_rejects_missing_command_empty_prompt_and_controls() {
        assert_eq!(
            AgentLaunchSpec { argv: vec![] }.command_line("hi"),
            Err(ChatSubmitError::MissingCommand)
        );
        assert_eq!(
            AgentLaunchSpec::default().command_line(""),
            Err(ChatSubmitError::EmptyDraft)
        );
        for prompt in [
            "a\0b",
            "two\nlines",
            "carriage\rreturn",
            "tab\tstop",
            "\u{1b}escape",
        ] {
            assert_eq!(
                AgentLaunchSpec::default().command_line(prompt),
                Err(ChatSubmitError::ControlCharacter)
            );
        }
    }
    #[test]
    fn draft_normalizes_line_controls_and_drops_terminal_controls() {
        let mut chat = ChatSession::default();
        chat.insert("one\ntwo\tthree\u{1b}");
        assert_eq!(chat.draft(), "one two three");
    }
    #[test]
    fn draft_is_bounded_without_splitting_utf8() {
        let mut chat = ChatSession::default();
        chat.insert(&"a".repeat(DRAFT_CAP_BYTES - 1));
        chat.insert("é");
        assert_eq!(chat.draft().len(), DRAFT_CAP_BYTES - 1);
        chat.insert("b");
        assert_eq!(chat.draft().len(), DRAFT_CAP_BYTES);
    }

    #[test]
    fn submit_commits_only_after_the_writer_succeeds() {
        let mut chat = ChatSession::default();
        chat.insert("hello");
        let prepared = chat.prepare_launch(&AgentLaunchSpec::default()).unwrap();
        assert_eq!(chat.state(), AgentSessionState::Idle);
        assert!(chat.events().next().is_none());
        assert_eq!(chat.draft(), "hello");

        chat.commit_submit(prepared);
        assert_eq!(chat.state(), AgentSessionState::Launching { input_id: 0 });
        assert_eq!(chat.events().next().unwrap().text, "hello");
        assert!(chat.draft().is_empty());
    }

    #[test]
    fn semantic_command_boundaries_track_the_nested_agent() {
        let mut chat = ChatSession::default();
        chat.insert("hello");
        let prepared = chat.prepare_launch(&AgentLaunchSpec::default()).unwrap();
        chat.commit_submit(prepared);
        chat.apply_semantic(&SemanticPrompt::new(Action::EndInputStartOutput));

        assert_eq!(chat.state(), AgentSessionState::Running { input_id: 0 });

        let mut end = SemanticPrompt::new(Action::EndCommand);
        end.options_unvalidated = "7".into();
        chat.apply_semantic(&end);
        assert_eq!(chat.state(), AgentSessionState::Exited { code: Some(7) });
    }

    #[test]
    fn outer_pty_exit_releases_a_running_session() {
        let mut chat = ChatSession::default();
        chat.insert("hello");
        let prepared = chat.prepare_launch(&AgentLaunchSpec::default()).unwrap();
        chat.commit_submit(prepared);
        chat.apply_semantic(&SemanticPrompt::new(Action::EndInputStartOutput));
        chat.outer_pty_exited(None);
        assert_eq!(chat.state(), AgentSessionState::Exited { code: None });
    }

    #[test]
    fn unrelated_semantic_events_do_not_invent_a_session() {
        let mut chat = ChatSession::default();
        chat.apply_semantic(&SemanticPrompt::new(Action::EndInputStartOutput));
        chat.apply_semantic(&SemanticPrompt::new(Action::EndCommand));
        assert_eq!(chat.state(), AgentSessionState::Idle);
    }
}

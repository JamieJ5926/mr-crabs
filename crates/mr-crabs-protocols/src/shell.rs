//! Shell integration event state, ported from Ghostty
//! `src/termio/shell_integration.zig` (feature detection and the
//! `GHOSTTY_SHELL_FEATURES` environment string) and the terminal-side
//! semantic-prompt state tracking from `src/terminal/Terminal.zig`
//! (`semanticPrompt`, `cursorIsAtPrompt`).
//!
//! This module is pure state: it never executes a shell. The PTY layer uses
//! [`detect_shell`] and [`features_env_string`] when spawning; the terminal
//! layer feeds OSC 133 commands through [`SemanticPromptState::apply`].

/// Shell types we support (Ghostty `Shell`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Shell {
    Bash,
    Elvish,
    Fish,
    Nushell,
    Zsh,
}

/// Detect the shell from the executable basename of the spawn command.
///
/// Like Ghostty, Apple's `/bin/bash` (SIP-protected, ENV startup path
/// disabled) is deliberately NOT detected so automatic integration is
/// skipped there.
pub fn detect_shell(exe: &str) -> Option<Shell> {
    let base = std::path::Path::new(exe)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| exe.to_owned());
    match base.as_str() {
        "bash" => {
            if cfg!(target_os = "macos") && exe == "/bin/bash" {
                return None;
            }
            Some(Shell::Bash)
        }
        "elvish" => Some(Shell::Elvish),
        "fish" => Some(Shell::Fish),
        "nu" => Some(Shell::Nushell),
        "zsh" => Some(Shell::Zsh),
        _ => None,
    }
}

/// Shell integration features (Ghostty `config.ShellIntegrationFeatures`).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct ShellIntegrationFeatures {
    pub cursor: bool,
    pub sudo: bool,
    pub title: bool,
    pub ssh_env: bool,
    pub ssh_terminfo: bool,
    pub path: bool,
}

/// Build the deterministic `GHOSTTY_SHELL_FEATURES` value (features sorted
/// case-insensitively, `cursor` suffixed with `:blink`/`:steady`), or `None`
/// when no feature is enabled (Ghostty `setupFeatures`).
pub fn features_env_string(
    features: ShellIntegrationFeatures,
    cursor_blink: bool,
) -> Option<String> {
    let mut enabled = Vec::new();
    if features.cursor {
        enabled.push(if cursor_blink {
            "cursor:blink"
        } else {
            "cursor:steady"
        });
    }
    if features.path {
        enabled.push("path");
    }
    if features.ssh_env {
        enabled.push("ssh-env");
    }
    if features.ssh_terminfo {
        enabled.push("ssh-terminfo");
    }
    if features.sudo {
        enabled.push("sudo");
    }
    if features.title {
        enabled.push("title");
    }
    // Ghostty sorts the field names case-insensitively; the literal feature
    // strings sort the same way here because the only case difference is
    // "cursor", which sorts before lowercase names case-insensitively.
    enabled.sort_by_key(|a| a.to_ascii_lowercase());
    if enabled.is_empty() {
        None
    } else {
        Some(enabled.join(","))
    }
}

/// The semantic content of the cursor (Ghostty `Screen.SemanticContent`).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SemanticContent {
    #[default]
    None,
    Prompt,
    Input,
    Output,
}

/// The semantic prompt type of a row (Ghostty `Terminal.SemanticPrompt`).
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum RowSemantic {
    #[default]
    None,
    Prompt,
    PromptContinuation,
    Input,
    Command,
}

/// The click-move option currently active (Ghostty `Screen.SemanticPrompt`).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClickMode {
    /// `click_events=1` (absolute) or `2` (relative).
    ClickEvents(super::semantic_prompt::ClickEvents),
    /// `cl=...` cursor-key handling.
    Cl(super::semantic_prompt::Click),
}

/// Terminal-side shell-integration state fed by OSC 133 commands.
///
/// The terminal integration applies the returned [`SemanticAction`]s to its
/// semantic region table; this struct tracks the pure state transitions
/// (Ghostty `Terminal.semanticPrompt` + `cursorSetSemanticContent`).
pub struct SemanticPromptState {
    /// The semantic content at the cursor.
    pub content: SemanticContent,
    /// The semantic prompt type of the cursor row.
    pub row: RowSemantic,
    /// Whether the shell redraws prompts on resize (`redraw` option).
    pub shell_redraws_prompt: RedrawState,
    /// The click-handling mode from the last prompt start.
    pub click: Option<ClickMode>,
    /// OSC 133 `k=` prompt kind from the last A/P/N, if present.
    pub prompt_kind: Option<super::semantic_prompt::PromptKind>,
    /// Viewport column of the input start, set on OSC 133 B/I.
    pub input_start_col: Option<u16>,
    /// Viewport row of the input start, set on OSC 133 B/I.
    pub input_start_row: Option<u16>,
    /// Last OSC 133 D exit code, when the option was present and valid.
    pub last_exit_code: Option<i32>,
}

/// The `redraw` option value (Ghostty `Redraw`); defaults to true.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RedrawState {
    True,
    False,
    Last,
}

/// An action the terminal layer must apply (row marking / cursor movement).
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SemanticAction {
    /// Mark the current row as a prompt (optionally with a kind).
    MarkPrompt,
    /// Mark the current row as input.
    MarkInput,
    /// Mark the current row as output.
    MarkOutput,
    /// OSC 133;L fresh-line: carriage return + index (when the cursor is not
    /// at the left margin).
    FreshLine,
    /// Clear the input marker to the end of the line.
    ClearInputEol,
    /// No grid effect.
    None,
}

impl SemanticPromptState {
    pub fn new() -> Self {
        Self {
            content: SemanticContent::None,
            row: RowSemantic::None,
            shell_redraws_prompt: RedrawState::True,
            click: None,
            prompt_kind: None,
            input_start_col: None,
            input_start_row: None,
            last_exit_code: None,
        }
    }

    /// Apply an OSC 133 command, returning the actions the terminal must
    /// perform in order.
    ///
    /// `cursor_col`/`cursor_row` are the live viewport cursor at the moment
    /// the command is applied. OSC 133 B/I record those as
    /// [`Self::input_start_col`]/[`Self::input_start_row`].
    pub fn apply(
        &mut self,
        cmd: &super::semantic_prompt::SemanticPrompt,
        cursor_col: u16,
        cursor_row: u16,
    ) -> Vec<SemanticAction> {
        use super::semantic_prompt::{Action, Option as Opt, OptionValue};
        match cmd.action {
            Action::FreshLine => vec![SemanticAction::FreshLine],
            Action::FreshLineNewPrompt | Action::NewCommand => {
                let actions = vec![SemanticAction::FreshLine, SemanticAction::MarkPrompt];
                if let Some(OptionValue::Redraw(redraw)) = cmd.read_option(Opt::Redraw) {
                    self.shell_redraws_prompt = match redraw {
                        super::semantic_prompt::Redraw::True => RedrawState::True,
                        super::semantic_prompt::Redraw::False => RedrawState::False,
                        super::semantic_prompt::Redraw::Last => RedrawState::Last,
                    };
                }
                self.click =
                    if let Some(OptionValue::ClickEvents(ce)) = cmd.read_option(Opt::ClickEvents) {
                        Some(ClickMode::ClickEvents(ce))
                    } else if let Some(OptionValue::Cl(cl)) = cmd.read_option(Opt::Cl) {
                        Some(ClickMode::Cl(cl))
                    } else {
                        None
                    };
                if let Some(OptionValue::PromptKind(kind)) = cmd.read_option(Opt::PromptKind) {
                    self.prompt_kind = Some(kind);
                } else {
                    self.prompt_kind = None;
                }
                self.input_start_col = None;
                self.input_start_row = None;
                self.content = SemanticContent::Prompt;
                self.row = RowSemantic::Prompt;
                actions
            }
            Action::PromptStart => {
                if let Some(OptionValue::PromptKind(kind)) = cmd.read_option(Opt::PromptKind) {
                    self.prompt_kind = Some(kind);
                } else {
                    self.prompt_kind = None;
                }
                self.content = SemanticContent::Prompt;
                self.row = RowSemantic::Prompt;
                vec![SemanticAction::MarkPrompt]
            }
            Action::EndPromptStartInput => {
                self.content = SemanticContent::Input;
                self.input_start_col = Some(cursor_col);
                self.input_start_row = Some(cursor_row);
                vec![SemanticAction::MarkInput]
            }
            Action::EndPromptStartInputTerminateEol => {
                self.content = SemanticContent::Input;
                self.input_start_col = Some(cursor_col);
                self.input_start_row = Some(cursor_row);
                vec![SemanticAction::ClearInputEol]
            }
            Action::EndInputStartOutput => {
                let mut actions = vec![SemanticAction::MarkOutput];
                // Heuristic for fish: at column zero on a prompt row, assume
                // we are overwriting the prompt (Ghostty comment in
                // `semanticPrompt`).
                if self.row != RowSemantic::None {
                    actions.push(SemanticAction::None);
                }
                self.content = SemanticContent::Output;
                self.row = RowSemantic::None;
                self.input_start_col = None;
                self.input_start_row = None;
                actions
            }
            Action::EndCommand => {
                if let Some(OptionValue::ExitCode(code)) = cmd.read_option(Opt::ExitCode) {
                    self.last_exit_code = Some(code);
                }
                self.content = SemanticContent::Output;
                self.row = RowSemantic::None;
                self.input_start_col = None;
                self.input_start_row = None;
                vec![SemanticAction::MarkOutput]
            }
        }
    }

    /// Whether the cursor is currently at a prompt (Ghostty
    /// `cursorIsAtPrompt`); requires shell integration.
    pub fn cursor_is_at_prompt(&self) -> bool {
        if self.row != RowSemantic::None {
            return true;
        }
        matches!(
            self.content,
            SemanticContent::Input | SemanticContent::Prompt
        )
    }
}

impl Default for SemanticPromptState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::semantic_prompt::{Action, SemanticPrompt};

    fn sp(action: Action, options: &str) -> SemanticPrompt {
        SemanticPrompt {
            action,
            options_unvalidated: options.into(),
        }
    }

    #[test]
    fn detect_shell_basename() {
        assert_eq!(detect_shell("sh"), None);
        assert_eq!(detect_shell("bash"), Some(Shell::Bash));
        assert_eq!(detect_shell("/usr/bin/bash"), Some(Shell::Bash));
        assert_eq!(detect_shell("elvish"), Some(Shell::Elvish));
        assert_eq!(detect_shell("fish"), Some(Shell::Fish));
        assert_eq!(detect_shell("nu"), Some(Shell::Nushell));
        assert_eq!(detect_shell("zsh"), Some(Shell::Zsh));
        if cfg!(target_os = "macos") {
            assert_eq!(detect_shell("/bin/bash"), None);
        }
    }

    #[test]
    fn features_env() {
        let all = ShellIntegrationFeatures {
            cursor: true,
            sudo: true,
            title: true,
            ssh_env: true,
            ssh_terminfo: true,
            path: true,
        };
        assert_eq!(
            features_env_string(all, true).as_deref(),
            Some("cursor:blink,path,ssh-env,ssh-terminfo,sudo,title")
        );
        assert_eq!(features_env_string(Default::default(), true), None);
        let mixed = ShellIntegrationFeatures {
            sudo: true,
            ssh_env: true,
            ..Default::default()
        };
        assert_eq!(
            features_env_string(mixed, false).as_deref(),
            Some("ssh-env,sudo")
        );
        let cursor = ShellIntegrationFeatures {
            cursor: true,
            ..Default::default()
        };
        assert_eq!(
            features_env_string(cursor, true).as_deref(),
            Some("cursor:blink")
        );
        assert_eq!(
            features_env_string(cursor, false).as_deref(),
            Some("cursor:steady")
        );
    }

    #[test]
    fn semantic_prompt_transitions() {
        let mut s = SemanticPromptState::new();
        assert!(!s.cursor_is_at_prompt());

        // A: prompt starts
        s.apply(&sp(Action::FreshLineNewPrompt, "k=i;redraw=1"), 0, 0);
        assert_eq!(s.content, SemanticContent::Prompt);
        assert_eq!(s.row, RowSemantic::Prompt);
        assert_eq!(
            s.prompt_kind,
            Some(crate::semantic_prompt::PromptKind::Initial)
        );
        assert!(s.cursor_is_at_prompt());
        assert_eq!(s.shell_redraws_prompt, RedrawState::True);

        // P: explicit prompt start with kind
        s.apply(&sp(Action::PromptStart, "k=s"), 0, 0);
        assert_eq!(s.row, RowSemantic::Prompt);
        assert_eq!(
            s.prompt_kind,
            Some(crate::semantic_prompt::PromptKind::Secondary)
        );

        // B: input starts
        s.apply(&sp(Action::EndPromptStartInput, ""), 2, 1);
        assert_eq!(s.content, SemanticContent::Input);
        assert_eq!(s.input_start_col, Some(2));
        assert_eq!(s.input_start_row, Some(1));
        assert!(s.cursor_is_at_prompt());

        // C: output starts
        s.apply(&sp(Action::EndInputStartOutput, ""), 2, 1);
        assert_eq!(s.content, SemanticContent::Output);
        assert_eq!(s.row, RowSemantic::None);
        assert!(!s.cursor_is_at_prompt());

        // D: command end clears the stuck-active A,B,D row
        s.apply(&sp(Action::EndCommand, "42"), 0, 0);
        assert_eq!(s.content, SemanticContent::Output);
        assert_eq!(s.row, RowSemantic::None);
        assert_eq!(s.last_exit_code, Some(42));
        assert!(!s.cursor_is_at_prompt());

        // redraw=last
        s.apply(&sp(Action::FreshLineNewPrompt, "redraw=last"), 0, 0);
        assert_eq!(s.shell_redraws_prompt, RedrawState::Last);

        // click options
        s.apply(&sp(Action::FreshLineNewPrompt, "click_events=2"), 0, 0);
        assert_eq!(
            s.click,
            Some(ClickMode::ClickEvents(
                super::super::semantic_prompt::ClickEvents::Relative
            ))
        );
        s.apply(&sp(Action::FreshLineNewPrompt, "cl=line"), 0, 0);
        assert_eq!(
            s.click,
            Some(ClickMode::Cl(super::super::semantic_prompt::Click::Line))
        );
    }

    #[test]
    fn end_command_clears_row_after_a_b_d() {
        let mut s = SemanticPromptState::new();
        s.apply(&sp(Action::FreshLineNewPrompt, "k=i"), 0, 0);
        s.apply(&sp(Action::EndPromptStartInput, ""), 1, 0);
        s.apply(&sp(Action::EndCommand, "0"), 0, 0);
        assert_eq!(s.row, RowSemantic::None);
        assert_eq!(s.content, SemanticContent::Output);
        assert!(!s.cursor_is_at_prompt());
    }
}

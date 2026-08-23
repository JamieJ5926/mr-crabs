const SHELL_KILL_LINE: char = '\u{15}';

/// Argv used to encode a control-free launch command for an idle shell PTY.
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

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LaunchByteError {
    EmptyPrompt,
    MissingCommand,
    ControlCharacter,
}

impl AgentLaunchSpec {
    /// Encode `^U` + POSIX-quoted argv + prompt + CR.
    ///
    /// The kill-line prefix clears existing shell input. Control characters
    /// in argv or prompt are rejected so the bytes stay terminal-safe.
    pub fn command_line(&self, prompt: &str) -> Result<Vec<u8>, LaunchByteError> {
        if self.argv.is_empty() {
            return Err(LaunchByteError::MissingCommand);
        }
        if prompt.is_empty() {
            return Err(LaunchByteError::EmptyPrompt);
        }
        if contains_control(prompt) || self.argv.iter().any(|part| contains_control(part)) {
            return Err(LaunchByteError::ControlCharacter);
        }

        let mut command = String::new();
        command.push(SHELL_KILL_LINE);
        let mut first = true;
        for part in self
            .argv
            .iter()
            .map(String::as_str)
            .chain(std::iter::once(prompt))
        {
            if !first {
                command.push(' ');
            }
            first = false;
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
            b"\x15'omp' '--profile' 'work space' 'say '\\''hello'\\'''\r"
        );
    }

    #[test]
    fn launch_command_rejects_missing_command_empty_prompt_and_controls() {
        assert_eq!(
            AgentLaunchSpec { argv: vec![] }.command_line("hi"),
            Err(LaunchByteError::MissingCommand)
        );
        assert_eq!(
            AgentLaunchSpec::default().command_line(""),
            Err(LaunchByteError::EmptyPrompt)
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
                Err(LaunchByteError::ControlCharacter)
            );
        }
    }

    #[test]
    fn launch_command_rejects_control_characters_in_argv() {
        let spec = AgentLaunchSpec {
            argv: vec!["omp".into(), "bad\narg".into()],
        };
        assert_eq!(
            spec.command_line("hello"),
            Err(LaunchByteError::ControlCharacter)
        );
    }
}

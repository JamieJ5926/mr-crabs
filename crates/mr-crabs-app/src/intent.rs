//! App intents: `ghostty://`-style URL intents and the bounded intent
//! router.
//!
//! The shell accepts `ghostty://open?tab=new&cwd=...` (open a window or a
//! new tab, optionally in a working directory) and `ghostty://open-url?url=
//! ...` (route a URL to the focused surface). Intent dispatch is recorded
//! in a bounded ring so headless tests can assert exactly what happened.

use std::fmt::{self, Display, Formatter};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[cfg(test)]
use crate::model::app_model::AppModel;

/// A parsed app intent.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "intent", rename_all = "snake_case")]
pub enum AppIntent {
    /// Open a window (or a new tab in the active window when `new_tab`),
    /// optionally in a working directory.
    Open { cwd: Option<PathBuf>, new_tab: bool },
    /// Route a URL to the focused surface.
    OpenUrl { url: String },
    /// Reload configuration.
    ReloadConfig,
    /// Toggle the quick terminal.
    ToggleQuickTerminal,
    /// Activate the terminal app (focus its windows).
    FocusTerminal,
    /// Quit.
    Quit,
}

impl AppIntent {
    /// Parse a `ghostty://` or `mr-crabs://` intent URL.
    ///
    /// Supported forms:
    /// - `ghostty://open` (new window)
    /// - `ghostty://open?tab=new&cwd=%2Ftmp` (new tab in a directory)
    /// - `ghostty://open-url?url=https%3A%2F%2Fexample.com`
    pub fn parse_url(input: &str) -> Result<Self, IntentError> {
        let parsed = url::Url::parse(input).map_err(|e| IntentError::Invalid(e.to_string()))?;
        match parsed.scheme() {
            "ghostty" | "mr-crabs" => {}
            other => return Err(IntentError::UnsupportedScheme(other.to_string())),
        }
        match parsed.host_str() {
            Some("open") => {
                let mut cwd: Option<PathBuf> = None;
                let mut new_tab = false;
                for (key, value) in parsed.query_pairs() {
                    match key.as_ref() {
                        "cwd" if !value.is_empty() => cwd = Some(PathBuf::from(value.as_ref())),
                        "tab" if value == "new" => new_tab = true,
                        _ => {}
                    }
                }
                Ok(AppIntent::Open { cwd, new_tab })
            }
            Some("open-url") => {
                let mut url = None;
                for (key, value) in parsed.query_pairs() {
                    if key == "url" {
                        url = Some(value.into_owned());
                    }
                }
                match url {
                    Some(url) => Ok(AppIntent::OpenUrl { url }),
                    None => Err(IntentError::MissingParameter("url".to_string())),
                }
            }
            Some(other) => Err(IntentError::UnknownHost(other.to_string())),
            None => Err(IntentError::MissingParameter("host".to_string())),
        }
    }

    /// Stable intent name for diagnostics.
    pub fn name(&self) -> &'static str {
        match self {
            AppIntent::Open { .. } => "open",
            AppIntent::OpenUrl { .. } => "open_url",
            AppIntent::ReloadConfig => "reload_config",
            AppIntent::ToggleQuickTerminal => "toggle_quick_terminal",
            AppIntent::FocusTerminal => "focus_terminal",
            AppIntent::Quit => "quit",
        }
    }
}

/// Intent parse failures.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IntentError {
    UnsupportedScheme(String),
    UnknownHost(String),
    MissingParameter(String),
    Invalid(String),
}

impl Display for IntentError {
    fn fmt(&self, f: &mut Formatter<'_>) -> fmt::Result {
        match self {
            IntentError::UnsupportedScheme(scheme) => write!(f, "unsupported scheme {scheme:?}"),
            IntentError::UnknownHost(host) => write!(f, "unknown intent host {host:?}"),
            IntentError::MissingParameter(parameter) => {
                write!(f, "missing parameter {parameter:?}")
            }
            IntentError::Invalid(message) => write!(f, "invalid intent url: {message}"),
        }
    }
}

impl std::error::Error for IntentError {}

/// Outcome of routing one intent.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IntentOutcome {
    Performed,
    NoWindow,
    Ignored(String),
}

/// One recorded dispatch.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IntentRecord {
    pub intent: AppIntent,
    pub outcome: IntentOutcome,
    pub at: u64,
}

/// Bounded intent router: records the last `max_records` dispatches.
#[derive(Clone, Debug)]
pub struct IntentRouter {
    records: Vec<IntentRecord>,
    max_records: usize,
}

impl IntentRouter {
    pub fn new() -> Self {
        Self::new_bounded(64)
    }

    pub fn new_bounded(max_records: usize) -> Self {
        Self {
            records: Vec::new(),
            max_records: max_records.max(1),
        }
    }

    /// Record one dispatch, bounding the history.
    pub fn push(&mut self, intent: AppIntent, outcome: IntentOutcome, at: u64) {
        self.records.push(IntentRecord {
            intent,
            outcome,
            at,
        });
        if self.records.len() > self.max_records {
            let excess = self.records.len() - self.max_records;
            self.records.drain(..excess);
        }
    }

    pub fn records(&self) -> &[IntentRecord] {
        &self.records
    }

    pub fn len(&self) -> usize {
        self.records.len()
    }

    pub fn is_empty(&self) -> bool {
        self.records.is_empty()
    }
}

impl Default for IntentRouter {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_open_intent_urls() {
        let intent = AppIntent::parse_url("ghostty://open").expect("bare open");
        assert_eq!(
            intent,
            AppIntent::Open {
                cwd: None,
                new_tab: false
            }
        );

        let intent = AppIntent::parse_url("ghostty://open?tab=new").expect("new tab");
        assert_eq!(
            intent,
            AppIntent::Open {
                cwd: None,
                new_tab: true
            }
        );

        let intent = AppIntent::parse_url("ghostty://open?cwd=%2Ftmp%2Fproject").expect("cwd");
        assert_eq!(
            intent,
            AppIntent::Open {
                cwd: Some(PathBuf::from("/tmp/project")),
                new_tab: false
            }
        );

        let intent = AppIntent::parse_url("ghostty://open?tab=new&cwd=%2Ftmp").expect("both");
        assert_eq!(
            intent,
            AppIntent::Open {
                cwd: Some(PathBuf::from("/tmp")),
                new_tab: true
            }
        );

        // The alias scheme is accepted too.
        let intent = AppIntent::parse_url("mr-crabs://open?tab=new").expect("alias");
        assert_eq!(
            intent,
            AppIntent::Open {
                cwd: None,
                new_tab: true
            }
        );
    }

    #[test]
    fn parses_open_url_intent() {
        let intent =
            AppIntent::parse_url("ghostty://open-url?url=https%3A%2F%2Fexample.com%2Fa%3Fb%3D1")
                .expect("open-url");
        assert_eq!(
            intent,
            AppIntent::OpenUrl {
                url: "https://example.com/a?b=1".to_string()
            }
        );
    }

    #[test]
    fn rejects_bad_intent_urls() {
        assert_eq!(
            AppIntent::parse_url("https://example.com"),
            Err(IntentError::UnsupportedScheme("https".to_string()))
        );
        assert_eq!(
            AppIntent::parse_url("ghostty://bogus"),
            Err(IntentError::UnknownHost("bogus".to_string()))
        );
        assert_eq!(
            AppIntent::parse_url("ghostty://open-url"),
            Err(IntentError::MissingParameter("url".to_string()))
        );
        assert!(AppIntent::parse_url("not a url").is_err());
    }

    #[test]
    fn intent_names_are_stable() {
        assert_eq!(
            AppIntent::Open {
                cwd: None,
                new_tab: false
            }
            .name(),
            "open"
        );
        assert_eq!(AppIntent::Quit.name(), "quit");
        assert_eq!(AppIntent::FocusTerminal.name(), "focus_terminal");
    }

    #[test]
    fn router_bounds_records() {
        let mut router = IntentRouter::new_bounded(3);
        for tick in 0..5 {
            router.push(AppIntent::FocusTerminal, IntentOutcome::Performed, tick);
        }
        assert_eq!(router.len(), 3);
        assert_eq!(router.records()[0].at, 2, "oldest records are evicted");
        assert_eq!(router.records()[2].at, 4);
    }

    #[test]
    fn dispatch_open_url_creates_tab_and_records() {
        let mut model = AppModel::headless();
        let outcome = model.handle_open_url("ghostty://open?tab=new", 7);
        assert_eq!(outcome, IntentOutcome::Performed);
        assert_eq!(model.active_window().unwrap().tabs.len(), 2);
        assert_eq!(model.intents.len(), 1);
        assert_eq!(model.intents.records()[0].at, 7);
        // FocusTerminal with no windows reports NoWindow.
        let mut empty = AppModel::headless();
        let window_id = empty.active_window.unwrap();
        empty.close_window(window_id);
        let outcome = empty.dispatch_intent(AppIntent::FocusTerminal, 8);
        assert_eq!(outcome, IntentOutcome::NoWindow);
    }
}

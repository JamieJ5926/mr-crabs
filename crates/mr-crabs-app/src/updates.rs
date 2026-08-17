//! Update checking: explicit disabled/local implementations only — this
//! shell never performs network telemetry or remote update checks.
//!
//! [`UpdateService`] is the product interface. [`DisabledUpdateService`] is
//! the explicit default. [`LocalManifestUpdateService`] compares the
//! bundled version against a local JSON manifest file
//! (`{"version": "...", "notes": "..."}`) and never touches the network.

use std::cmp::Ordering;
use std::path::PathBuf;
use std::time::SystemTime;

/// The bundled shell version.
pub const SHELL_VERSION: &str = env!("CARGO_PKG_VERSION");

/// What kind of update service is installed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UpdateServiceKind {
    Disabled,
    LocalManifest,
}

/// The result of an update check.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UpdateCheckResult {
    pub status: UpdateStatus,
    pub checked_at: SystemTime,
}

/// The status of an update check.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum UpdateStatus {
    /// Updates are explicitly disabled in this build.
    Disabled { reason: String },
    /// The installed version is current.
    UpToDate { version: String },
    /// A newer version is described by the local source.
    UpdateAvailable { version: String, notes: String },
}

/// The product update interface. Implementations MUST NOT perform network
/// I/O.
pub trait UpdateService: Send + Sync {
    fn kind(&self) -> UpdateServiceKind;
    /// Human description of the service, shown in diagnostics.
    fn description(&self) -> String;
    /// Check for updates; never performs network I/O.
    fn check(&self) -> UpdateCheckResult;
}

/// Explicit disabled implementation.
pub struct DisabledUpdateService {
    pub reason: String,
}

impl UpdateService for DisabledUpdateService {
    fn kind(&self) -> UpdateServiceKind {
        UpdateServiceKind::Disabled
    }

    fn description(&self) -> String {
        format!("disabled: {}", self.reason)
    }

    fn check(&self) -> UpdateCheckResult {
        UpdateCheckResult {
            status: UpdateStatus::Disabled {
                reason: self.reason.clone(),
            },
            checked_at: SystemTime::now(),
        }
    }
}

/// Local-manifest implementation: compares `current_version` against a
/// local JSON manifest. A missing/unreadable manifest is simply "up to
/// date" — no error is fabricated and no network is involved.
pub struct LocalManifestUpdateService {
    pub current_version: String,
    pub manifest_path: Option<PathBuf>,
}

impl LocalManifestUpdateService {
    pub fn new(current_version: impl Into<String>) -> Self {
        Self {
            current_version: current_version.into(),
            manifest_path: None,
        }
    }

    pub fn with_manifest(current_version: impl Into<String>, manifest_path: PathBuf) -> Self {
        Self {
            current_version: current_version.into(),
            manifest_path: Some(manifest_path),
        }
    }
}

impl UpdateService for LocalManifestUpdateService {
    fn kind(&self) -> UpdateServiceKind {
        UpdateServiceKind::LocalManifest
    }

    fn description(&self) -> String {
        match &self.manifest_path {
            Some(path) => format!("local manifest at {}", path.display()),
            None => "local manifest (none configured)".to_string(),
        }
    }

    fn check(&self) -> UpdateCheckResult {
        let status = match &self.manifest_path {
            Some(path) => {
                let Ok(contents) = std::fs::read_to_string(path) else {
                    return UpdateCheckResult {
                        status: UpdateStatus::UpToDate {
                            version: self.current_version.clone(),
                        },
                        checked_at: SystemTime::now(),
                    };
                };
                match serde_json::from_str::<LocalManifest>(&contents) {
                    Ok(manifest) => {
                        match compare_versions(&self.current_version, &manifest.version) {
                            Ordering::Less => UpdateStatus::UpdateAvailable {
                                version: manifest.version,
                                notes: manifest.notes,
                            },
                            _ => UpdateStatus::UpToDate {
                                version: self.current_version.clone(),
                            },
                        }
                    }
                    Err(_) => UpdateStatus::UpToDate {
                        version: self.current_version.clone(),
                    },
                }
            }
            None => UpdateStatus::UpToDate {
                version: self.current_version.clone(),
            },
        };
        UpdateCheckResult {
            status,
            checked_at: SystemTime::now(),
        }
    }
}

/// Local manifest shape.
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct LocalManifest {
    pub version: String,
    #[serde(default)]
    pub notes: String,
}

/// Compare dotted versions numerically (`1.10` > `1.9`). A suffix on an
/// otherwise equal numeric segment is a prerelease and sorts before the
/// release segment (`0-dev` < `0`).
pub fn compare_versions(a: &str, b: &str) -> Ordering {
    let a_parts: Vec<&str> = a.split('.').collect();
    let b_parts: Vec<&str> = b.split('.').collect();
    for (a_part, b_part) in a_parts.iter().zip(b_parts.iter()) {
        let (a_num, a_suffix) = version_part(a_part);
        let (b_num, b_suffix) = version_part(b_part);
        let ordering =
            a_num
                .cmp(&b_num)
                .then_with(|| match (a_suffix.is_empty(), b_suffix.is_empty()) {
                    (true, false) => Ordering::Greater,
                    (false, true) => Ordering::Less,
                    _ => a_suffix.cmp(b_suffix),
                });
        if ordering != Ordering::Equal {
            return ordering;
        }
    }
    a_parts.len().cmp(&b_parts.len())
}

fn version_part(part: &str) -> (u64, &str) {
    let digits = part.bytes().take_while(u8::is_ascii_digit).count();
    let number = part[..digits].parse().unwrap_or(0);
    (number, &part[digits..])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn disabled_service_never_checks() {
        let service = DisabledUpdateService {
            reason: "release builds check locally only".to_string(),
        };
        assert_eq!(service.kind(), UpdateServiceKind::Disabled);
        let result = service.check();
        assert!(matches!(result.status, UpdateStatus::Disabled { .. }));
    }

    #[test]
    fn local_manifest_reports_available_update() {
        let dir = std::env::temp_dir().join(format!("mr-crabs-updates-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("tmp dir");
        let path = dir.join("manifest.json");
        std::fs::write(&path, r#"{"version": "1.2.3", "notes": "local test"}"#).expect("write");
        let service = LocalManifestUpdateService::with_manifest("1.0.0", path.clone());
        assert_eq!(service.kind(), UpdateServiceKind::LocalManifest);
        let result = service.check();
        match result.status {
            UpdateStatus::UpdateAvailable { version, notes } => {
                assert_eq!(version, "1.2.3");
                assert_eq!(notes, "local test");
            }
            other => panic!("expected update, got {other:?}"),
        }
        // The manifest file is a local file; nothing else is touched.
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn local_manifest_reports_up_to_date_when_equal_or_newer() {
        let dir = std::env::temp_dir().join(format!("mr-crabs-updates-eq-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("tmp dir");
        let path = dir.join("manifest.json");
        std::fs::write(&path, r#"{"version": "1.0.0"}"#).expect("write");
        let service = LocalManifestUpdateService::with_manifest("1.0.0", path.clone());
        assert!(matches!(
            service.check().status,
            UpdateStatus::UpToDate { .. }
        ));
        std::fs::write(&path, r#"{"version": "0.9.0"}"#).expect("write");
        let service = LocalManifestUpdateService::with_manifest("1.0.0", path.clone());
        assert!(matches!(
            service.check().status,
            UpdateStatus::UpToDate { .. }
        ));
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn missing_manifest_is_up_to_date_not_an_error() {
        let service = LocalManifestUpdateService::with_manifest(
            "1.0.0",
            PathBuf::from("/nonexistent/mr-crabs-manifest.json"),
        );
        assert!(matches!(
            service.check().status,
            UpdateStatus::UpToDate { .. }
        ));
        let service = LocalManifestUpdateService::new("1.0.0");
        assert!(matches!(
            service.check().status,
            UpdateStatus::UpToDate { .. }
        ));
    }

    #[test]
    fn version_comparison_is_numeric_per_segment() {
        assert_eq!(compare_versions("1.10.0", "1.9.0"), Ordering::Greater);
        assert_eq!(compare_versions("1.0.0", "1.0.0"), Ordering::Equal);
        assert_eq!(compare_versions("1.0", "1.0.0"), Ordering::Less);
        assert_eq!(compare_versions("0.1.0-dev", "0.1.0"), Ordering::Less);
        assert_eq!(compare_versions("2.0", "1.99"), Ordering::Greater);
    }
}

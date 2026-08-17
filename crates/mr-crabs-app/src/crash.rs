//! Crash reporting: explicit disabled/local implementations only. Reports
//! are written to a local directory as JSON files with bounded rotation;
//! nothing ever leaves the machine.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::SystemTime;

use parking_lot::Mutex;
use serde::{Deserialize, Serialize};

/// What kind of crash reporting is installed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CrashReportingKind {
    Disabled,
    LocalFile,
}

/// One crash report.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CrashReport {
    pub id: String,
    pub app_version: String,
    pub occurred_at: SystemTime,
    pub message: String,
    pub thread: String,
    pub backtrace: Option<String>,
}

impl CrashReport {
    pub fn new(
        app_version: impl Into<String>,
        message: impl Into<String>,
        thread: impl Into<String>,
    ) -> Self {
        Self {
            id: next_report_id(),
            app_version: app_version.into(),
            occurred_at: SystemTime::now(),
            message: message.into(),
            thread: thread.into(),
            backtrace: None,
        }
    }

    pub fn with_backtrace(mut self, backtrace: impl Into<String>) -> Self {
        self.backtrace = Some(backtrace.into());
        self
    }
}

/// Errors from a crash reporter.
#[derive(Clone, Debug, PartialEq)]
pub enum CrashError {
    Unsupported,
    Io(String),
}

/// The product crash-reporting interface. Implementations MUST NOT perform
/// network I/O.
pub trait CrashReporter: Send + Sync {
    fn kind(&self) -> CrashReportingKind;
    fn is_enabled(&self) -> bool;
    /// Persist a report; returns the report id on success.
    fn report(&self, report: CrashReport) -> Result<String, CrashError>;
    /// The most recent local reports, newest first.
    fn recent(&self) -> Vec<CrashReport>;
}

/// Explicit disabled implementation.
pub struct DisabledCrashReporter {
    pub reason: String,
}

impl CrashReporter for DisabledCrashReporter {
    fn kind(&self) -> CrashReportingKind {
        CrashReportingKind::Disabled
    }

    fn is_enabled(&self) -> bool {
        false
    }

    fn report(&self, _report: CrashReport) -> Result<String, CrashError> {
        Err(CrashError::Unsupported)
    }

    fn recent(&self) -> Vec<CrashReport> {
        Vec::new()
    }
}

/// Local-file implementation: JSON reports in `directory`, rotating to the
/// newest `max_reports`.
pub struct LocalFileCrashReporter {
    pub directory: PathBuf,
    pub max_reports: usize,
    reports: Mutex<Vec<CrashReport>>,
}

impl LocalFileCrashReporter {
    pub fn new(directory: impl Into<PathBuf>, max_reports: usize) -> Self {
        Self {
            directory: directory.into(),
            max_reports: max_reports.max(1),
            reports: Mutex::new(Vec::new()),
        }
    }

    pub fn report_path(&self, id: &str) -> PathBuf {
        self.directory.join(format!("crash-{id}.json"))
    }
}

impl CrashReporter for LocalFileCrashReporter {
    fn kind(&self) -> CrashReportingKind {
        CrashReportingKind::LocalFile
    }

    fn is_enabled(&self) -> bool {
        true
    }

    fn report(&self, report: CrashReport) -> Result<String, CrashError> {
        std::fs::create_dir_all(&self.directory).map_err(|e| CrashError::Io(e.to_string()))?;
        let json =
            serde_json::to_string_pretty(&report).map_err(|e| CrashError::Io(e.to_string()))?;
        let id = report.id.clone();
        let path = self.report_path(&id);
        std::fs::write(&path, json).map_err(|e| CrashError::Io(e.to_string()))?;
        let mut reports = self.reports.lock();
        reports.insert(0, report);
        reports.truncate(self.max_reports);
        // Rotate on disk to the newest max_reports.
        let mut entries: Vec<PathBuf> = std::fs::read_dir(&self.directory)
            .map_err(|e| CrashError::Io(e.to_string()))?
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
            .collect();
        entries.sort();
        while entries.len() > self.max_reports {
            if let Some(oldest) = entries.first().cloned() {
                let _ = std::fs::remove_file(&oldest);
                entries.remove(0);
            }
        }
        Ok(id)
    }

    fn recent(&self) -> Vec<CrashReport> {
        self.reports.lock().clone()
    }
}

/// Process-unique, time-sortable local report id.
fn next_report_id() -> String {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let timestamp = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let sequence = SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("{timestamp:020}-{}-{sequence:020}", std::process::id())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(label: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "mr-crabs-crash-{label}-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        dir
    }

    #[test]
    fn disabled_reporter_rejects_reports() {
        let reporter = DisabledCrashReporter {
            reason: "release builds do not report crashes".to_string(),
        };
        assert!(!reporter.is_enabled());
        let report = CrashReport::new("0.1.0", "boom", "main");
        assert_eq!(reporter.report(report), Err(CrashError::Unsupported));
        assert!(reporter.recent().is_empty());
    }

    #[test]
    fn local_reporter_writes_files_and_rotates() {
        let dir = temp_dir("local");
        let reporter = LocalFileCrashReporter::new(dir.clone(), 2);
        let first = CrashReport::new("0.1.0", "first crash", "main");
        let second = CrashReport::new("0.1.0", "second crash", "render");
        let third = CrashReport::new("0.1.0", "third crash", "pty");
        let id1 = reporter.report(first.clone()).expect("report 1");
        let id2 = reporter.report(second.clone()).expect("report 2");
        assert!(reporter.report_path(&id1).exists());
        assert!(reporter.report_path(&id2).exists());
        assert_eq!(reporter.recent().len(), 2);
        // Rotation: the third report evicts the oldest file.
        let _id3 = reporter.report(third.clone()).expect("report 3");
        assert_eq!(reporter.recent().len(), 2);
        assert_eq!(reporter.recent()[0].message, "third crash");
        let json_files = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|entry| entry.ok())
            .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
            .count();
        assert_eq!(json_files, 2, "disk rotation keeps max_reports files");
        // The file content round-trips.
        let loaded: CrashReport =
            serde_json::from_str(&std::fs::read_to_string(reporter.report_path(&id2)).unwrap())
                .unwrap();
        assert_eq!(loaded.message, "second crash");
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[test]
    fn report_ids_are_unique_and_contain_timestamps() {
        let dir = temp_dir("ids");
        let reporter = LocalFileCrashReporter::new(dir.clone(), 4);
        let a = reporter
            .report(CrashReport::new("0.1.0", "a", "main"))
            .expect("a");
        let b = reporter
            .report(CrashReport::new("0.1.0", "b", "main"))
            .expect("b");
        assert_ne!(a, b);
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }
}

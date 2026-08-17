//! Snapshot restore and deterministic replay (S8: `snapshot-restore`,
//! `replay-restore`).
//!
//! [`TerminalSnapshot`] captures the full observable terminal state —
//! grid size, cursor, modes, compact history (with per-line widths), the
//! visible grid, style table, and combining marks — through the engine's
//! public snapshot/read APIs, and restores it into a freshly constructed
//! terminal through the engine's own restore hooks (`restore_modes`,
//! `restore_visible_grid`, `restore_cursor`, `push_history_line`). No second
//! terminal model is created.
//!
//! [`ReplayLog`] records deterministic events (`Feed` byte slices and
//! `Resize` grids) with explicit byte and event caps; [`ReplayLog::verify`]
//! restores the captured start state, replays the events, and compares the
//! resulting snapshot against a fresh capture — deterministic equality is
//! the acceptance contract.
//!
//! Alternate-screen note: snapshots capture the *active* grid. When
//! `AltScreen` was active, the restored terminal re-enters the alternate
//! screen and restores its grid; the primary screen content is not part of a
//! single-grid snapshot (the engine's `NormalizedSnapshot` contract). The
//! engine additionally discards alternate-screen scrollback on `?1049l`
//! (`history-alt-screen-transitions`), which replay preserves.

use mr_crabs_terminal::{
    Cell, CombiningMarks, CursorSnapshot, GridSize, Style, Terminal, TerminalError, TerminalMode,
};

/// Snapshot format version; `decode` rejects mismatches.
pub const SNAPSHOT_VERSION: u32 = 1;
/// Default cap on encoded snapshot size.
pub const DEFAULT_MAX_SNAPSHOT_BYTES: usize = 256 * 1024 * 1024;
/// Default cap on captured/restored history lines.
pub const DEFAULT_MAX_HISTORY_LINES: usize = 1_000_000;
/// Default cap on replay log encoded size.
pub const DEFAULT_MAX_REPLAY_BYTES: usize = 64 * 1024 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReplayError {
    Terminal(TerminalError),
    /// Encoded payload exceeds the configured cap.
    TooLarge,
    /// History line count exceeds the configured cap.
    TooManyLines,
    /// The payload carries a different snapshot version.
    VersionMismatch(u32),
    /// Structurally invalid payload.
    Corrupt,
}

impl std::fmt::Display for ReplayError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Terminal(err) => write!(f, "terminal restore failed: {err}"),
            Self::TooLarge => write!(f, "snapshot payload exceeds the size cap"),
            Self::TooManyLines => write!(f, "snapshot history exceeds the line cap"),
            Self::VersionMismatch(version) => {
                write!(f, "snapshot version {version} is not supported")
            }
            Self::Corrupt => write!(f, "snapshot payload is corrupt"),
        }
    }
}

impl std::error::Error for ReplayError {}

impl From<TerminalError> for ReplayError {
    fn from(err: TerminalError) -> Self {
        Self::Terminal(err)
    }
}

/// A full observable terminal state (single active grid).
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TerminalSnapshot {
    pub version: u32,
    pub size: GridSize,
    pub cursor: CursorSnapshot,
    pub modes: Vec<TerminalMode>,
    /// Per-line widths (history lines keep their original widths across
    /// resizes).
    pub history_cols: Vec<u16>,
    pub history: Vec<Vec<Cell>>,
    /// Visible grid, row-major (`cols * rows` cells).
    pub visible: Vec<Cell>,
    /// Style table referenced by `visible` cells.
    pub styles: Vec<Style>,
    pub combining_marks: Vec<CombiningMarks>,
    #[serde(default)]
    pub hyperlinks: Vec<mr_crabs_terminal::SnapshotHyperlink>,
}

impl TerminalSnapshot {
    /// Capture the current terminal state. History is read line by line
    /// through the bounded read cache; `max_history_lines` bounds capture.
    pub fn capture(term: &mut Terminal, max_history_lines: usize) -> Result<Self, ReplayError> {
        let snap = term.snapshot();
        let history_len = term.history_len();
        if history_len > max_history_lines {
            return Err(ReplayError::TooManyLines);
        }
        let mut history_cols = Vec::with_capacity(history_len);
        let mut history = Vec::with_capacity(history_len);
        for index in 0..history_len {
            let cols = term.history_line_cols(index).ok_or(ReplayError::Corrupt)?;
            history_cols.push(u16::try_from(cols).map_err(|_| ReplayError::Corrupt)?);
            let mut cells = Vec::new();
            if !term.read_history_line(index, &mut cells) {
                return Err(ReplayError::Corrupt);
            }
            history.push(cells);
        }
        Ok(Self {
            version: SNAPSHOT_VERSION,
            size: snap.size,
            cursor: snap.cursor,
            modes: snap.modes,
            history_cols,
            history,
            visible: snap.cells,
            styles: snap.styles,
            combining_marks: snap.combining_marks,
            hyperlinks: snap.hyperlinks,
        })
    }

    /// Restore this snapshot into a terminal (typically freshly
    /// constructed). Order: clear history, push history lines (old widths
    /// preserved), restore modes (alternate screen before the visible grid
    /// so the captured grid lands in the active screen), restore the visible
    /// grid and combining marks, then the cursor.
    pub fn restore(&self, term: &mut Terminal) -> Result<(), ReplayError> {
        if self.version != SNAPSHOT_VERSION {
            return Err(ReplayError::VersionMismatch(self.version));
        }
        if self.size != term.size() {
            return Err(ReplayError::Terminal(TerminalError::RestoreSizeMismatch));
        }
        if self.history.len() != self.history_cols.len() {
            return Err(ReplayError::Corrupt);
        }
        term.clear_history();
        for (cells, cols) in self.history.iter().zip(&self.history_cols) {
            if cells.len() > usize::from(*cols) {
                return Err(ReplayError::Corrupt);
            }
            term.push_history_line(*cols, cells);
        }
        term.restore_modes(&self.modes);
        term.restore_visible_grid(
            &self.visible,
            &self.styles,
            &self.combining_marks,
            &self.hyperlinks,
        )?;
        term.restore_cursor(self.cursor)?;
        Ok(())
    }

    /// Encode as versioned JSON with an explicit size cap.
    pub fn encode(&self, max_bytes: usize) -> Result<Vec<u8>, ReplayError> {
        if self.version != SNAPSHOT_VERSION {
            return Err(ReplayError::VersionMismatch(self.version));
        }
        let encoded = serde_json::to_vec(self).map_err(|_| ReplayError::Corrupt)?;
        if encoded.len() > max_bytes {
            return Err(ReplayError::TooLarge);
        }
        Ok(encoded)
    }

    /// Decode and validate a snapshot payload (size cap, version, structure).
    pub fn decode(
        bytes: &[u8],
        max_bytes: usize,
        max_history_lines: usize,
    ) -> Result<Self, ReplayError> {
        if bytes.len() > max_bytes {
            return Err(ReplayError::TooLarge);
        }
        let snapshot: Self = serde_json::from_slice(bytes).map_err(|_| ReplayError::Corrupt)?;
        if snapshot.version != SNAPSHOT_VERSION {
            return Err(ReplayError::VersionMismatch(snapshot.version));
        }
        if !snapshot.size.is_valid() {
            return Err(ReplayError::Corrupt);
        }
        if snapshot.history.len() > max_history_lines
            || snapshot.history.len() != snapshot.history_cols.len()
        {
            return Err(if snapshot.history.len() > max_history_lines {
                ReplayError::TooManyLines
            } else {
                ReplayError::Corrupt
            });
        }
        for (cells, cols) in snapshot.history.iter().zip(&snapshot.history_cols) {
            if cells.len() > usize::from(*cols) {
                return Err(ReplayError::Corrupt);
            }
        }
        let expected = usize::from(snapshot.size.cols) * usize::from(snapshot.size.rows);
        if snapshot.visible.len() != expected {
            return Err(ReplayError::Corrupt);
        }
        for cell in &snapshot.visible {
            if usize::from(cell.style) >= snapshot.styles.len() {
                return Err(ReplayError::Corrupt);
            }
        }
        for marks in &snapshot.combining_marks {
            if marks.cell_index as usize >= expected {
                return Err(ReplayError::Corrupt);
            }
        }
        Ok(snapshot)
    }
}

/// One deterministic replay event.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ReplayEvent {
    /// Feed terminal bytes (bounded by the log's byte cap).
    Feed(Vec<u8>),
    /// Resize the grid (reflow included).
    Resize(GridSize),
}

/// A bounded, deterministic replay log: a start snapshot plus ordered
/// events. `apply` restores the snapshot into a fresh terminal and replays
/// the events; `verify` asserts snapshot equality after replay.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ReplayLog {
    pub start: TerminalSnapshot,
    pub events: Vec<ReplayEvent>,
}

impl ReplayLog {
    pub fn new(start: TerminalSnapshot) -> Self {
        Self {
            start,
            events: Vec::new(),
        }
    }

    /// Record a feed event; fails when the log's encoded size cap would be
    /// exceeded (the log stays unchanged).
    pub fn record_feed(&mut self, bytes: &[u8], max_bytes: usize) -> Result<(), ReplayError> {
        if bytes.is_empty() {
            return Ok(());
        }
        self.events.push(ReplayEvent::Feed(bytes.to_vec()));
        if self.encoded_size_estimate() > max_bytes {
            self.events.pop();
            return Err(ReplayError::TooLarge);
        }
        Ok(())
    }

    pub fn record_resize(&mut self, size: GridSize, max_bytes: usize) -> Result<(), ReplayError> {
        if !size.is_valid() {
            return Err(ReplayError::Corrupt);
        }
        self.events.push(ReplayEvent::Resize(size));
        if self.encoded_size_estimate() > max_bytes {
            self.events.pop();
            return Err(ReplayError::TooLarge);
        }
        Ok(())
    }

    /// Apply the log: restore `start` into `term`, then replay every event
    /// in order (resizes before the following feeds).
    pub fn apply(&self, term: &mut Terminal) -> Result<(), ReplayError> {
        self.start.restore(term)?;
        for event in &self.events {
            match event {
                ReplayEvent::Feed(bytes) => term.feed(bytes),
                ReplayEvent::Resize(size) => term.resize(*size)?,
            }
        }
        Ok(())
    }

    /// Restore, replay, and verify that a fresh capture equals the log's
    /// end state (`true` on equality; `Err` on restore/replay failure).
    pub fn verify(
        &self,
        term: &mut Terminal,
        max_history_lines: usize,
    ) -> Result<bool, ReplayError> {
        self.apply(term)?;
        let recaptured = TerminalSnapshot::capture(term, max_history_lines)?;
        Ok(recaptured == self.expected_end(max_history_lines)?)
    }

    /// Capture the end state without mutating the caller's terminal.
    pub fn expected_end(&self, max_history_lines: usize) -> Result<TerminalSnapshot, ReplayError> {
        let mut scratch = Terminal::new(self.start.size).map_err(ReplayError::Terminal)?;
        self.apply(&mut scratch)?;
        TerminalSnapshot::capture(&mut scratch, max_history_lines)
    }

    fn encoded_size_estimate(&self) -> usize {
        // Replay recording is not a render-loop path. Measure the actual
        // serialized representation so added snapshot fields and JSON byte
        // expansion cannot silently bypass the configured cap.
        serde_json::to_vec(self)
            .map(|encoded| encoded.len())
            .unwrap_or(usize::MAX)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mr_crabs_terminal::ScrollbackConfig;

    fn build_term() -> Terminal {
        let mut term = Terminal::new_with_config(
            GridSize::new(10, 4),
            ScrollbackConfig {
                max_lines: 1000,
                hot_page_lines: 2,
                max_queued_jobs: 4,
                max_pending_completions: 4,
            },
        )
        .unwrap();
        term.feed(b"\x1b[31mred prompt\x1b[0m\r\n");
        term.feed(b"ls -la\r\n");
        term.feed(b"drwxr-xr-x  user  staff\r\n");
        term.feed(b"\x1b]8;id=docs;https://example.com\x07link\x1b]8;;\x07\r\n");
        term.feed(b"done\r\n");
        term.feed(b"\x1b]8;id=visible;https://example.org\x07docs\x1b]8;;\x07");
        term
    }

    #[test]
    fn capture_restore_is_deterministic_and_equal() {
        let mut term = build_term();
        let original = term.snapshot();
        let snapshot = TerminalSnapshot::capture(&mut term, 1000).expect("capture");
        assert!(!snapshot.history.is_empty());
        assert_eq!(snapshot.visible.len(), 10 * 4);
        assert!(!snapshot.hyperlinks.is_empty());

        // Restore into a fresh terminal and compare full snapshots.
        let mut restored = Terminal::new(snapshot.size).expect("fresh terminal");
        snapshot.restore(&mut restored).expect("restore");
        let after = restored.snapshot();
        assert_eq!(after, original, "visible grid, cursor, modes, styles equal");
        assert_eq!(restored.history_len(), term.history_len());
        let mut a = Vec::new();
        let mut b = Vec::new();
        for i in 0..term.history_len() {
            assert!(term.read_history_line(i, &mut a));
            assert!(restored.read_history_line(i, &mut b));
            assert_eq!(a, b, "history line {i} byte-identical");
        }
    }

    #[test]
    fn snapshot_encode_decode_roundtrip_and_validation() {
        let mut term = build_term();
        let snapshot = TerminalSnapshot::capture(&mut term, 1000).expect("capture");
        let encoded = snapshot.encode(DEFAULT_MAX_SNAPSHOT_BYTES).expect("encode");
        let decoded =
            TerminalSnapshot::decode(&encoded, DEFAULT_MAX_SNAPSHOT_BYTES, 1000).expect("decode");
        assert_eq!(decoded, snapshot);

        // Version mismatch: rewrite `"version":1` to `"version":99`.
        let mut wrong = encoded.clone();
        let needle = b"\"version\":1";
        let pos = wrong
            .windows(needle.len())
            .position(|w| w == needle)
            .expect("version field");
        wrong.splice(pos..pos + needle.len(), b"\"version\":99".iter().copied());
        assert_eq!(
            TerminalSnapshot::decode(&wrong, DEFAULT_MAX_SNAPSHOT_BYTES, 1000),
            Err(ReplayError::VersionMismatch(99))
        );

        // Corrupt JSON.
        let mut corrupt = encoded.clone();
        let last = corrupt.len() - 1;
        corrupt[last] = b'{';
        assert_eq!(
            TerminalSnapshot::decode(&corrupt, DEFAULT_MAX_SNAPSHOT_BYTES, 1000),
            Err(ReplayError::Corrupt)
        );

        // Oversized.
        assert_eq!(
            TerminalSnapshot::decode(&encoded, 16, 1000),
            Err(ReplayError::TooLarge)
        );
    }

    #[test]
    fn replay_apply_matches_expected_end() {
        let mut term = build_term();
        let snapshot = TerminalSnapshot::capture(&mut term, 1000).expect("capture");
        let mut log = ReplayLog::new(snapshot);
        log.record_feed(b"tail -f /var/log/system.log\r\n", DEFAULT_MAX_REPLAY_BYTES)
            .expect("feed recorded");
        log.record_feed(b"line 1\r\nline 2\r\n", DEFAULT_MAX_REPLAY_BYTES)
            .expect("feed recorded");
        log.record_resize(GridSize::new(12, 5), DEFAULT_MAX_REPLAY_BYTES)
            .expect("resize recorded");
        log.record_feed(b"wide content here\r\n", DEFAULT_MAX_REPLAY_BYTES)
            .expect("feed recorded");

        // Verify determinism: apply on a fresh terminal equals the log's
        // own expected end state.
        let mut fresh = Terminal::new(log.start.size).expect("fresh");
        assert!(log.verify(&mut fresh, 1000).expect("verify"));
        let mut fresh2 = Terminal::new(log.start.size).expect("fresh");
        log.apply(&mut fresh2).expect("apply");
        let recaptured = TerminalSnapshot::capture(&mut fresh2, 1000).expect("recapture");
        assert_eq!(recaptured, log.expected_end(1000).expect("expected"));
    }

    #[test]
    fn replay_log_is_bounded() {
        let mut term = build_term();
        let snapshot = TerminalSnapshot::capture(&mut term, 1000).expect("capture");
        // The log embeds the full start snapshot, so the cap must leave
        // room for it: base the cap on the snapshot's actual encoded size.
        let base = snapshot
            .encode(DEFAULT_MAX_SNAPSHOT_BYTES)
            .expect("encode")
            .len();
        let mut log = ReplayLog::new(snapshot);
        let small = base + 64;
        // A 2 KiB feed never fits under a cap only 64 bytes above the base.
        assert!(log.record_feed(&vec![b'x'; 2048], small).is_err());
        assert!(
            log.events.is_empty(),
            "failed record must not mutate the log"
        );
        // Resize events still fit.
        log.record_resize(GridSize::new(10, 4), small)
            .expect("resize fits");
        assert_eq!(log.events.len(), 1);
    }

    #[test]
    fn alternate_screen_history_is_discarded_and_restorable() {
        let mut term = Terminal::new(GridSize::new(10, 3)).unwrap();
        term.feed(b"primary line one\r\nprimary line two\r\n");
        // Enter alt, scroll within it, then exit: alt scrollback is dropped.
        term.feed(b"\x1b[?1049h");
        let before_exit = term.history_len();
        for i in 0..5 {
            term.feed(format!("alt content {i}\r\n").as_bytes());
        }
        // The alternate screen has its own page list (Ghostty per-screen
        // PageLists); rows scrolled on it never enter the shared primary
        // history, so the observable history length is unchanged while the
        // alternate screen is active, not grown.
        assert_eq!(term.history_len(), before_exit);
        term.feed(b"\x1b[?1049l");
        assert_eq!(
            term.history_len(),
            before_exit,
            "alt-screen scrollback is discarded on exit"
        );

        // The primary state round-trips through snapshot restore.
        let snapshot = TerminalSnapshot::capture(&mut term, 1000).expect("capture");
        assert!(!snapshot.modes.contains(&TerminalMode::AltScreen));
        let mut restored = Terminal::new(snapshot.size).expect("fresh");
        snapshot.restore(&mut restored).expect("restore");
        assert_eq!(restored.snapshot(), term.snapshot());
    }
}

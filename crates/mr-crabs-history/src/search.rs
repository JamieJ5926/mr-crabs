//! Cancellable, generation-tokened background search over history and the
//! visible grid (S8: `search-worker`, `search-cancellation`).
//!
//! Semantics mirror the Ghostty search thread
//! (`src/terminal/search/Thread.zig`, `search/sliding_window.zig`,
//! `search/viewport.zig` in the oracle at d2c70a8c):
//!
//! - A dedicated worker thread owns the search loop; the terminal is locked
//!   only for the duration of one line read (`Thread.zig` copies data under
//!   the lock to minimize contention).
//! - The needle is matched **case-insensitively** (`std.ascii.indexOfIgnoreCase`,
//!   ASCII folding only) as a substring of the plain-text stream.
//! - Soft-wrapped rows join into one continuous line; every non-wrapped row
//!   emits a trailing `\n`; wide-character spacers and trimmed trailing
//!   blanks never appear in the stream (formatter.zig plain+unwrap rules,
//!   lines 1180-1343; sliding_window.zig append rules).
//! - Forward search scans the stream in order; reverse search scans the
//!   reversed stream with the reversed needle (`sliding_window.zig:111-143`).
//! - The needle length is capped at [`MAX_NEEDLE_BYTES`] (255), matching the
//!   Ghostty search mailbox write-request cap (`Thread.zig:470-477`).
//!
//! Cancellation and staleness:
//! - [`SearchWorker::start`] bumps the generation token, invalidating any
//!   in-flight search (its outcome arrives with a stale token and must be
//!   discarded), and replaces the pending request (a single bounded slot).
//! - The worker checks for a newer pending request and the cancel flag
//!   between every line; [`SearchWorker::cancel`] aborts the current search
//!   with `cancelled: true`.
//! - [`SearchWorker::note_history_changed`] bumps the token when the owner
//!   mutates history (feed/resize/alt-screen), so results produced before
//!   the mutation report stale.
//!
//! Bounds: one pending request slot, one result slot, a search window of at
//! most `needle.len() - 1 + max_line_bytes`, at most `limit` matches
//! (default 1000, hard cap [`MAX_SEARCH_LIMIT`]), and at most `needle.len()`
//! span entries per match.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use mr_crabs_terminal::{Cell, HistoryRead};

/// Hard cap on the needle length in bytes (Ghostty mailbox `WriteReq` cap).
pub const MAX_NEEDLE_BYTES: usize = 255;
/// Default maximum number of matches per search.
pub const DEFAULT_SEARCH_LIMIT: usize = 1000;
/// Hard cap on matches per search (result-count bound).
pub const MAX_SEARCH_LIMIT: usize = 100_000;

/// Alacritty `Flags::WRAPLINE` bit, preserved in compact cells.
const WRAPLINE_BIT: u16 = 0x0010;

/// Row-wrap state of a compact cell row: the last cell carries WRAPLINE
/// (alacritty sets it on `Column(cols - 1)`, term/mod.rs:2485).
pub(crate) fn row_wrapped(cells: &[Cell]) -> bool {
    cells
        .last()
        .is_some_and(|cell| cell.flags & WRAPLINE_BIT != 0)
}

/// Encode one row into the search stream and record the byte range and row
/// column of every text-emitting cell (spacers and trimmed trailing blanks
/// are excluded).
///
/// Rules (formatter.zig plain+unwrap, `trim=true`):
/// - wide-character spacer cells are skipped;
/// - empty and space cells accumulate as pending blanks and are only emitted
///   as spaces when a non-blank cell follows (trailing blanks are trimmed);
/// - when `continuation` is true (the previous row was soft-wrapped) the
///   pending-blank accumulator carries over, preserving mid-line blanks of
///   wrapped rows.
///
/// Returns `(text, starts, ends, cols, widths)` with `starts`/`ends`
/// parallel per-cell byte offsets in forward text order and `cols`/`widths`
/// the row column and cell width (1 or 2 for wide cells) of each
/// text-emitting cell. Columns account for collapsed blanks and skipped
/// spacers so match spans map back onto real row coordinates.
type EncodedRowMap = (Vec<u8>, Vec<u32>, Vec<u32>, Vec<u16>, Vec<u16>);

fn encode_row_with_map(cells: &[Cell], continuation: bool, blank: &mut usize) -> EncodedRowMap {
    let mut out = Vec::with_capacity(cells.len() * 4 + 1);
    let mut starts = Vec::new();
    let mut ends = Vec::new();
    let mut cols: Vec<u16> = Vec::new();
    let mut widths: Vec<u16> = Vec::new();
    if !continuation {
        *blank = 0;
    }
    let mut col: usize = 0;
    for cell in cells {
        if cell.flags & Cell::WIDE_SPACER != 0 {
            // The wide cell already accounted for both columns.
            continue;
        }
        let content = cell.content;
        if content == 0 || content == u32::from(' ') {
            *blank += 1;
            col += 1;
            continue;
        }
        if *blank > 0 {
            out.extend(std::iter::repeat_n(b' ', *blank));
            *blank = 0;
        }
        if let Some(ch) = char::from_u32(content) {
            let mut buf = [0u8; 4];
            let encoded = ch.encode_utf8(&mut buf);
            let start = out.len();
            out.extend_from_slice(encoded.as_bytes());
            starts.push(u32::try_from(start).expect("row text fits u32"));
            ends.push(u32::try_from(out.len()).expect("row text fits u32"));
            let width: u16 = if cell.flags & Cell::WIDE != 0 { 2 } else { 1 };
            cols.push(u16::try_from(col).expect("row fits u16"));
            widths.push(width);
            col += usize::from(width);
        }
    }
    (out, starts, ends, cols, widths)
}

/// Encode a single row with a fresh accumulator (the search/selection text
/// contract; also used by corpus and viewport comparisons).
pub fn row_text(cells: &[Cell]) -> Vec<u8> {
    let mut blank = 0usize;
    let (text, _, _, _, _) = encode_row_with_map(cells, false, &mut blank);
    text
}

/// Search direction over the line space.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SearchDirection {
    /// Increasing line index (oldest toward newest).
    Forward,
    /// Decreasing line index (newest toward oldest).
    Reverse,
}

/// Where a search starts.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SearchStart {
    /// First line of the line space.
    Top,
    /// Last line of the line space.
    Bottom,
    /// An absolute line index (clamped into range).
    Line(usize),
}

/// One contiguous span of a match on a single line. `end_col` is exclusive.
#[derive(Clone, Copy, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SearchSpan {
    pub line: usize,
    pub start_col: u16,
    pub end_col: u16,
}

/// A match of the needle, possibly spanning soft-wrapped rows. Spans are
/// always in ascending line order.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SearchMatch {
    pub spans: Vec<SearchSpan>,
    pub start_line: usize,
    pub start_col: u16,
}

/// A search request. `visible_rows` is the visible-grid copy captured by the
/// caller when the search starts (the worker never reads the live grid).
#[derive(Clone, Debug)]
pub struct SearchRequest {
    /// UTF-8 needle; empty means "stop the search" (Ghostty inactive search).
    pub needle: Vec<u8>,
    pub direction: SearchDirection,
    pub start: SearchStart,
    /// Maximum matches returned; clamped into `[1, MAX_SEARCH_LIMIT]`.
    pub limit: usize,
    /// Case-sensitive matching (default false, matching Ghostty's
    /// case-insensitive search).
    pub case_sensitive: bool,
    pub visible_rows: Vec<Vec<Cell>>,
}

impl Default for SearchRequest {
    fn default() -> Self {
        Self {
            needle: Vec::new(),
            direction: SearchDirection::Forward,
            start: SearchStart::Top,
            limit: DEFAULT_SEARCH_LIMIT,
            case_sensitive: false,
            visible_rows: Vec::new(),
        }
    }
}

/// The outcome of one search.
#[derive(Clone, Debug, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SearchOutcome {
    /// Generation token captured when the search started. Compare with the
    /// worker's current generation: a mismatch means the outcome is stale.
    pub token: u64,
    pub matches: Vec<SearchMatch>,
    /// True when more matches exist beyond `limit`.
    pub truncated: bool,
    /// True when the whole search range was scanned.
    pub completed: bool,
    /// True when the search was aborted by [`SearchWorker::cancel`] or a
    /// replacement request.
    pub cancelled: bool,
    /// Number of lines scanned.
    pub lines_searched: usize,
}

impl SearchOutcome {
    /// An outcome is stale when history changed after the search started.
    pub fn is_stale(&self, current_generation: u64) -> bool {
        self.token != current_generation
    }
}

// ---------------------------------------------------------------------------
// Core search (shared by the synchronous API and the worker thread).
// ---------------------------------------------------------------------------

/// One line's contribution to the search window.
struct LineEntry {
    line: usize,
    cols: u16,
    /// Byte length of the whole entry within the window.
    byte_len: usize,
    /// Byte length of the separator ('\n' for non-wrapped rows) at the
    /// beginning (reverse) or end (forward) of the entry.
    sep_len: usize,
    /// Byte length of the row text within the entry.
    text_len: usize,
    /// Byte offset of each text-emitting cell's first byte within the row
    /// text (ascending, forward order).
    cell_starts: Vec<u32>,
    /// One past the last byte of each text-emitting cell (parallel).
    cell_ends: Vec<u32>,
    /// Row column of each text-emitting cell (parallel). Columns account
    /// for collapsed blanks and skipped wide spacers, so a byte offset
    /// maps back onto the real row coordinate.
    cell_cols: Vec<u16>,
    /// Width (1, or 2 for wide cells) of each text-emitting cell.
    cell_widths: Vec<u16>,
    /// True when the entry's text bytes are stored reversed (reverse
    /// searches append `sep + reversed(row text)`, mirroring Ghostty's
    /// sliding window, which reverses the page encoding for `.reverse`).
    reversed: bool,
}

impl LineEntry {
    /// Map an entry-local byte position onto the forward text order: text
    /// bytes occupy `[0, text_len)`; the separator (which leads in reversed
    /// entries) occupies `[text_len, byte_len)`.
    fn forward_pos(&self, local: usize) -> usize {
        if local < self.sep_len {
            self.text_len + local
        } else {
            self.text_len - 1 - (local - self.sep_len)
        }
    }
}

/// Run one complete search over history plus the captured visible rows.
pub(crate) fn search_core<R: HistoryRead + ?Sized>(
    reader: &mut R,
    req: &SearchRequest,
    token: u64,
    cancel: &AtomicBool,
    abort: &dyn Fn() -> bool,
) -> SearchOutcome {
    search_core_range(reader, req, token, cancel, abort, None, usize::MAX).0
}

/// Run a bounded forward-search slice. Slices end only after a non-wrapped
/// row, so continuation state never leaks across calls. `next_line` is the
/// first line for the next slice; it equals the line-space length on finish.
pub fn search_slice<R: HistoryRead + ?Sized>(
    reader: &mut R,
    req: &SearchRequest,
    start_line: usize,
    budget: usize,
    token: u64,
    cancel: &AtomicBool,
) -> (SearchOutcome, usize) {
    if req.direction == SearchDirection::Reverse {
        let outcome = search_core(reader, req, token, cancel, &|| false);
        let next = reader.history_len() + req.visible_rows.len();
        return (outcome, next);
    }
    search_core_range(
        reader,
        req,
        token,
        cancel,
        &|| false,
        Some(start_line),
        budget.max(1),
    )
}

fn search_core_range<R: HistoryRead + ?Sized>(
    reader: &mut R,
    req: &SearchRequest,
    token: u64,
    cancel: &AtomicBool,
    abort: &dyn Fn() -> bool,
    start_override: Option<usize>,
    budget: usize,
) -> (SearchOutcome, usize) {
    let history_len = reader.history_len();
    let visible_len = req.visible_rows.len();
    let total = history_len + visible_len;
    let limit = req.limit.clamp(1, MAX_SEARCH_LIMIT);
    let needle = &req.needle[..req.needle.len().min(MAX_NEEDLE_BYTES)];
    let mut matches = Vec::new();
    let mut truncated = false;
    let mut lines_searched = 0usize;
    let mut cancelled = false;
    let mut budget_exhausted = false;

    if needle.is_empty() || total == 0 {
        return (
            SearchOutcome {
                token,
                matches,
                truncated: false,
                completed: true,
                cancelled: false,
                lines_searched: 0,
            },
            total,
        );
    }
    let needle_len = needle.len();

    let reverse = req.direction == SearchDirection::Reverse;
    let start_line = start_override.unwrap_or_else(|| match req.start {
        SearchStart::Top => 0,
        SearchStart::Bottom => total - 1,
        SearchStart::Line(line) => line.min(total - 1),
    });
    if start_line >= total {
        return (
            SearchOutcome {
                token,
                matches,
                truncated: false,
                completed: true,
                cancelled: false,
                lines_searched: 0,
            },
            total,
        );
    }

    // The search window: at most `needle_len - 1` bytes of overlap plus the
    // current entry (a leading entry is kept whole when it straddles the
    // prune boundary). `window_base` is the global byte offset of
    // `window[0]`; `search_from` is a window-relative offset.
    let mut window: Vec<u8> = Vec::with_capacity(needle_len + 4096);
    let mut entries: Vec<LineEntry> = Vec::new();
    let mut window_base: usize = 0;
    let mut search_from: usize = 0;
    let mut blank = 0usize;
    // Reversed needle for reverse searches (sliding_window.zig:111-143).
    let mut needle_rev: Vec<u8> = needle.to_vec();
    if reverse {
        needle_rev.reverse();
    }
    let search_needle: &[u8] = if reverse { &needle_rev } else { needle };

    let mut line = start_line;
    let mut previous_wrapped_for_next = false;

    while line < total && !cancelled && !abort() {
        if cancel.load(Ordering::Acquire) {
            cancelled = true;
            break;
        }
        let (cells, cols, wrapped) = if line < history_len {
            let mut cells = Vec::new();
            if !reader.read_history_line(line, &mut cells) {
                // Unreadable (corrupt) page: skip the line rather than
                // fabricate content, keeping the traversal bounded.
                lines_searched += 1;
                previous_wrapped_for_next = false;
                match reverse {
                    true => line = line.saturating_sub(1),
                    false => line += 1,
                }
                continue;
            }
            let cols = reader.history_line_cols(line).unwrap_or(0);
            let cols = u16::try_from(cols).unwrap_or(u16::MAX);
            let wrapped = row_wrapped(&cells);
            (cells, cols, wrapped)
        } else {
            let cells = &req.visible_rows[line - history_len];
            let cols = u16::try_from(cells.len()).unwrap_or(u16::MAX);
            (cells.clone(), cols, row_wrapped(cells))
        };

        let continuation = previous_wrapped_for_next;
        let (mut text, starts, ends, cell_cols, cell_widths) =
            encode_row_with_map(&cells, continuation, &mut blank);
        let sep_len = if !wrapped { 1 } else { 0 };
        let mut entry_bytes = Vec::with_capacity(text.len() + 1);
        let reversed = reverse;
        if reverse {
            // Reverse the row text (Ghostty sliding_window reverses the page
            // encoding for reverse searches) but keep the cell maps in
            // forward order; spans_for maps match ranges back through
            // `forward_pos` before resolving columns.
            text.reverse();
            if !wrapped {
                entry_bytes.push(b'\n');
            }
            entry_bytes.extend_from_slice(&text);
        } else {
            entry_bytes.extend_from_slice(&text);
            if !wrapped {
                entry_bytes.push(b'\n');
            }
        }
        let text_len = text.len();

        // Prune the window: keep at most `needle_len - 1` bytes from before
        // this entry, dropping only whole entries.
        let keep = needle_len.saturating_sub(1);
        if window.len() > keep {
            let mut drop_target = window.len() - keep;
            while drop_target > 0 {
                let Some(first) = entries.first() else {
                    break;
                };
                if first.byte_len > drop_target {
                    break;
                }
                window.drain(..first.byte_len);
                window_base += first.byte_len;
                search_from = search_from.saturating_sub(first.byte_len);
                drop_target -= first.byte_len;
                entries.remove(0);
            }
        }

        // Append this entry.
        let byte_len = entry_bytes.len();
        let entry = LineEntry {
            line,
            cols,
            byte_len,
            sep_len,
            text_len,
            cell_starts: starts,
            cell_ends: ends,
            cell_cols,
            cell_widths,
            reversed,
        };
        window.append(&mut entry_bytes);
        entries.push(entry);

        // Find matches starting at or after `search_from` within the window.
        loop {
            if cancel.load(Ordering::Acquire) {
                cancelled = true;
                break;
            }
            if abort() {
                break;
            }
            let Some(found) = find_needle(&window, search_needle, search_from, req.case_sensitive)
            else {
                break;
            };
            let end = found + needle_len;
            // `found`/`end` are window-relative; spans_for maps a *global*
            // byte range onto entries, so translate before mapping (the
            // window prunes whole entries, so window_base grows past 0).
            let mut spans = spans_for(
                &entries,
                window_base,
                window_base + found,
                window_base + end,
            );
            if spans.is_empty() {
                // No line coverage (defensive; cannot happen for a valid
                // window): advance past the match.
                search_from = end;
                continue;
            }
            if reverse {
                // Spans are produced in scan (newest-first) order; deliver
                // them in ascending line order like forward results.
                spans.reverse();
            }
            let start_line_of_match = spans[0].line;
            let start_col_of_match = spans[0].start_col;
            matches.push(SearchMatch {
                spans,
                start_line: start_line_of_match,
                start_col: start_col_of_match,
            });
            search_from = end;
            if matches.len() >= limit {
                truncated = true;
                break;
            }
        }
        lines_searched += 1;
        previous_wrapped_for_next = wrapped;
        if reverse {
            if line == 0 {
                break;
            }
            line -= 1;
        } else {
            line += 1;
        }
        if lines_searched >= budget && !wrapped {
            budget_exhausted = line < total;
            break;
        }
        if truncated || cancelled {
            break;
        }
    }

    (
        SearchOutcome {
            token,
            matches,
            truncated,
            completed: !cancelled && !truncated && !abort() && !budget_exhausted,
            cancelled,
            lines_searched,
        },
        line.min(total),
    )
}

/// Case-insensitive (ASCII-only, Ghostty `indexOfIgnoreCase`) or exact
/// substring search starting at `from`.
fn find_needle(hay: &[u8], needle: &[u8], from: usize, case_sensitive: bool) -> Option<usize> {
    if needle.is_empty() || hay.len() < needle.len() || from > hay.len() - needle.len() {
        return None;
    }
    if case_sensitive {
        hay[from..]
            .windows(needle.len())
            .position(|w| w == needle)
            .map(|pos| from + pos)
    } else {
        hay[from..]
            .windows(needle.len())
            .position(|w| eq_ignore_case_ascii(w, needle))
            .map(|pos| from + pos)
    }
}

fn eq_ignore_case_ascii(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).all(|(x, y)| x.eq_ignore_ascii_case(y))
}

/// Map a global byte range `[start, end)` onto per-line spans.
fn spans_for(
    entries: &[LineEntry],
    window_base: usize,
    start: usize,
    end: usize,
) -> Vec<SearchSpan> {
    if start >= end || entries.is_empty() {
        return Vec::new();
    }
    let mut spans = Vec::new();
    let mut global = window_base;
    let mut first_idx = None;
    let mut last_idx = None;
    for (idx, entry) in entries.iter().enumerate() {
        let entry_end = global + entry.byte_len;
        if first_idx.is_none() && start < entry_end {
            first_idx = Some(idx);
        }
        if end - 1 < entry_end {
            last_idx = Some(idx);
            break;
        }
        global = entry_end;
    }
    let (Some(first), Some(last)) = (first_idx, last_idx) else {
        return spans;
    };
    let mut global = window_base;
    for (idx, entry) in entries.iter().enumerate() {
        if idx > last {
            break;
        }
        if idx >= first {
            let local_start = start.saturating_sub(global).min(entry.byte_len);
            let local_end = end.saturating_sub(global).min(entry.byte_len);
            if entry.reversed {
                // The entry's text is stored byte-reversed (separator
                // leading). The local match range [local_start, local_end)
                // reads backwards in forward terms, so its forward extent is
                // [forward_pos(local_end - 1), forward_pos(local_start) + 1).
                let forward_start = entry.forward_pos(local_end.saturating_sub(1));
                let forward_end = entry.forward_pos(local_start) + 1;
                spans.push(SearchSpan {
                    line: entry.line,
                    start_col: col_of(entry, forward_start),
                    end_col: if idx == last {
                        col_end_of(entry, forward_end.saturating_sub(1))
                    } else {
                        entry.cols
                    },
                });
            } else {
                let start_col = col_of(entry, local_start);
                let end_col = if idx == last {
                    col_end_of(entry, local_end.saturating_sub(1))
                } else {
                    entry.cols
                };
                spans.push(SearchSpan {
                    line: entry.line,
                    start_col,
                    end_col,
                });
            }
        }
        global += entry.byte_len;
    }
    spans
}

/// Column of the cell containing byte `pos` of an entry. `pos` is a
/// forward-order text position: for reversed entries the caller maps the
/// local range through `forward_pos` first, and the separator only leads in
/// reversed entries, so no separator adjustment is needed here. Collapsed
/// blanks and skipped wide spacers are accounted for through `cell_cols`,
/// so the result is the real row column; a position inside a blank run
/// (each blank emits exactly one byte) maps to the run's column, and a
/// separator position maps to the row end.
fn col_of(entry: &LineEntry, pos: usize) -> u16 {
    if pos >= entry.text_len {
        return entry.cols;
    }
    let pos_u = u32::try_from(pos).unwrap_or(u32::MAX);
    let idx = entry.cell_starts.partition_point(|&s| s <= pos_u);
    if idx == 0 {
        // Leading blank run (possibly blanks carried over a wrap): each
        // blank is a single-width cell emitting exactly one byte, so the
        // byte offset is the run's column.
        return u16::try_from(pos).unwrap_or(u16::MAX);
    }
    let last = idx - 1;
    if pos_u < entry.cell_ends[last] {
        return entry.cell_cols[last];
    }
    // Blank run following cell `last`: one column per emitted byte.
    let run_byte = entry.cell_ends[last];
    let run_col = u32::from(entry.cell_cols[last]) + u32::from(entry.cell_widths[last]);
    u16::try_from(run_col + (pos_u - run_byte))
        .unwrap_or(u16::MAX)
        .min(entry.cols)
}

/// One past the column of the cell containing byte `pos` of an entry.
/// `pos` is a forward-order text position (see [`col_of`]); blank-run
/// positions map to one past the containing blank cell, wide cells advance
/// two columns, and separator positions map to the row end.
fn col_end_of(entry: &LineEntry, pos: usize) -> u16 {
    if pos >= entry.text_len {
        return entry.cols;
    }
    let pos_u = u32::try_from(pos).unwrap_or(u32::MAX);
    let idx = entry.cell_starts.partition_point(|&s| s <= pos_u);
    if idx == 0 {
        // Leading blank run: one single-width cell per byte.
        return u16::try_from(pos + 1).unwrap_or(u16::MAX).min(entry.cols);
    }
    let last = idx - 1;
    if pos_u < entry.cell_ends[last] {
        return entry.cell_cols[last]
            .saturating_add(entry.cell_widths[last])
            .min(entry.cols);
    }
    // Blank run following cell `last`: one column per emitted byte.
    let run_byte = entry.cell_ends[last];
    let run_col = u32::from(entry.cell_cols[last]) + u32::from(entry.cell_widths[last]);
    u16::try_from(run_col + (pos_u - run_byte) + 1)
        .unwrap_or(u16::MAX)
        .min(entry.cols)
}

/// Deterministic synchronous search (corpus tests, in-thread clients).
pub fn search_sync<R: HistoryRead + ?Sized>(
    reader: &mut R,
    req: &SearchRequest,
    token: u64,
) -> SearchOutcome {
    let cancel = AtomicBool::new(false);
    search_core(reader, req, token, &cancel, &|| false)
}

// ---------------------------------------------------------------------------
// Background worker with generation tokens and cancellation.
// ---------------------------------------------------------------------------

/// One queued search job: the request plus the generation token captured at
/// `start()` time (staleness is measured against request time, not worker
/// pickup time).
struct WorkerJob {
    token: u64,
    req: Box<SearchRequest>,
}

/// Cancellable background search worker (one dedicated thread; one pending
/// request slot, one result slot — both bounded).
pub struct SearchWorker {
    generation: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
    request: Arc<(Mutex<Option<WorkerJob>>, Condvar)>,
    result: Arc<(Mutex<Option<SearchOutcome>>, Condvar)>,
    thread: Option<JoinHandle<()>>,
}

impl SearchWorker {
    /// Spawn the worker thread. `reader` must be `Send`; the worker locks it
    /// only for single-line reads.
    pub fn new(reader: Arc<Mutex<dyn HistoryRead + Send>>) -> Self {
        Self::new_with_cancel(reader, Arc::new(AtomicBool::new(false)))
    }

    /// Advanced constructor with an externally owned cancel flag, shared with
    /// callers (deterministic cancellation from reader callbacks, tests, and
    /// engine wiring).
    pub fn new_with_cancel(
        reader: Arc<Mutex<dyn HistoryRead + Send>>,
        cancel: Arc<AtomicBool>,
    ) -> Self {
        let generation = Arc::new(AtomicU64::new(1));
        let shutdown = Arc::new(AtomicBool::new(false));
        let request: Arc<(Mutex<Option<WorkerJob>>, Condvar)> =
            Arc::new((Mutex::new(None), Condvar::new()));
        let result: Arc<(Mutex<Option<SearchOutcome>>, Condvar)> =
            Arc::new((Mutex::new(None), Condvar::new()));
        let thread = {
            let reader = Arc::clone(&reader);
            let generation = Arc::clone(&generation);
            let cancel = Arc::clone(&cancel);
            let shutdown = Arc::clone(&shutdown);
            let request = Arc::clone(&request);
            let result = Arc::clone(&result);
            std::thread::spawn(move || {
                worker_loop(reader, generation, cancel, shutdown, request, result);
            })
        };
        Self {
            generation,
            cancel,
            shutdown,
            request,
            result,
            thread: Some(thread),
        }
    }

    /// The current generation token. Outcomes with a different token are
    /// stale and must be discarded.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    /// Bump the generation token. Call when history mutates (feed, resize,
    /// alternate-screen transitions) so in-flight search results are
    /// detected as stale.
    pub fn note_history_changed(&self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
    }

    /// Start (or replace) a search. Bumps the generation token, so any
    /// in-flight search's outcome arrives stale; an empty needle stops the
    /// current search. The needle is truncated to [`MAX_NEEDLE_BYTES`].
    /// Returns the token of the new search.
    pub fn start(&self, mut req: SearchRequest) -> u64 {
        req.needle.truncate(MAX_NEEDLE_BYTES);
        // Reset cancellation for the new search *before* publishing it, so a
        // later cancel() can never be lost.
        self.cancel.store(false, Ordering::SeqCst);
        let token = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        let (mutex, cvar) = &*self.request;
        let mut guard = mutex.lock().expect("request lock");
        // Replacement semantics: the newest request wins; the slot never
        // holds more than one request.
        *guard = Some(WorkerJob {
            token,
            req: Box::new(req),
        });
        cvar.notify_one();
        drop(guard);
        token
    }

    /// Cancel the in-flight search; its outcome reports `cancelled: true`.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::SeqCst);
    }

    /// A handle to the worker's cancel flag (shared with the worker; useful
    /// for deterministic cancellation from reader callbacks and tests).
    pub fn cancel_handle(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.cancel)
    }

    /// Non-blocking poll for the latest outcome (replaces on overflow).
    pub fn poll(&self) -> Option<SearchOutcome> {
        let (mutex, _) = &*self.result;
        mutex.lock().expect("result lock").take()
    }

    /// Block until an outcome is available or `timeout` elapses.
    pub fn poll_wait(&self, timeout: Duration) -> Option<SearchOutcome> {
        let (mutex, cvar) = &*self.result;
        let mut guard = mutex.lock().expect("result lock");
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if let Some(outcome) = guard.take() {
                return Some(outcome);
            }
            let now = std::time::Instant::now();
            if now >= deadline {
                return None;
            }
            let (new_guard, wait_result) = cvar
                .wait_timeout(guard, deadline - now)
                .expect("result lock");
            guard = new_guard;
            if wait_result.timed_out() {
                return guard.take();
            }
        }
    }
}

impl Drop for SearchWorker {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        let (mutex, cvar) = &*self.request;
        let guard = mutex.lock().expect("request lock");
        cvar.notify_all();
        drop(guard);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn worker_loop(
    reader: Arc<Mutex<dyn HistoryRead + Send>>,
    _generation: Arc<AtomicU64>,
    cancel: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
    request: Arc<(Mutex<Option<WorkerJob>>, Condvar)>,
    result: Arc<(Mutex<Option<SearchOutcome>>, Condvar)>,
) {
    let (request_mutex, request_cvar) = &*request;
    let (result_mutex, result_cvar) = &*result;
    let mut guard = request_mutex.lock().expect("request lock");
    loop {
        while guard.is_none() && !shutdown.load(Ordering::Acquire) {
            guard = request_cvar.wait(guard).expect("request wait");
        }
        let Some(job) = guard.take() else {
            return; // shutdown
        };
        drop(guard);

        if !job.req.needle.is_empty() {
            let token = job.token;
            // A newer pending request supersedes the current search: the
            // worker aborts between lines and re-enters the loop to take it.
            let abort = || {
                matches!(
                    request_mutex.try_lock(),
                    Ok(pending) if pending.is_some()
                )
            };
            let outcome = {
                let mut reader_guard = reader.lock().expect("reader lock");
                search_core(&mut *reader_guard, &job.req, token, &cancel, &abort)
            };
            let mut result_guard = result_mutex.lock().expect("result lock");
            *result_guard = Some(outcome);
            result_cvar.notify_one();
            drop(result_guard);
        }

        guard = request_mutex.lock().expect("request lock");
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use mr_crabs_terminal::{GridSize, ScrollbackConfig, Terminal};
    use std::sync::Mutex;

    /// In-memory history for deterministic tests.
    struct VecHistory {
        lines: Vec<(u16, Vec<Cell>)>,
    }

    impl VecHistory {
        fn new() -> Self {
            Self { lines: Vec::new() }
        }

        fn push(&mut self, cols: u16, text: &str) {
            let mut cells = vec![Cell::default(); usize::from(cols)];
            for (i, ch) in text.chars().take(usize::from(cols)).enumerate() {
                cells[i].content = u32::from(ch);
            }
            self.lines.push((cols, cells));
        }

        fn push_wrapped(&mut self, cols: u16, text: &str) {
            let mut cells = vec![Cell::default(); usize::from(cols)];
            for (i, ch) in text.chars().take(usize::from(cols)).enumerate() {
                cells[i].content = u32::from(ch);
            }
            if let Some(last) = cells.last_mut() {
                last.flags |= WRAPLINE_BIT;
            }
            self.lines.push((cols, cells));
        }
    }

    impl HistoryRead for VecHistory {
        fn history_len(&self) -> usize {
            self.lines.len()
        }

        fn history_line_cols(&self, index: usize) -> Option<usize> {
            self.lines.get(index).map(|(cols, _)| usize::from(*cols))
        }

        fn read_history_line(&mut self, index: usize, out: &mut Vec<Cell>) -> bool {
            let Some((_, cells)) = self.lines.get(index) else {
                return false;
            };
            out.clear();
            out.extend_from_slice(cells);
            true
        }
    }

    fn cells(text: &str) -> Vec<Cell> {
        let mut cells = Vec::new();
        for ch in text.chars() {
            cells.push(Cell {
                content: u32::from(ch),
                style: 0,
                flags: 0,
            });
        }
        cells
    }

    fn req(
        needle: &str,
        direction: SearchDirection,
        start: SearchStart,
        limit: usize,
    ) -> SearchRequest {
        SearchRequest {
            needle: needle.as_bytes().to_vec(),
            direction,
            start,
            limit,
            case_sensitive: false,
            visible_rows: Vec::new(),
        }
    }

    #[test]
    fn row_encoding_trim_skip_and_join() {
        // Wide char + spacer: only the wide cell emits bytes.
        let mut wide = cells("界");
        wide.push(Cell {
            content: u32::from(' '),
            style: 0,
            flags: Cell::WIDE_SPACER,
        });
        assert_eq!(row_text(&wide), "界".as_bytes());

        // Trailing blanks trimmed, mid-line blanks kept as spaces.
        let line = cells("ab   cd  ");
        assert_eq!(row_text(&line), b"ab   cd");

        // Continuation rows keep the previous row's trailing blanks.
        let mut wrapped = cells("ab  ");
        if let Some(last) = wrapped.last_mut() {
            last.flags |= WRAPLINE_BIT;
        }
        let cont = cells("cd");
        let mut blank = 0usize;
        let mut out = Vec::new();
        let (mut t1, _, _, _, _) = encode_row_with_map(&wrapped, false, &mut blank);
        out.append(&mut t1);
        let (t2, _, _, _, _) = encode_row_with_map(&cont, true, &mut blank);
        out.extend_from_slice(&t2);
        assert_eq!(out, b"ab  cd", "wrapped-row blanks preserved mid-line");

        // Empty cells are blank.
        let empty: Vec<Cell> = vec![Cell::default(); 3];
        assert!(row_text(&empty).is_empty());
    }

    #[test]
    fn find_needle_is_ascii_case_insensitive() {
        let hay = b"Hello WORLD";
        assert_eq!(find_needle(hay, b"world", 0, false), Some(6));
        assert_eq!(find_needle(hay, b"HELLO", 0, false), Some(0));
        assert_eq!(find_needle(hay, b"world", 0, true), None);
        assert_eq!(find_needle(hay, b"WORLD", 0, true), Some(6));
        assert_eq!(find_needle(hay, b"x", 0, false), None);
        // Non-ASCII is not case-folded (Ghostty ASCII-only folding).
        assert_eq!(
            find_needle("ÄBC".as_bytes(), "äbc".as_bytes(), 0, false),
            None
        );
    }

    #[test]
    fn search_forward_across_wrapped_rows() {
        let mut history = VecHistory::new();
        // Row 0 wraps ("alpha beta " + trailing blank, 11 cols); row 1 is
        // its continuation; rows 2-3 are separate lines.
        history.push_wrapped(11, "alpha beta ");
        history.push(11, "gamma");
        history.push(11, "alpha again");
        history.push(11, "beta only");
        let mut reader = history;

        // Cross-row match: "ta gamma" spans the wrapped boundary and the
        // preserved trailing blank. The joined stream is
        // "alpha beta gamma" (the trailing blank of the wrapped row is
        // emitted because a non-blank follows), so 't' of "ta" is the
        // ninth byte: column 8.
        let outcome = search_sync(
            &mut reader,
            &req("ta gamma", SearchDirection::Forward, SearchStart::Top, 10),
            1,
        );
        assert!(outcome.completed);
        assert_eq!(outcome.matches.len(), 1);
        let m = &outcome.matches[0];
        assert_eq!(m.start_line, 0);
        assert_eq!(m.start_col, 8);
        assert_eq!(
            m.spans,
            vec![
                SearchSpan {
                    line: 0,
                    start_col: 8,
                    end_col: 11
                },
                SearchSpan {
                    line: 1,
                    start_col: 0,
                    end_col: 5
                },
            ]
        );

        // Same-line match.
        let outcome = search_sync(
            &mut reader,
            &req("alpha", SearchDirection::Forward, SearchStart::Top, 10),
            1,
        );
        assert_eq!(outcome.matches.len(), 2);
        assert_eq!(outcome.matches[0].start_line, 0);
        assert_eq!(outcome.matches[0].start_col, 0);
        assert_eq!(outcome.matches[0].spans[0].end_col, 5);
        assert_eq!(outcome.matches[1].start_line, 2);

        // Case-insensitive default.
        let outcome = search_sync(
            &mut reader,
            &req("GAMMA", SearchDirection::Forward, SearchStart::Top, 10),
            1,
        );
        assert_eq!(outcome.matches.len(), 1);
        assert_eq!(outcome.matches[0].start_line, 1);
    }

    #[test]
    fn search_reverse_and_limit_and_start() {
        let mut history = VecHistory::new();
        for i in 0..50 {
            history.push(20, &format!("needle line {i:02}"));
        }
        let mut reader = history;

        // Reverse from bottom finds the newest first. The limit of 3 cuts
        // the scan short, so the outcome is truncated, not completed.
        let outcome = search_sync(
            &mut reader,
            &req("needle", SearchDirection::Reverse, SearchStart::Bottom, 3),
            1,
        );
        assert!(outcome.truncated);
        assert!(!outcome.completed);
        assert_eq!(outcome.matches.len(), 3);
        assert_eq!(outcome.matches[0].start_line, 49);
        assert_eq!(outcome.matches[1].start_line, 48);
        assert_eq!(outcome.matches[2].start_line, 47);

        // Limit truncation.
        let outcome = search_sync(
            &mut reader,
            &req("needle", SearchDirection::Forward, SearchStart::Top, 5),
            1,
        );
        assert!(outcome.truncated);
        assert_eq!(outcome.matches.len(), 5);
        assert_eq!(outcome.matches[0].start_line, 0);

        // Start at a specific line.
        let outcome = search_sync(
            &mut reader,
            &req("needle", SearchDirection::Forward, SearchStart::Line(40), 2),
            1,
        );
        assert_eq!(outcome.matches[0].start_line, 40);
        assert_eq!(outcome.matches[1].start_line, 41);

        // Empty needle = inactive search.
        let outcome = search_sync(
            &mut reader,
            &req("", SearchDirection::Forward, SearchStart::Top, 10),
            1,
        );
        assert!(outcome.completed);
        assert!(outcome.matches.is_empty());
    }

    #[test]
    fn search_crosses_compressed_and_uncompressed_pages_identically() {
        let size = GridSize::new(10, 4);
        let mut hot_term = Terminal::new_with_config(
            size,
            ScrollbackConfig {
                max_lines: 1000,
                hot_page_lines: 2,
                max_queued_jobs: 8,
                max_pending_completions: 8,
            },
        )
        .unwrap();
        for i in 0..40 {
            // CRLF keeps each feed's two rows ("target NN " and "filler")
            // aligned at column 0; bare LF would drift the columns.
            hot_term.feed(format!("target {i:02} filler\r\n").as_bytes());
        }
        let mut cold_term = Terminal::new_with_config(
            size,
            ScrollbackConfig {
                max_lines: 1000,
                hot_page_lines: 2,
                max_queued_jobs: 8,
                max_pending_completions: 8,
            },
        )
        .unwrap();
        for i in 0..40 {
            cold_term.feed(format!("target {i:02} filler\r\n").as_bytes());
        }
        cold_term.force_compress_all();
        assert!(cold_term.storage_stats().compressed_bytes > 0);

        let req = SearchRequest {
            needle: b"target 12".to_vec(),
            direction: SearchDirection::Forward,
            start: SearchStart::Top,
            limit: 10,
            case_sensitive: false,
            visible_rows: Vec::new(),
        };
        let hot = search_sync(&mut hot_term, &req, 1);
        let cold = search_sync(&mut cold_term, &req, 1);
        assert_eq!(
            hot.matches, cold.matches,
            "compress must not change results"
        );
        assert_eq!(hot.completed, cold.completed);
        assert_eq!(hot.lines_searched, cold.lines_searched);
        assert_eq!(hot_term.history_len(), cold_term.history_len());
        // "target 12" occurs only in feed 12's first row; every other feed
        // carries a different index. Each feed adds two rows to history
        // (the "target NN " row and the "filler" row), starting with feed
        // 0's rows at lines 0-1, so feed 12's first row lands at history
        // line 2 * 12 = 24.
        assert_eq!(hot.matches.len(), 1);
        assert_eq!(hot.matches[0].start_line, 24, "feed 12 row 0");
        assert_eq!(hot.matches[0].start_col, 0);
        assert_eq!(hot.matches[0].spans[0].end_col, 9);
    }

    #[test]
    fn worker_delivers_results_and_generation_invalidates() {
        let mut history = VecHistory::new();
        for i in 0..100 {
            history.push(20, &format!("worker needle {i:02}"));
        }
        let reader: Arc<Mutex<dyn HistoryRead + Send>> = Arc::new(Mutex::new(history));
        let worker = SearchWorker::new(Arc::clone(&reader));

        let token = worker.start(req("needle", SearchDirection::Forward, SearchStart::Top, 5));
        let outcome = worker
            .poll_wait(Duration::from_secs(5))
            .expect("outcome delivered");
        assert!(!outcome.is_stale(worker.generation()));
        assert_eq!(outcome.token, token);
        assert_eq!(outcome.matches.len(), 5);
        assert!(outcome.truncated);
        assert_eq!(outcome.matches[0].start_line, 0);

        // History changed after the search started: the outcome is stale.
        let token2 = worker.start(req("needle", SearchDirection::Forward, SearchStart::Top, 5));
        worker.note_history_changed();
        let outcome2 = worker
            .poll_wait(Duration::from_secs(5))
            .expect("outcome delivered");
        assert_eq!(outcome2.token, token2);
        assert!(
            outcome2.is_stale(worker.generation()),
            "generation bump invalidates in-flight results"
        );
        drop(worker);
    }

    #[test]
    fn worker_cancellation_stops_mid_search() {
        struct CountingHistory {
            lines: usize,
            cancel: Arc<AtomicBool>,
            serve: usize,
        }
        impl HistoryRead for CountingHistory {
            fn history_len(&self) -> usize {
                self.lines
            }
            fn history_line_cols(&self, index: usize) -> Option<usize> {
                (index < self.lines).then_some(20)
            }
            fn read_history_line(&mut self, index: usize, out: &mut Vec<Cell>) -> bool {
                if index >= self.lines {
                    return false;
                }
                self.serve += 1;
                if self.serve >= 10 {
                    // Deterministically cancel mid-search through the
                    // worker's own flag.
                    self.cancel.store(true, Ordering::SeqCst);
                }
                out.clear();
                out.extend(cells(&format!("cancelled needle {index:02}")));
                true
            }
        }

        // One shared cancel flag: the reader sets it while the worker polls
        // it between lines, so cancellation fires after exactly ten served
        // lines (the reader cannot reach the worker's internal flag).
        let cancel = Arc::new(AtomicBool::new(false));
        let reader: Arc<Mutex<dyn HistoryRead + Send>> = Arc::new(Mutex::new(CountingHistory {
            lines: 10_000,
            cancel: Arc::clone(&cancel),
            serve: 0,
        }));
        let worker = SearchWorker::new_with_cancel(Arc::clone(&reader), Arc::clone(&cancel));
        let token = worker.start(req(
            "needle",
            SearchDirection::Forward,
            SearchStart::Top,
            1000,
        ));
        let outcome = worker
            .poll_wait(Duration::from_secs(5))
            .expect("outcome delivered");
        assert!(outcome.cancelled, "cancellation stops the search");
        assert!(!outcome.completed);
        assert!(outcome.lines_searched < 10_000);
        assert_eq!(outcome.token, token);
        drop(worker);
    }

    #[test]
    fn worker_replaces_pending_request() {
        let mut history = VecHistory::new();
        for i in 0..2000 {
            history.push(20, &format!("replace needle {i:02}"));
        }
        let reader: Arc<Mutex<dyn HistoryRead + Send>> = Arc::new(Mutex::new(history));
        let worker = SearchWorker::new(reader);
        let token_a = worker.start(req(
            "needle",
            SearchDirection::Forward,
            SearchStart::Top,
            1000,
        ));
        // The replacement needle must actually occur in the history lines
        // ("replace needle {i:02}") so the replacement search yields
        // matches.
        let token_b = worker.start(req(
            "replace",
            SearchDirection::Forward,
            SearchStart::Top,
            5,
        ));
        assert!(token_b > token_a);
        // The first search is aborted by the replacement; the second outcome
        // is delivered afterwards. Poll until the newest token arrives.
        let deadline = std::time::Instant::now() + Duration::from_secs(5);
        let mut outcome = None;
        while std::time::Instant::now() < deadline {
            if let Some(o) = worker.poll() {
                if o.token == token_b {
                    outcome = Some(o);
                    break;
                }
                assert!(
                    o.cancelled || o.token == token_a,
                    "interim outcome from the replaced search"
                );
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let outcome = outcome.expect("replacement outcome delivered");
        assert!(!outcome.matches.is_empty());
        drop(worker);
    }

    #[test]
    fn search_spans_visible_rows() {
        let size = GridSize::new(10, 3);
        let mut term = Terminal::new(size).unwrap();
        // CRLF so "three" starts at column 0 of the second visible row
        // (a bare LF would leave the cursor at column 7 and wrap the row).
        term.feed(b"one two\r\nthree\r\n");
        let snap = term.snapshot();
        let visible = crate::visible_rows(&snap);
        let mut req = req("three", SearchDirection::Forward, SearchStart::Top, 10);
        req.visible_rows = visible;
        let outcome = search_sync(&mut term, &req, 1);
        assert_eq!(outcome.matches.len(), 1);
        let m = &outcome.matches[0];
        assert_eq!(m.start_line, 1, "match lives in the visible grid row 1");
        assert_eq!(m.start_col, 0);
        assert_eq!(m.spans[0].end_col, 5);
    }

    #[test]
    fn needle_truncation_is_bounded() {
        let mut history = VecHistory::new();
        history.push(300, &format!("x{}y", "n".repeat(298)));
        let mut reader = history;
        let long: String = "n".repeat(400);
        let outcome = search_sync(
            &mut reader,
            &req(&long, SearchDirection::Forward, SearchStart::Top, 10),
            1,
        );
        assert!(outcome.completed);
        // The needle is truncated to 255 'n's, which still matches inside
        // the 298-'n' line.
        assert_eq!(outcome.matches.len(), 1);
    }

    #[test]
    fn bounded_slices_resume_and_preserve_wrapped_matches() {
        let mut history = VecHistory::new();
        history.push(4, "none");
        history.push_wrapped(3, "hel");
        history.push(2, "lo");
        history.push(4, "tail");
        let request = req("hello", SearchDirection::Forward, SearchStart::Top, 10);
        let cancel = AtomicBool::new(false);

        let (first, next) = search_slice(&mut history, &request, 0, 1, 7, &cancel);
        assert!(!first.completed);
        assert!(first.matches.is_empty());
        assert_eq!(next, 1);
        let (second, next) = search_slice(&mut history, &request, next, 1, 7, &cancel);
        assert!(!second.completed);
        assert_eq!(next, 3);
        assert_eq!(second.matches.len(), 1);
        assert_eq!(second.matches[0].start_line, 1);
        assert_eq!(second.matches[0].spans.last().expect("last span").line, 2);

        let (third, done) = search_slice(&mut history, &request, next, 1, 7, &cancel);
        assert!(third.completed);
        assert_eq!(done, 4);
        assert!(third.matches.is_empty());
    }
}

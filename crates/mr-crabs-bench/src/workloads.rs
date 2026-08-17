//! S12 release workloads.
//!
//! Each workload performs exactly ONE run in the calling process. The
//! Python gate driver (`verification/tools/release_gate.py`) invokes the
//! bench binary once per run (one warmup + five isolated measured runs in
//! fresh processes), aggregates medians and p50/p95/p99 with nearest-rank
//! arithmetic, compares only byte-identical oracle payloads, and fails
//! closed on missing or comparison-ineligible data.
//!
//! Status vocabulary (results schema, `verification/manifests/s12-schema.json`):
//! - `measured`: real numbers were captured; the workload can satisfy gates.
//! - `failed`: the workload errored; the gate fails.
//! - `blocked`: `not_measured` with an exact reason; can never satisfy a gate.

use crate::memory::FootprintTracker;
use crate::payloads;
use crate::stats;
use mr_crabs_element::RenderCache;
use mr_crabs_pty::{CommandBuilder, PtyConfig, PtySession, PtySize};
use mr_crabs_terminal::{Cell, DamageKind, FramePool, GridSize, Terminal};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::BTreeMap;
use std::collections::VecDeque;
use std::path::Path;
use std::sync::mpsc::RecvTimeoutError;
use std::time::{Duration, Instant};

/// Schema version of S12 release results.
pub const RELEASE_SCHEMA_VERSION: u32 = 1;

/// Metrics captured by one measured workload run. Every field is optional:
/// a metric that cannot be measured on this platform or for this workload
/// stays `None` (never a fabricated zero).
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Metrics {
    pub wall_ns: Option<u64>,
    pub max_rss_bytes: Option<u64>,
    pub peak_footprint_bytes: Option<u64>,
    pub current_rss_bytes: Option<u64>,
    pub allocations: Option<u64>,
    pub allocated_bytes: Option<u64>,
    pub throughput_mib_s: Option<f64>,
    pub payload_bytes: Option<u64>,
    pub logical_lines: Option<u64>,
    pub hot_resident_bytes: Option<u64>,
    pub compressed_bytes: Option<u64>,
    pub engines: Option<u64>,
    pub resizes: Option<u64>,
    pub frames_built: Option<u64>,
    pub dirty_rows: Option<u64>,
    pub redraw_requests: Option<u64>,
    pub idle_redraw_requests: Option<u64>,
    pub idle_animation_requests: Option<u64>,
    pub capacity_growth_bytes: Option<u64>,
    pub cache_ok: Option<bool>,
    pub frame_build_mean_ns: Option<u64>,
    pub frame_build_p50_ns: Option<u64>,
    pub frame_build_p95_ns: Option<u64>,
    pub frame_build_p99_ns: Option<u64>,
    pub launch_to_prompt_ns: Option<u64>,
    pub prompt_bytes: Option<u64>,
    pub echo_bytes: Option<u64>,
    pub echo_mib_s: Option<f64>,
    pub child_reaped: Option<bool>,
    pub child_alive_after_reap: Option<bool>,
    pub exit_code: Option<i32>,
    pub images_decoded: Option<u64>,
    pub decoded_bytes: Option<u64>,
    pub decode_mib_s: Option<f64>,
    pub search_matches: Option<u64>,
    pub search_lines_scanned: Option<u64>,
    pub worker_matches: Option<u64>,
    pub worker_round_trip_ns: Option<u64>,
    pub worker_cancelled: Option<bool>,
    pub effects_frames: Option<u64>,
    pub frames_until_idle: Option<u64>,
    pub frames_after_expiry: Option<u64>,
    pub effects_retained_capacity: Option<u64>,
    pub effects_disabled_retained_capacity: Option<u64>,
}

/// Payload identity reported with a measured result.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PayloadRef {
    pub id: String,
    pub bytes: u64,
}

/// One S12 workload run result (single invocation, single run).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReleaseRunResult {
    pub schema_version: u32,
    pub suite: String,
    pub workload: String,
    /// `measured` | `failed` | `blocked`.
    pub status: String,
    /// Exact reason for `blocked`/`failed`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    /// Canonical payload reference (deterministic workloads only).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload: Option<PayloadRef>,
    /// FNV-1a 64 identity of the payload actually generated, hex.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub payload_fnv1a64: Option<String>,
    /// Workload spec (replay scripts, PTY setup, ...) for corpus identity.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub spec: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub metrics: Option<Metrics>,
}

/// One-process aggregate document: every release workload run once.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReleaseAggregateResult {
    pub schema_version: u32,
    pub suite: String,
    pub mode: String,
    pub workloads: BTreeMap<String, ReleaseRunResult>,
}

/// Internal outcome of one workload.
struct WorkloadOutcome {
    status: &'static str,
    reason: Option<String>,
    payload: Option<PayloadRef>,
    payload_fnv1a64: Option<String>,
    spec: Option<serde_json::Value>,
    metrics: Option<Metrics>,
}

fn measured(
    metrics: Metrics,
    payload: Option<PayloadRef>,
    payload_fnv1a64: Option<String>,
    spec: Option<serde_json::Value>,
) -> WorkloadOutcome {
    WorkloadOutcome {
        status: "measured",
        reason: None,
        payload,
        payload_fnv1a64,
        spec,
        metrics: Some(metrics),
    }
}

fn failed(reason: impl Into<String>) -> WorkloadOutcome {
    WorkloadOutcome {
        status: "failed",
        reason: Some(reason.into()),
        payload: None,
        payload_fnv1a64: None,
        spec: None,
        metrics: None,
    }
}

fn blocked(reason: impl Into<String>) -> WorkloadOutcome {
    WorkloadOutcome {
        status: "blocked",
        reason: Some(reason.into()),
        payload: None,
        payload_fnv1a64: None,
        spec: None,
        metrics: None,
    }
}

/// Run one S12 release workload (a single run in this process).
pub fn run_release_workload(name: &str) -> ReleaseRunResult {
    let outcome = match name {
        "ascii_10mb" => throughput_workload("ascii_10mb"),
        "unicode_10mb" => throughput_workload("unicode_10mb"),
        "scrollback_1m" => scrollback_workload(),
        "resize_storm" => resize_storm_workload(),
        "redraw_replay" => redraw_replay_workload(),
        "engines_1" => engines_workload(1),
        "engines_10" => engines_workload(10),
        "engines_50" => engines_workload(50),
        "headless_idle" => headless_idle_workload(),
        "headless_cache" => headless_cache_workload(),
        "pty_launch_to_prompt" => pty_launch_to_prompt_workload(),
        "pty_echo" => pty_echo_workload(),
        "image_decode_stress" => image_decode_stress_workload(),
        "effects" => effects_workload(),
        "search" => search_workload(),
        "gui_frame_time" => blocked(
            "not_measured: requires an authorized GUI render surface; the S12 release bench is headless and never launches a GUI instance",
        ),
        "window_redraw" => blocked(
            "not_measured: requires an authorized GUI render surface; the S12 release bench is headless and never launches a GUI instance",
        ),
        "strict_gui_idle" => blocked(
            "not_measured: requires an authorized GUI render surface; the S12 release bench is headless and never launches a GUI instance",
        ),
        "energy" => blocked(
            "not_measured: powermetrics requires root authorization and is not invoked by the S12 release bench",
        ),
        other => failed(format!("unknown release workload {other:?}")),
    };
    ReleaseRunResult {
        schema_version: RELEASE_SCHEMA_VERSION,
        suite: "release".to_owned(),
        workload: name.to_owned(),
        status: outcome.status.to_owned(),
        reason: outcome.reason,
        payload: outcome.payload,
        payload_fnv1a64: outcome.payload_fnv1a64,
        spec: outcome.spec,
        metrics: outcome.metrics,
    }
}

/// Run every release workload once and return the aggregate document.
pub fn run_release_aggregate() -> ReleaseAggregateResult {
    let workloads = RELEASE_WORKLOADS
        .iter()
        .map(|name| (name.to_string(), run_release_workload(name)))
        .collect();
    ReleaseAggregateResult {
        schema_version: RELEASE_SCHEMA_VERSION,
        suite: "release".to_owned(),
        mode: "aggregate".to_owned(),
        workloads,
    }
}

/// Stable release workload id list (order also used by the gate driver).
pub const RELEASE_WORKLOADS: [&str; 19] = [
    "ascii_10mb",
    "unicode_10mb",
    "scrollback_1m",
    "resize_storm",
    "redraw_replay",
    "engines_1",
    "engines_10",
    "engines_50",
    "headless_idle",
    "headless_cache",
    "pty_launch_to_prompt",
    "pty_echo",
    "image_decode_stress",
    "effects",
    "search",
    "gui_frame_time",
    "window_redraw",
    "strict_gui_idle",
    "energy",
];

/// Shared measured-metrics wrapper: wall time, RSS/footprint, allocations.
fn memory_metrics(
    start: Instant,
    tracker: &mut FootprintTracker,
    before: crate::alloc::AllocationStats,
) -> Metrics {
    let after = crate::alloc::stats();
    Metrics {
        wall_ns: Some(start.elapsed().as_nanos() as u64),
        max_rss_bytes: crate::memory::peak_rss_bytes(),
        peak_footprint_bytes: tracker.peak_bytes(),
        current_rss_bytes: crate::memory::current_rss_bytes(),
        allocations: Some(after.count - before.count),
        allocated_bytes: Some(after.bytes - before.bytes),
        ..Metrics::default()
    }
}

fn grid(size: (u16, u16)) -> GridSize {
    GridSize::new(size.0, size.1)
}

/// True when every cell of a row is a default (blank) cell. Blank rows are
/// cursor-movement/wrap artifacts of terminal scrolling; the S12 corpus
/// payload records always write content, so blank rows are never records.
fn row_is_empty(row: &[Cell]) -> bool {
    row.iter().all(Cell::is_default)
}

// ---------------------------------------------------------------------------
// Throughput: 10 MiB ASCII and mixed Unicode
// ---------------------------------------------------------------------------

/// FNV-1a 64 offset basis and prime, mirroring `payloads` identity hashing.
const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;

/// Incremental FNV-1a 64 hasher for streamed payload identity checks.
#[derive(Clone, Copy, Debug, Default)]
struct Fnv1a64 {
    hash: u64,
}

impl Fnv1a64 {
    fn new() -> Self {
        Self { hash: FNV_OFFSET }
    }

    fn update(&mut self, bytes: &[u8]) {
        for &b in bytes {
            self.hash ^= u64::from(b);
            self.hash = self.hash.wrapping_mul(FNV_PRIME);
        }
    }

    fn finish_hex(self) -> String {
        format!("{:016x}", self.hash)
    }
}

/// Fill `out` with bytes `[offset, offset + len)` of the infinite
/// repetition of `seed` (the payload's minimal period), reusing `out`'s
/// allocation. The fill starts at `offset % seed.len()`, so every chunk
/// carries the canonical bytes at its global stream offset even when the
/// chunk size is not period-aligned.
fn fill_period_phase(out: &mut Vec<u8>, seed: &[u8], offset: u64, len: usize) {
    out.clear();
    if len == 0 || seed.is_empty() {
        return;
    }
    let mut pos = (offset % seed.len() as u64) as usize;
    while out.len() < len {
        let n = (len - out.len()).min(seed.len() - pos);
        out.extend_from_slice(&seed[pos..pos + n]);
        pos = 0;
    }
}

/// Fixed 9-byte SGR prefix shared by every 48-byte Unicode cycle
/// (`ESC[31m` bytes 0-4, `ESC[0m` bytes 5-8).
const UNICODE_SGR: [u8; 9] = [0x1b, b'[', b'3', b'1', b'm', 0x1b, b'[', b'0', b'm'];

/// Tripled letter alphabet (78 bytes). The 30-letter window of cycle `k`
/// starts at `(k*7 + 18) % 26`; the maximum start 25 + 30 = 55 stays in
/// bounds without a wrap branch.
const UNICODE_LETTERS: [u8; 78] = {
    let mut out = [0u8; 78];
    let mut i = 0;
    while i < 78 {
        out[i] = b'A' + (i % 26) as u8;
        i += 1;
    }
    out
};

/// UTF-8 encoding table for the payload's CJK range: entry `i` holds the
/// three-byte UTF-8 scalar of `U+4E00 + i` (36 KiB total).
fn unicode_cjk_table() -> Vec<u8> {
    let mut out = Vec::with_capacity(0x3000 * 3);
    for i in 0..0x3000u32 {
        let cp = 0x4E00 + i;
        out.push(0xE0 | (cp >> 12) as u8);
        out.push(0x80 | ((cp >> 6) & 0x3F) as u8);
        out.push(0x80 | (cp & 0x3F) as u8);
    }
    out
}

/// Fill `out` with canonical Unicode payload bytes `[offset, offset + len)`.
///
/// Each 48-byte cycle (`unicode_48_cycle`) is the fixed SGR prefix, three
/// 3-byte UTF-8 CJK scalars `U+4E00 + ((k*3 + c) % 0x3000)`, then 30
/// letters `A + ((k*7 + r) % 26)`. The k-dependent bytes are memcpy'd from
/// bounded cyclic tables: the CJK window starts at `(k*3) % 0x3000` (the
/// residues of `k*3 mod 0x3000` are multiples of 3, so the 3-byte entry
/// never straddles the window) and the letter window starts at
/// `(k*7 + 18) % 26`. The joint period is lcm(4096, 26) = 53248 cycles, so
/// the tables are a reusable cyclic source for the whole 10 MiB stream.
fn fill_unicode_period(out: &mut Vec<u8>, cjk: &[u8], offset: u64, len: usize) {
    out.clear();
    let mut remaining = len;
    let mut at = offset;
    while remaining > 0 {
        let k = at / 48;
        let start = (at % 48) as usize;
        let take = (48 - start).min(remaining);
        let mut cycle = [0u8; 48];
        cycle[..9].copy_from_slice(&UNICODE_SGR);
        let cjk_start = ((k * 3) % 0x3000) as usize * 3;
        cycle[9..18].copy_from_slice(&cjk[cjk_start..cjk_start + 9]);
        let letter_start = ((k * 7 + 18) % 26) as usize;
        cycle[18..48].copy_from_slice(&UNICODE_LETTERS[letter_start..letter_start + 30]);
        out.extend_from_slice(&cycle[start..start + take]);
        remaining -= take;
        at += take as u64;
    }
}

/// Canonical S12 throughput payload as a bounded, reusable chunk source.
///
/// Both the identity pre-pass and the measured feed generate every chunk
/// from this one source, so the hashed stream and the fed stream are
/// identical by construction. The source holds only the payload's cyclic
/// building blocks (an 832-byte ASCII period seed, a 36 KiB UTF-8 CJK
/// table, and a 78-byte letter alphabet) — never the 10 MiB payload — so
/// process peak RSS stays at oracle levels.
struct ThroughputPayload<'a> {
    id: &'static str,
    ascii_seed: &'a [u8],
    unicode_cjk: &'a [u8],
}

impl ThroughputPayload<'_> {
    /// Fill `out` with canonical payload bytes `[offset, offset + len)`.
    fn fill_chunk(&self, out: &mut Vec<u8>, offset: u64, len: usize) {
        match self.id {
            "ascii_10mb" => fill_period_phase(out, self.ascii_seed, offset, len),
            "unicode_10mb" => fill_unicode_period(out, self.unicode_cjk, offset, len),
            _ => unreachable!("throughput ids are validated by the caller"),
        }
        if self.id == "unicode_10mb" {
            // The canonical payload replaces the final byte (the lead byte
            // of an otherwise truncated third CJK codepoint) with ASCII
            // 'A'; the last chunk carries that boundary byte.
            let last = payloads::TEN_MIB as u64;
            if offset + len as u64 == last {
                out[len - 1] = b'A';
            }
        }
    }
}

fn throughput_workload(id: &'static str) -> WorkloadOutcome {
    let pin = match id {
        "ascii_10mb" => payloads::ASCII_10MB_FNV1A64,
        "unicode_10mb" => payloads::UNICODE_10MB_FNV1A64,
        _ => return failed("internal: unexpected throughput workload id"),
    };
    const CHUNK: usize = 256 * 1024;
    // `ascii_byte` minimal period: 32-byte SGR/letter cycle whose letters
    // advance once per cycle (`(j / 32) % 26`).
    const ASCII_PERIOD: u64 = 32 * 26;
    let total = payloads::TEN_MIB as u64;

    // Build the bounded cyclic source outside the measured window. It never
    // materializes the 10 MiB payload (holding it would add ~10 MiB to
    // process-peak RSS, which the S0 oracle baseline demonstrably never
    // held); the ASCII period seed and the Unicode tables are derived from
    // the authoritative byte functions and validated by the pinned FNV
    // hash below, which fails closed on any drift.
    let ascii_seed: Vec<u8> = if id == "ascii_10mb" {
        (0..ASCII_PERIOD).map(payloads::ascii_byte).collect()
    } else {
        Vec::new()
    };
    let unicode_cjk: Vec<u8> = if id == "unicode_10mb" {
        unicode_cjk_table()
    } else {
        Vec::new()
    };
    let source = ThroughputPayload {
        id,
        ascii_seed: &ascii_seed,
        unicode_cjk: &unicode_cjk,
    };

    // Streaming identity pre-pass (outside the measured window): hash the
    // canonical payload chunk by chunk through the same source the feed
    // uses, so the hashed stream and the fed stream are identical by
    // construction. The ASCII seed is phase-filled at each chunk's global
    // offset (the 256 KiB chunk size is not a multiple of the 832-byte
    // period, so restarting the seed per chunk would corrupt the stream).
    let mut hasher = Fnv1a64::new();
    let mut scratch: Vec<u8> = Vec::with_capacity(CHUNK);
    let mut offset = 0u64;
    while offset < total {
        let len = ((total - offset) as usize).min(CHUNK);
        source.fill_chunk(&mut scratch, offset, len);
        hasher.update(&scratch);
        offset += len as u64;
    }
    let actual_pin = hasher.finish_hex();
    if actual_pin != pin {
        return failed(format!(
            "{id} payload identity mismatch: generated {actual_pin}, pinned {pin}"
        ));
    }

    let mut term = match Terminal::new(grid((80, 24))) {
        Ok(term) => term,
        Err(err) => return failed(format!("Terminal::new failed: {err:?}")),
    };
    // The pinned Ghostty parser oracle uses a no-scrollback terminal. Keep
    // this workload parser-only; scrollback retention has its own 1M-line
    // workload below.
    let mut config = term.scrollback_config();
    config.max_lines = 0;
    term.set_scrollback_config(config);
    let mut tracker = FootprintTracker::new();
    let before = crate::alloc::stats();
    let start = Instant::now();
    // Feed in 256 KiB chunks (PTY-style), sampling footprint per chunk.
    // Chunks are produced by the same canonical source into the reused
    // scratch (never retained), so the measured window holds the same
    // bytes as the oracle stream.
    let mut offset = 0u64;
    while offset < total {
        let len = ((total - offset) as usize).min(CHUNK);
        source.fill_chunk(&mut scratch, offset, len);
        term.feed(&scratch);
        tracker.tick();
        offset += len as u64;
    }
    let metrics = memory_metrics(start, &mut tracker, before);
    // Post-measurement drain: prevents DCE and settles compression.
    let _ = term.snapshot();
    term.drain_compression();

    let wall_s = metrics.wall_ns.unwrap_or(1) as f64 / 1e9;
    let mut metrics = metrics;
    metrics.throughput_mib_s = Some(total as f64 / wall_s / 1048576.0);
    metrics.payload_bytes = Some(total);
    let spec = json!({
        "grid": [80, 24],
        "scrollback_lines": 0,
        "chunk_bytes": CHUNK,
    });
    measured(
        metrics,
        Some(PayloadRef {
            id: id.to_owned(),
            bytes: total,
        }),
        Some(actual_pin),
        Some(spec),
    )
}

// ---------------------------------------------------------------------------
// 1M-line scrollback
// ---------------------------------------------------------------------------

/// Cell contents of one payload record, derived from the canonical payload:
/// the bytes up to the first newline (`line`) map 1:1 to cell content for
/// the ASCII corpus. Record retention scores every retained row for this
/// content, so records are counted from actual record boundaries/content
/// rather than from physical row counts.
fn payload_record_cells(payload: &[u8]) -> Vec<u32> {
    payload
        .split(|&b| b == b'\n')
        .next()
        .unwrap_or_default()
        .iter()
        .map(|&b| u32::from(b))
        .collect()
}

/// Number of payload records in one retained row: non-overlapping matches of
/// the record cell content. In the LF-preserving layout a record never spans
/// rows (4 cells at a preserved cursor column), so each occurrence is exactly
/// one record. Wrap fragments (WRAPLINE-flagged blank rows) and the blank
/// cursor row contain no record content and score zero, so they are never
/// counted as records.
fn row_record_count(row: &[Cell], record: &[u32]) -> usize {
    if record.is_empty() || record.len() > row.len() {
        return 0;
    }
    let mut count = 0;
    let mut start = 0;
    while start + record.len() <= row.len() {
        if row[start..start + record.len()]
            .iter()
            .map(|cell| cell.content)
            .eq(record.iter().copied())
        {
            count += 1;
            start += record.len();
        } else {
            start += 1;
        }
    }
    count
}

fn scrollback_workload() -> WorkloadOutcome {
    const RECORD: &[u8; 5] = b"line\n";
    const RECORDS_PER_BATCH: usize = 4096;
    let payload_len = payloads::SCROLLBACK_BYTES;
    let mut hash = 0xcbf29ce484222325u64;
    for _ in 0..payloads::SCROLLBACK_LINES {
        for &byte in RECORD {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100000001b3);
        }
    }
    let actual_pin = format!("{hash:016x}");
    if actual_pin != payloads::SCROLLBACK_FNV1A64 {
        return failed(format!(
            "scrollback_1m payload identity mismatch: generated {actual_pin}, pinned {}",
            payloads::SCROLLBACK_FNV1A64
        ));
    }
    let batch = RECORD.repeat(RECORDS_PER_BATCH);
    let size = grid((80, 24));
    let mut term = match Terminal::new(size) {
        Ok(term) => term,
        Err(err) => return failed(format!("Terminal::new failed: {err:?}")),
    };
    let mut config = term.scrollback_config();
    // The LF-only payload preserves the cursor column across line feeds, so
    // at 80 columns the wrap after every 20th record fires while the cursor
    // sits on the row below the last content row: the feed creates
    // SCROLLBACK_LINES content rows + SCROLLBACK_LINES/20 WRAPLINE-flagged
    // wrap rows + the blank cursor row (1,050,000 physical rows total). The
    // storage budget must cover the physical rows or the oldest records are
    // evicted (line loss).
    config.max_lines =
        payloads::SCROLLBACK_LINES + payloads::SCROLLBACK_LINES / 20 + 1 + usize::from(size.rows);
    term.set_scrollback_config(config);

    let mut tracker = FootprintTracker::new();
    let before = crate::alloc::stats();
    let start = Instant::now();
    // Feed the same canonical `line\n` corpus in 4096-line batches without
    // retaining a second 5 MiB copy beside the terminal under measurement.
    let full_batches = payloads::SCROLLBACK_LINES / RECORDS_PER_BATCH;
    for _ in 0..full_batches {
        term.feed(&batch);
        tracker.tick();
    }
    let remaining = payloads::SCROLLBACK_LINES % RECORDS_PER_BATCH;
    if remaining != 0 {
        term.feed(&batch[..remaining * RECORD.len()]);
        tracker.tick();
    }
    let mut metrics = memory_metrics(start, &mut tracker, before);
    // Settle compression outside the measured feed window, then read stats.
    term.drain_compression();
    term.force_compress_all();
    let storage = term.storage_stats();
    // Retained payload records are counted from actual record content, not
    // physical row counts: score every retained row (paged history + visible
    // screen) for occurrences of the record cell content. WRAPLINE-flagged
    // wrap fragments and the blank cursor row contain no record content and
    // are never counted; an evicted record row removes its occurrences, so
    // the score is exactly the retained payload record count.
    let record = payload_record_cells(RECORD);
    let stored_records =
        term.fold_history_lines(0usize, |acc, row| acc + row_record_count(row, &record));
    let visible_records: usize = term
        .visible_rows()
        .iter()
        .map(|row| row_record_count(row, &record))
        .sum();
    let logical_lines = stored_records + visible_records;
    metrics.logical_lines = Some(logical_lines as u64);
    metrics.hot_resident_bytes = Some(storage.hot_resident_bytes as u64);
    metrics.compressed_bytes = Some(storage.compressed_bytes as u64);
    metrics.payload_bytes = Some(payload_len as u64);
    let wall_s = metrics.wall_ns.unwrap_or(1) as f64 / 1e9;
    metrics.throughput_mib_s = Some(payload_len as f64 / wall_s / 1048576.0);

    if logical_lines != payloads::SCROLLBACK_LINES {
        return failed(format!(
            "scrollback_1m retained {logical_lines} records != {} (line loss)",
            payloads::SCROLLBACK_LINES
        ));
    }

    measured(
        metrics,
        Some(PayloadRef {
            id: "scrollback_1m".to_owned(),
            bytes: payload_len as u64,
        }),
        Some(actual_pin),
        None,
    )
}

// ---------------------------------------------------------------------------
// Resize storm
// ---------------------------------------------------------------------------

const RESIZE_STORM_STEPS: u64 = 256;
const RESIZE_STORM_SEED: u64 = 4242;
const RESIZE_STORM_CHUNK: usize = 4096;
const RESIZE_STORM_SIZES: [(u16, u16); 5] = [(80, 24), (200, 60), (40, 10), (120, 40), (160, 50)];

fn resize_storm_workload() -> WorkloadOutcome {
    let mut term = match Terminal::new(grid(RESIZE_STORM_SIZES[0])) {
        Ok(term) => term,
        Err(err) => return failed(format!("Terminal::new failed: {err:?}")),
    };
    let mut pool = FramePool::new(4);
    let mut tracker = FootprintTracker::new();
    let before = crate::alloc::stats();
    let start = Instant::now();
    for step in 0..RESIZE_STORM_STEPS {
        let size = RESIZE_STORM_SIZES[(step % RESIZE_STORM_SIZES.len() as u64) as usize];
        if let Err(err) = term.resize(grid(size)) {
            return failed(format!("resize failed at step {step}: {err:?}"));
        }
        let chunk = payloads::seeded_chunk(RESIZE_STORM_SEED, step, RESIZE_STORM_CHUNK);
        term.feed(&chunk);
        tracker.tick();
    }
    let mut metrics = memory_metrics(start, &mut tracker, before);
    let frame = term.build_frame_delta(&mut pool);
    metrics.resizes = Some(RESIZE_STORM_STEPS);
    metrics.frames_built = Some(1);
    metrics.dirty_rows = Some(frame.rows.len() as u64);
    metrics.redraw_requests = Some(u64::from(frame.damage != DamageKind::Clean));
    pool.release(frame);
    let spec = json!({
        "steps": RESIZE_STORM_STEPS,
        "chunk_bytes": RESIZE_STORM_CHUNK,
        "seed": RESIZE_STORM_SEED,
        "sizes": RESIZE_STORM_SIZES,
    });
    measured(metrics, None, None, Some(spec))
}

// ---------------------------------------------------------------------------
// Redraw replay
// ---------------------------------------------------------------------------

const REPLAY_STEPS: u64 = 512;
// Pinned corpus seed (verification/corpus/replay/payloads.json, redraw_replay.seed).
const REPLAY_SEED: u64 = 24261;
const REPLAY_CHUNK: usize = 4096;
const REPLAY_RESIZE_EVERY: u64 = 32;
const REPLAY_SIZES: [(u16, u16); 5] = [(80, 24), (132, 43), (40, 10), (200, 60), (100, 30)];

fn redraw_replay_workload() -> WorkloadOutcome {
    let mut term = match Terminal::new(grid(REPLAY_SIZES[0])) {
        Ok(term) => term,
        Err(err) => return failed(format!("Terminal::new failed: {err:?}")),
    };
    let mut pool = FramePool::new(4);
    let mut tracker = FootprintTracker::new();
    let before = crate::alloc::stats();
    let start = Instant::now();
    let mut frame_times: Vec<u64> = Vec::with_capacity(REPLAY_STEPS as usize);
    let mut redraw_requests = 0u64;
    let mut dirty_rows = 0u64;
    for step in 0..REPLAY_STEPS {
        if step % REPLAY_RESIZE_EVERY == 0 {
            let size = REPLAY_SIZES[(step / REPLAY_RESIZE_EVERY) as usize % REPLAY_SIZES.len()];
            if let Err(err) = term.resize(grid(size)) {
                return failed(format!("resize failed at step {step}: {err:?}"));
            }
        }
        let chunk = payloads::seeded_chunk(REPLAY_SEED, step, REPLAY_CHUNK);
        term.feed(&chunk);
        let frame_start = Instant::now();
        let frame = term.build_frame_delta(&mut pool);
        frame_times.push(frame_start.elapsed().as_nanos() as u64);
        if frame.damage != DamageKind::Clean {
            redraw_requests += 1;
        }
        dirty_rows += frame.rows.len() as u64;
        pool.release(frame);
        tracker.tick();
    }
    let mut metrics = memory_metrics(start, &mut tracker, before);
    frame_times.sort_unstable();
    metrics.frames_built = Some(REPLAY_STEPS);
    metrics.redraw_requests = Some(redraw_requests);
    metrics.dirty_rows = Some(dirty_rows);
    let sum: u128 = frame_times.iter().map(|&t| u128::from(t)).sum();
    metrics.frame_build_mean_ns = Some((sum / frame_times.len() as u128) as u64);
    metrics.frame_build_p50_ns = stats::percentile(&frame_times, 50.0);
    metrics.frame_build_p95_ns = stats::percentile(&frame_times, 95.0);
    metrics.frame_build_p99_ns = stats::percentile(&frame_times, 99.0);
    let spec = json!({
        "steps": REPLAY_STEPS,
        "chunk_bytes": REPLAY_CHUNK,
        "seed": REPLAY_SEED,
        "resize_every": REPLAY_RESIZE_EVERY,
        "sizes": REPLAY_SIZES,
    });
    measured(metrics, None, None, Some(spec))
}

// ---------------------------------------------------------------------------
// 1 / 10 / 50 terminal engines
// ---------------------------------------------------------------------------

fn engines_workload(count: u64) -> WorkloadOutcome {
    let payload = payloads::sample_ascii_lines(1024 * 1024);
    let mut tracker = FootprintTracker::new();
    let before = crate::alloc::stats();
    let start = Instant::now();
    let mut terms = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let mut term = match Terminal::new(grid((80, 24))) {
            Ok(term) => term,
            Err(err) => return failed(format!("Terminal::new failed: {err:?}")),
        };
        term.feed(&payload);
        tracker.tick();
        terms.push(term);
    }
    // Settle compression so the footprint reflects steady state.
    for term in &mut terms {
        term.drain_compression();
        term.force_compress_all();
    }
    let mut metrics = memory_metrics(start, &mut tracker, before);
    metrics.engines = Some(count);
    metrics.payload_bytes = Some(payload.len() as u64);
    // Keep engines alive through the final footprint sample.
    let _ = terms.len();
    measured(metrics, None, None, None)
}

// ---------------------------------------------------------------------------
// Headless idle and cache checks
// ---------------------------------------------------------------------------

fn headless_idle_workload() -> WorkloadOutcome {
    let mut term = match Terminal::new(grid((80, 24))) {
        Ok(term) => term,
        Err(err) => return failed(format!("Terminal::new failed: {err:?}")),
    };
    let mut pool = FramePool::new(4);
    let mut cache = RenderCache::new();
    let burst = payloads::sample_ascii_lines(256 * 1024);
    let mut tracker = FootprintTracker::new();
    let before = crate::alloc::stats();
    let start = Instant::now();

    // Drain the burst into frames until the engine reports Clean.
    let mut redraw_requests = 0u64;
    let mut frames_built = 0u64;
    for chunk in burst.chunks(64 * 1024) {
        term.feed(chunk);
    }
    let mut last_sequence;
    loop {
        let frame = term.build_frame_delta(&mut pool);
        frames_built += 1;
        last_sequence = frame.sequence;
        let damaged = frame.damage != DamageKind::Clean;
        cache.apply_frame(&frame);
        if damaged {
            redraw_requests += 1;
        }
        pool.release(frame);
        if !damaged || frames_built > 500 {
            break;
        }
    }
    let capacities_warm = cache.snapshot_capacities();

    // Strict idle: 100 identical clean frames must request no redraw or
    // animation and must not grow any retained capacity. Reusing the last
    // applied sequence exercises the identical-frame fast path, which is
    // exactly the idle condition after cursor/animation expiry.
    let idle_start = Instant::now();
    let mut idle_redraw = 0u64;
    let mut idle_animation = 0u64;
    for _ in 0..100 {
        let frame = pool.acquire(last_sequence, term.size());
        let action = cache.apply_frame(&frame);
        if action.needs_redraw {
            idle_redraw += 1;
        }
        if action.needs_animation {
            idle_animation += 1;
        }
        pool.release(frame);
    }
    let idle_wall = idle_start.elapsed().as_nanos() as u64;
    let capacities_idle = cache.snapshot_capacities();
    let growth = (capacities_idle.rows + capacities_idle.runs + capacities_idle.backgrounds)
        .saturating_sub(capacities_warm.rows + capacities_warm.runs + capacities_warm.backgrounds);

    let mut metrics = memory_metrics(start, &mut tracker, before);
    metrics.redraw_requests = Some(redraw_requests);
    metrics.frames_built = Some(frames_built + 100);
    metrics.idle_redraw_requests = Some(idle_redraw);
    metrics.idle_animation_requests = Some(idle_animation);
    metrics.capacity_growth_bytes = Some(growth as u64);
    metrics.cache_ok = Some(idle_redraw == 0 && idle_animation == 0 && growth == 0);
    metrics.wall_ns = Some(metrics.wall_ns.unwrap_or(0) + idle_wall);
    measured(metrics, None, None, None)
}

fn headless_cache_workload() -> WorkloadOutcome {
    use mr_crabs_terminal::{Cell, CursorState, RowDelta, Run, Style};

    let mut pool = FramePool::new(2);
    let mut cache = RenderCache::new();
    let mut tracker = FootprintTracker::new();
    let before = crate::alloc::stats();
    let start = Instant::now();

    let mut warm = pool.acquire(7, grid((4, 2)));
    warm.damage = DamageKind::Full;
    warm.cursor = CursorState {
        blinking: false,
        ..CursorState::default()
    };
    warm.styles.push(Style::default());
    warm.rows.push(RowDelta {
        row: 0,
        generation: 1,
        cells: vec![
            Cell {
                content: 'm' as u32,
                style: 0,
                flags: 0,
            },
            Cell {
                content: 'r' as u32,
                style: 0,
                flags: 0,
            },
            Cell::default(),
            Cell::default(),
        ],
        runs: vec![Run {
            start_col: 0,
            len: 4,
            style: 0,
        }],
    });
    let warm_action = cache.apply_frame(&warm);
    let capacities_warm = cache.snapshot_capacities();

    // The idle frame must reuse the warm frame's sequence: a Clean frame
    // with a NEWER sequence means cursor/selection repaint (needs_redraw),
    // which is not an idle condition.
    let mut idle = pool.acquire(7, grid((4, 2)));
    idle.damage = DamageKind::Clean;
    idle.cursor.blinking = false;
    let idle_action = cache.apply_frame(&idle);
    let capacities_idle = cache.snapshot_capacities();
    let growth = (capacities_idle.rows + capacities_idle.runs + capacities_idle.backgrounds)
        .saturating_sub(capacities_warm.rows + capacities_warm.runs + capacities_warm.backgrounds);

    let ok = warm_action.needs_redraw
        && !warm_action.needs_animation
        && !idle_action.needs_redraw
        && !idle_action.needs_animation
        && growth == 0;

    let mut metrics = memory_metrics(start, &mut tracker, before);
    metrics.cache_ok = Some(ok);
    metrics.capacity_growth_bytes = Some(growth as u64);
    metrics.frames_built = Some(2);
    metrics.idle_redraw_requests = Some(u64::from(idle_action.needs_redraw));
    metrics.idle_animation_requests = Some(u64::from(idle_action.needs_animation));
    pool.release(warm);
    pool.release(idle);
    if ok {
        measured(metrics, None, None, None)
    } else {
        failed(
            "headless render-cache check failed (redraw/animation requested on clean frames, or capacity grew)",
        )
    }
}

// ---------------------------------------------------------------------------
// PTY: launch-to-prompt and bounded echo throughput
// ---------------------------------------------------------------------------

const PROMPT_MARKER: &[u8] = b"mr-crabs-bench> ";
const PTY_DEADLINE: Duration = Duration::from_secs(15);

/// Pick a deterministic interactive shell for the prompt workload.
fn prompt_shell() -> &'static str {
    if Path::new("/bin/zsh").is_file() {
        "/bin/zsh"
    } else {
        "/bin/sh"
    }
}

/// True when `pid` is still a live process (POSIX `kill(pid, 0)`).
fn process_alive(pid: i32) -> bool {
    if pid <= 0 {
        return false;
    }
    // SAFETY: signal 0 performs no signal delivery; `pid` is the session's
    // own child, so no other process can be signaled.
    let result = unsafe { libc::kill(pid, 0) };
    if result == 0 {
        return true;
    }
    std::io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH)
}

fn pty_launch_to_prompt_workload() -> WorkloadOutcome {
    let shell = prompt_shell();
    let mut cmd = CommandBuilder::new(shell);
    cmd.args(["-f", "-i"])
        .clear_envs(true)
        .env("PS1", "mr-crabs-bench> ");
    let size = match PtySize::new(80, 24, 8, 16) {
        Ok(size) => size,
        Err(err) => return failed(format!("PtySize::new failed: {err:?}")),
    };
    let config = PtyConfig::new(cmd, size);
    let (mut session, rx, _exit_rx) = match PtySession::spawn(config) {
        Ok(spawned) => spawned,
        Err(err) => return failed(format!("PtySession::spawn failed: {err}")),
    };
    let pid = session.child_pid();
    let mut tracker = FootprintTracker::new();
    let before = crate::alloc::stats();
    let start = Instant::now();
    let deadline = Instant::now() + PTY_DEADLINE;

    let mut window: VecDeque<u8> = VecDeque::with_capacity(PROMPT_MARKER.len());
    let mut bytes_seen = 0u64;
    let mut matched = false;
    while Instant::now() < deadline && !matched {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(chunk) => {
                for byte in chunk {
                    bytes_seen += 1;
                    window.push_back(byte);
                    if window.len() > PROMPT_MARKER.len() {
                        window.pop_front();
                    }
                    if window.len() == PROMPT_MARKER.len()
                        && window.iter().copied().eq(PROMPT_MARKER.iter().copied())
                    {
                        matched = true;
                        break;
                    }
                }
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
        tracker.tick();
    }

    let launch_ns = start.elapsed().as_nanos() as u64;
    let status = session.shutdown_and_reap(Duration::from_secs(2));
    let reaped = status.is_ok();
    let alive_after = process_alive(pid);
    if !matched {
        return failed(format!(
            "shell prompt marker not observed within {}s (shell={shell}, bytes_seen={bytes_seen})",
            PTY_DEADLINE.as_secs()
        ));
    }
    let mut metrics = memory_metrics(start, &mut tracker, before);
    metrics.launch_to_prompt_ns = Some(launch_ns);
    metrics.prompt_bytes = Some(bytes_seen);
    metrics.child_reaped = Some(reaped);
    metrics.child_alive_after_reap = Some(alive_after);
    metrics.exit_code = status.ok().and_then(|s| s.code());
    let spec = json!({
        "product": mr_crabs_config::PRODUCT_NAME,
            "shell": shell,
        "marker": String::from_utf8_lossy(PROMPT_MARKER),
        "pty_size": [80, 24],
    });
    measured(metrics, None, None, Some(spec))
}

const ECHO_BYTES: usize = 4 * 1024 * 1024;
const ECHO_CHUNK: usize = 4096;
const ECHO_READY: &[u8] = b"mr-crabs-echo-ready";

fn pty_echo_workload() -> WorkloadOutcome {
    // Raw mode, echo disabled: the PTY line discipline cannot double the
    // stream, so the child's output must equal the input byte-for-byte.
    let mut cmd = CommandBuilder::new("/bin/sh");
    cmd.args([
        "-c",
        "/bin/stty raw -echo; printf mr-crabs-echo-ready; exec /bin/cat",
    ]);
    let size = match PtySize::new(80, 24, 8, 16) {
        Ok(size) => size,
        Err(err) => return failed(format!("PtySize::new failed: {err:?}")),
    };
    let config = PtyConfig::new(cmd, size)
        .with_writer_capacity(64)
        .with_reader_capacity(64);
    let (mut session, rx, _exit_rx) = match PtySession::spawn(config) {
        Ok(spawned) => spawned,
        Err(err) => return failed(format!("PtySession::spawn failed: {err}")),
    };
    let pid = session.child_pid();
    let payload = payloads::sample_ascii_lines(ECHO_BYTES);
    let ready_deadline = Instant::now() + PTY_DEADLINE;
    let mut ready = Vec::with_capacity(ECHO_READY.len());
    while ready.len() < ECHO_READY.len() && Instant::now() < ready_deadline {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(chunk) => ready.extend_from_slice(&chunk),
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    if !ready.ends_with(ECHO_READY) {
        return failed("PTY echo child did not enter raw mode before deadline");
    }

    let mut tracker = FootprintTracker::new();
    let before = crate::alloc::stats();
    let start = Instant::now();
    let deadline = Instant::now() + PTY_DEADLINE;

    let mut verified = true;
    let mut write_pos = 0usize;
    let mut read_pos = 0usize;
    while read_pos < ECHO_BYTES && Instant::now() < deadline {
        if write_pos < ECHO_BYTES {
            // Write the next chunk (bounded queue; blocks under backpressure).
            let end = (write_pos + ECHO_CHUNK).min(ECHO_BYTES);
            if let Err(err) = session.write(payload[write_pos..end].to_vec()) {
                return failed(format!("pty write failed: {err}"));
            }
            write_pos = end;
        }
        // Drain whatever is available now, keeping the pipeline flowing.
        loop {
            match rx.try_recv() {
                Ok(chunk) => {
                    if read_pos + chunk.len() > ECHO_BYTES
                        || chunk.as_slice() != &payload[read_pos..read_pos + chunk.len()]
                    {
                        verified = false;
                    }
                    read_pos += chunk.len();
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => break,
            }
        }
        tracker.tick();
    }
    // Final drain after all writes have been accepted.
    while read_pos < ECHO_BYTES && Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(200)) {
            Ok(chunk) => {
                if read_pos + chunk.len() > ECHO_BYTES
                    || chunk.as_slice() != &payload[read_pos..read_pos + chunk.len()]
                {
                    verified = false;
                }
                read_pos += chunk.len();
            }
            Err(RecvTimeoutError::Timeout) => {}
            Err(RecvTimeoutError::Disconnected) => break,
        }
    }
    let wall = start.elapsed().as_nanos() as u64;
    let status = session.shutdown_and_reap(Duration::from_secs(2));
    let reaped = status.is_ok();
    let alive_after = process_alive(pid);

    if read_pos != ECHO_BYTES {
        return failed(format!(
            "echo incomplete: received {read_pos} of {ECHO_BYTES} bytes before deadline"
        ));
    }
    if !verified {
        return failed("echo payload mismatch: PTY output differs from input");
    }
    let mut metrics = memory_metrics(start, &mut tracker, before);
    metrics.echo_bytes = Some(ECHO_BYTES as u64);
    metrics.wall_ns = Some(wall);
    metrics.echo_mib_s = Some(ECHO_BYTES as f64 / (wall as f64 / 1e9) / 1048576.0);
    metrics.child_reaped = Some(reaped);
    metrics.child_alive_after_reap = Some(alive_after);
    metrics.exit_code = status.ok().and_then(|s| s.code());
    let spec = json!({
        "bytes": ECHO_BYTES,
        "chunk_bytes": ECHO_CHUNK,
        "child": "/bin/sh -c \"/bin/stty raw -echo; printf mr-crabs-echo-ready; exec /bin/cat\"",
    });
    measured(metrics, None, None, Some(spec))
}

// ---------------------------------------------------------------------------
// Kitty image decode stress (S7 graphics crate)
// ---------------------------------------------------------------------------

/// Raw PNG fixture from the S7 graphics corpus (50x76).
const PNG_FIXTURE: &[u8] = include_bytes!(
    "../../../verification/graphics-corpus/fixtures/image-png-none-50x76-2147483647-raw.data"
);
/// Zlib-compressed RGB fixture from the S7 graphics corpus (128x96).
const ZLIB_FIXTURE: &[u8] = include_bytes!(
    "../../../verification/graphics-corpus/fixtures/image-rgb-zlib_deflate-128x96-2147483647-raw.data"
);
const IMAGE_ITERATIONS: u64 = 2048;

fn image_decode_stress_workload() -> WorkloadOutcome {
    use mr_crabs_graphics::image::{MAX_DIMENSION, MAX_SIZE, decode_png_to_rgba, zlib_decompress};

    // Warm decode + fixture parity check (50x76 per the corpus manifest).
    let first = match decode_png_to_rgba(PNG_FIXTURE, MAX_SIZE, MAX_DIMENSION) {
        Ok(image) => image,
        Err(err) => return failed(format!("png fixture decode failed: {err}")),
    };
    if first.width != 50 || first.height != 76 {
        return failed(format!(
            "png fixture dimension mismatch: {}x{} (expected 50x76)",
            first.width, first.height
        ));
    }

    let mut tracker = FootprintTracker::new();
    let before = crate::alloc::stats();
    let start = Instant::now();
    let mut decoded_bytes = first.rgba.len() as u64;
    for i in 0..IMAGE_ITERATIONS {
        match decode_png_to_rgba(PNG_FIXTURE, MAX_SIZE, MAX_DIMENSION) {
            Ok(image) => decoded_bytes += image.rgba.len() as u64,
            Err(err) => return failed(format!("png decode failed at iteration {i}: {err}")),
        }
        if i % 32 == 0 {
            tracker.tick();
        }
    }
    // Zlib parity check: 128x96 RGB = 36,864 raw bytes.
    let inflated = match zlib_decompress(ZLIB_FIXTURE, MAX_SIZE) {
        Ok(bytes) => bytes,
        Err(err) => return failed(format!("zlib fixture decompress failed: {err}")),
    };
    if inflated.len() != 128 * 96 * 3 {
        return failed(format!(
            "zlib fixture length mismatch: {} (expected 36864)",
            inflated.len()
        ));
    }
    decoded_bytes += inflated.len() as u64;
    let mut metrics = memory_metrics(start, &mut tracker, before);
    metrics.images_decoded = Some(IMAGE_ITERATIONS + 1);
    metrics.decoded_bytes = Some(decoded_bytes);
    let wall_s = metrics.wall_ns.unwrap_or(1) as f64 / 1e9;
    metrics.decode_mib_s = Some(decoded_bytes as f64 / wall_s / 1048576.0);
    let spec = json!({
        "png_fixture": "image-png-none-50x76-2147483647-raw.data",
        "zlib_fixture": "image-rgb-zlib_deflate-128x96-2147483647-raw.data",
        "iterations": IMAGE_ITERATIONS,
    });
    measured(metrics, None, None, Some(spec))
}

// ---------------------------------------------------------------------------
// Effects workload (S9 mr-crabs-effects, real measured workload)
// ---------------------------------------------------------------------------
//
// Headless and deterministic: a terminal burst produces damaged frames; an
// `EffectsModel` (explicit opt-in config — streaming 120ms, cursor trail on
// 250ms @ 0.35, the former default; the product default is plain) processes
// each frame under an explicit monotonic clock. The
// workload then advances the clock past every transition and verifies strict
// idle afterwards: zero frames after expiry and zero retained capacity when
// effects are disabled (the disabled path must allocate nothing).

const EFFECTS_CHUNKS: u64 = 64;
const EFFECTS_CHUNK_BYTES: usize = 4096;
const EFFECTS_SEED: u64 = 0xEFF5;
const EFFECTS_NOW_STEP_MS: u64 = 16;
const EFFECTS_IDLE_FRAMES: u64 = 100;

fn effects_workload() -> WorkloadOutcome {
    use mr_crabs_config::TextAnimation;
    use mr_crabs_effects::{CellPx, EffectsConfig, EffectsModel};

    let size = grid((80, 24));
    let cell = CellPx::new(8.0, 16.0);
    // Product defaults are plain (text none + trail off); this workload
    // measures the active-effects cycle, so pin the explicit opt-in config
    // that matches the former default (streaming 120ms/1.0, trail on
    // 250ms/0.35, max_tracked_cells 1<<20).
    let config = EffectsConfig::new(TextAnimation::Streaming, 120, 1.0, true, 0.35, 250, 1 << 20);
    let mut model = EffectsModel::new(config, size, cell);
    let mut term = match Terminal::new(size) {
        Ok(term) => term,
        Err(err) => return failed(format!("Terminal::new failed: {err:?}")),
    };
    let mut pool = FramePool::new(4);

    let mut tracker = FootprintTracker::new();
    let before = crate::alloc::stats();
    let start = Instant::now();
    let mut apply_times: Vec<u64> = Vec::with_capacity(EFFECTS_CHUNKS as usize);
    let mut now_ms: u64 = 0;
    let mut active_frames = 0u64;
    for step in 0..EFFECTS_CHUNKS {
        let chunk = payloads::seeded_chunk(EFFECTS_SEED, step, EFFECTS_CHUNK_BYTES);
        term.feed(&chunk);
        let frame = term.build_frame_delta(&mut pool);
        let frame_start = Instant::now();
        let fx = model.apply_frame(&frame, now_ms, true);
        apply_times.push(frame_start.elapsed().as_nanos() as u64);
        if fx.needs_frame {
            active_frames += 1;
        }
        pool.release(frame);
        now_ms += EFFECTS_NOW_STEP_MS;
        tracker.tick();
    }

    // Advance the clock until every transition expires (text reveal 120ms,
    // trail fade 250ms), then run 100 further frames that must request
    // nothing (strict effects idle after expiry).
    let mut frames_until_idle = 0u64;
    while model.needs_frame() && frames_until_idle < 10_000 {
        let frame = pool.acquire(term.next_sequence(), term.size());
        let fx = model.apply_frame(&frame, now_ms, true);
        frames_until_idle += 1;
        let still_active = fx.needs_frame;
        pool.release(frame);
        now_ms += EFFECTS_NOW_STEP_MS;
        if !still_active {
            break;
        }
    }
    let mut frames_after_expiry = 0u64;
    for _ in 0..EFFECTS_IDLE_FRAMES {
        let frame = pool.acquire(term.next_sequence(), term.size());
        let fx = model.apply_frame(&frame, now_ms, true);
        if fx.needs_frame {
            frames_after_expiry += 1;
        }
        pool.release(frame);
        now_ms += EFFECTS_NOW_STEP_MS;
    }
    let retained = model.retained_capacity();
    drop(model);

    // Disabled config must retain exactly zero heap bytes.
    let disabled = EffectsModel::new(
        EffectsConfig::new(
            TextAnimation::Disabled,
            120,
            1.0,
            false,
            0.35,
            250,
            usize::MAX,
        ),
        size,
        cell,
    );
    let disabled_retained = disabled.retained_capacity();
    drop(disabled);

    let mut metrics = memory_metrics(start, &mut tracker, before);
    metrics.effects_frames = Some(EFFECTS_CHUNKS);
    metrics.redraw_requests = Some(active_frames);
    metrics.frames_until_idle = Some(frames_until_idle);
    metrics.frames_after_expiry = Some(frames_after_expiry);
    metrics.idle_animation_requests = Some(frames_after_expiry);
    metrics.effects_retained_capacity = Some(retained as u64);
    metrics.effects_disabled_retained_capacity = Some(disabled_retained as u64);
    apply_times.sort_unstable();
    let sum: u128 = apply_times.iter().map(|&t| u128::from(t)).sum();
    metrics.frame_build_mean_ns = Some((sum / apply_times.len() as u128) as u64);
    metrics.frame_build_p50_ns = stats::percentile(&apply_times, 50.0);
    metrics.frame_build_p95_ns = stats::percentile(&apply_times, 95.0);
    metrics.frame_build_p99_ns = stats::percentile(&apply_times, 99.0);
    let spec = json!({
        "chunks": EFFECTS_CHUNKS,
        "chunk_bytes": EFFECTS_CHUNK_BYTES,
        "seed": EFFECTS_SEED,
        "now_step_ms": EFFECTS_NOW_STEP_MS,
        "idle_frames": EFFECTS_IDLE_FRAMES,
        "config": "explicit opt-in (former default): streaming 120ms intensity 1.0; cursor trail on 250ms opacity 0.35; max_tracked_cells 1<<20",
    });
    measured(metrics, None, None, Some(spec))
}

// ---------------------------------------------------------------------------
// Search workload (S8 mr-crabs-history, real measured workload)
// ---------------------------------------------------------------------------
//
// Deterministic: 200,000 history lines with a needle every 997th line, then
// a synchronous search over the full history plus a worker-thread round
// trip with cancellation on the same `Terminal` (which implements
// `HistoryRead`). Expected match count is pinned (201); line loss or a
// wrong match count fails the gate.

fn search_workload() -> WorkloadOutcome {
    use mr_crabs_history::search::{
        SearchDirection, SearchRequest, SearchStart, SearchWorker, search_sync,
    };
    use mr_crabs_terminal::HistoryRead;
    use std::sync::{Arc, Mutex};

    let payload =
        payloads::search_history_payload(payloads::SEARCH_LINES, payloads::SEARCH_NEEDLE_EVERY);
    let mut term = match Terminal::new(grid((80, 24))) {
        Ok(term) => term,
        Err(err) => return failed(format!("Terminal::new failed: {err:?}")),
    };
    let mut config = term.scrollback_config();
    config.max_lines = payloads::SEARCH_LINES + usize::from(term.size().rows);
    term.set_scrollback_config(config);
    term.feed(&payload);
    term.drain_compression();
    term.force_compress_all();

    // The newest records still sit on the visible screen (23 records plus
    // the blank cursor row), so they are not in paged history. Search
    // coverage is history + visible rows: pass the visible records or the
    // scan undercounts by the still-visible rows.
    let visible: Vec<Vec<Cell>> = term
        .visible_rows()
        .into_iter()
        .filter(|row| !row_is_empty(row))
        .collect();

    let mut tracker = FootprintTracker::new();
    let before = crate::alloc::stats();
    let start = Instant::now();

    // Deterministic synchronous search over the whole history.
    let outcome = search_sync(
        &mut term,
        &SearchRequest {
            needle: payloads::SEARCH_NEEDLE.to_vec(),
            direction: SearchDirection::Forward,
            start: SearchStart::Top,
            limit: mr_crabs_history::search::MAX_SEARCH_LIMIT,
            case_sensitive: false,
            visible_rows: visible.clone(),
        },
        1,
    );
    let sync_matches = outcome.matches.len() as u64;
    let lines_searched = outcome.lines_searched as u64;
    if outcome.truncated {
        return failed("search outcome truncated below expected matches");
    }
    if !outcome.completed {
        return failed("search did not complete the full history range");
    }
    if sync_matches != payloads::SEARCH_EXPECTED_MATCHES as u64 {
        return failed(format!(
            "search match count {} != expected {}",
            sync_matches,
            payloads::SEARCH_EXPECTED_MATCHES
        ));
    }
    if lines_searched != payloads::SEARCH_LINES as u64 {
        return failed(format!(
            "search scanned {lines_searched} lines != {} (line loss in search coverage)",
            payloads::SEARCH_LINES
        ));
    }

    // Worker-thread round trip over the same history.
    let reader: Arc<Mutex<dyn HistoryRead + Send>> = Arc::new(Mutex::new(term));
    let worker = SearchWorker::new(reader);
    let worker_start = Instant::now();
    let token = worker.start(SearchRequest {
        needle: payloads::SEARCH_NEEDLE.to_vec(),
        direction: SearchDirection::Forward,
        start: SearchStart::Top,
        limit: mr_crabs_history::search::MAX_SEARCH_LIMIT,
        case_sensitive: false,
        visible_rows: visible,
    });
    let worker_outcome = match worker.poll_wait(Duration::from_secs(15)) {
        Some(outcome) => outcome,
        None => return failed("search worker timed out"),
    };
    let worker_round_trip_ns = worker_start.elapsed().as_nanos() as u64;
    let worker_matches = worker_outcome.matches.len() as u64;
    let token_ok = token != 0 && !worker_outcome.is_stale(worker.generation());
    if !token_ok || worker_matches != payloads::SEARCH_EXPECTED_MATCHES as u64 {
        return failed(format!(
            "worker search mismatch: token_ok={token_ok}, matches={worker_matches}, expected={}",
            payloads::SEARCH_EXPECTED_MATCHES
        ));
    }

    // Cancellation: replace with a fresh search and cancel immediately.
    // (Measured, not gated: whether the worker observes the flag before
    // finishing is a scheduling race by design of the bounded worker.)
    let _ = worker.start(SearchRequest {
        needle: payloads::SEARCH_NEEDLE.to_vec(),
        ..SearchRequest::default()
    });
    worker.cancel();
    let cancelled = worker
        .poll_wait(Duration::from_secs(15))
        .is_some_and(|outcome| outcome.cancelled);
    drop(worker); // joins the worker thread

    let mut metrics = memory_metrics(start, &mut tracker, before);
    metrics.search_matches = Some(sync_matches);
    metrics.search_lines_scanned = Some(lines_searched);
    metrics.worker_matches = Some(worker_matches);
    metrics.worker_round_trip_ns = Some(worker_round_trip_ns);
    metrics.worker_cancelled = Some(cancelled);
    let spec = json!({
        "lines": payloads::SEARCH_LINES,
        "needle": String::from_utf8_lossy(payloads::SEARCH_NEEDLE),
        "needle_every": payloads::SEARCH_NEEDLE_EVERY,
        "expected_matches": payloads::SEARCH_EXPECTED_MATCHES,
    });
    measured(metrics, None, None, Some(spec))
}

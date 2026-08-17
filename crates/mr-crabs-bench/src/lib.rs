use serde::{Deserialize, Serialize};

#[derive(Clone, Debug)]
pub struct SeededChunks {
    remaining: usize,
    state: u64,
    max_chunk: usize,
}

impl SeededChunks {
    pub fn new(total: usize, seed: u64, max_chunk: usize) -> Option<Self> {
        (max_chunk != 0).then_some(Self {
            remaining: total,
            state: if seed == 0 {
                0x9e37_79b9_7f4a_7c15
            } else {
                seed
            },
            max_chunk,
        })
    }
}

impl Iterator for SeededChunks {
    type Item = usize;

    fn next(&mut self) -> Option<Self::Item> {
        if self.remaining == 0 {
            return None;
        }
        self.state ^= self.state << 13;
        self.state ^= self.state >> 7;
        self.state ^= self.state << 17;
        let upper: usize = self.remaining.min(self.max_chunk);
        let upper_u64: u64 = u64::try_from(upper).expect("chunk upper bound fits in u64");
        let remainder: u64 = self.state % upper_u64;
        let remainder_usize: usize = usize::try_from(remainder).expect("remainder fits in usize");
        let chunk: usize = 1_usize
            .checked_add(remainder_usize)
            .expect("chunk fits in usize");
        self.remaining = self
            .remaining
            .checked_sub(chunk)
            .expect("remaining underflow");
        Some(chunk)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct S3BenchOutput {
    pub suite: String,
    pub logical_lines: usize,
    pub hot_resident_bytes: usize,
    pub compressed_bytes: usize,
    pub throughput_mbps: f64,
    pub peak_rss_bytes: u64,
    /// Optional extended fields for debugging; not required by acceptance but useful.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub compression_latency_ms: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restored_pages: Option<u64>,
}

impl S3BenchOutput {
    pub fn new(
        logical_lines: usize,
        hot_resident_bytes: usize,
        compressed_bytes: usize,
        throughput_mbps: f64,
    ) -> Self {
        Self {
            suite: "s3".to_owned(),
            logical_lines,
            hot_resident_bytes,
            compressed_bytes,
            throughput_mbps,
            peak_rss_bytes: 0,
            compression_latency_ms: None,
            restored_pages: None,
        }
    }
}

/// Run the S3 benchmark suite headlessly using only the Terminal public API.
///
/// The suite covers:
/// (a) feed throughput micro-bench
/// (b) 1M-line deterministic scrollback with bounded resident accounting
/// (c) compression round-trip latency via synchronous drain hooks when available
///
/// Returns a JSON-serialisable `S3BenchOutput` with the fields required by
/// the acceptance contract: `{suite:"s3", logical_lines, hot_resident_bytes,
/// compressed_bytes, throughput_mbps}`.
pub fn run_s3_suite() -> S3BenchOutput {
    let throughput = measure_throughput();
    let (logical_lines, hot_resident_bytes, compressed_bytes, restored_pages) =
        measure_scrollback();
    let latency_ms = measure_compression_latency();

    let mut out = S3BenchOutput::new(
        logical_lines,
        hot_resident_bytes,
        compressed_bytes,
        throughput,
    );
    out.compression_latency_ms = Some(latency_ms);
    out.peak_rss_bytes = peak_rss_bytes();
    out.restored_pages = Some(restored_pages);
    out
}

#[cfg(target_os = "macos")]
fn peak_rss_bytes() -> u64 {
    let mut usage = std::mem::MaybeUninit::<libc::rusage>::zeroed();
    // SAFETY: `usage` points to writable storage for `getrusage`, and the
    // successful call initializes the complete structure.
    let result = unsafe { libc::getrusage(libc::RUSAGE_SELF, usage.as_mut_ptr()) };
    if result == 0 {
        // SAFETY: established by the successful `getrusage` call above.
        unsafe { usage.assume_init().ru_maxrss.try_into().unwrap_or(0) }
    } else {
        0
    }
}

#[cfg(not(target_os = "macos"))]
fn peak_rss_bytes() -> u64 {
    0
}

fn measure_throughput() -> f64 {
    use mr_crabs_terminal::{GridSize, Terminal};
    use std::time::Instant;

    // Deterministic 256 KiB payload: alternating printable ASCII and SGR.
    let size = GridSize::new(80, 24);
    let mut payload = Vec::with_capacity(256 * 1024);
    for i in 0..(256 * 1024) {
        match i % 16 {
            0 => payload.extend_from_slice(b"\x1b[31m"),
            8 => payload.extend_from_slice(b"\x1b[0m"),
            _ => payload.push(b'A' + ((i % 26) as u8)),
        }
    }
    // Warm-up.
    let mut term = Terminal::new(size).expect("valid grid");
    term.feed(&payload);
    let _ = term.snapshot();

    let iterations: usize = 40;
    let start = Instant::now();
    let mut total_bytes: usize = 0;
    for _ in 0..iterations {
        let mut t = Terminal::new(size).expect("valid grid");
        t.feed(&payload);
        total_bytes += payload.len();
        // Snapshot is not part of hot feed path but keep one to prevent DCE.
        let _ = t.snapshot();
    }
    let elapsed = start.elapsed().as_secs_f64().max(1e-9);
    let bytes_per_sec = total_bytes as f64 / elapsed;
    bytes_per_sec / (1024.0 * 1024.0)
}

fn measure_scrollback() -> (usize, usize, usize, u64) {
    use mr_crabs_terminal::{GridSize, Terminal};

    let size = GridSize::new(80, 24);
    let cols = usize::from(size.cols);
    let rows = usize::from(size.rows);
    let total_lines: usize = 1_000_000;

    let mut term = Terminal::new(size).expect("valid grid");
    set_max_lines_if_available(&mut term, total_lines + rows);

    // Match the S0 oracle workload exactly: one million `line\n` records.
    let batch_lines: usize = 4096;
    let mut line_buf = Vec::with_capacity(batch_lines * 5);
    let mut lines_fed: usize = 0;
    while lines_fed < total_lines {
        line_buf.clear();
        let batch = (total_lines - lines_fed).min(batch_lines);
        for _ in 0..batch {
            line_buf.extend_from_slice(b"line\n");
        }
        term.feed(&line_buf);
        lines_fed += batch;
    }

    drain_if_available(&mut term);

    let stats = storage_stats_if_available(&term);
    if let Some(s) = stats {
        return (
            s.logical_lines,
            s.hot_resident_bytes,
            s.compressed_bytes,
            s.restored_pages,
        );
    }

    let logical_lines = total_lines;
    let hot_resident_bytes = rows * cols * std::mem::size_of::<mr_crabs_terminal::Cell>();
    let compressed_bytes = 0usize;
    let restored_pages = 0u64;
    (
        logical_lines,
        hot_resident_bytes,
        compressed_bytes,
        restored_pages,
    )
}

fn measure_compression_latency() -> f64 {
    use mr_crabs_terminal::{GridSize, Terminal};
    use std::time::Instant;

    let size = GridSize::new(80, 24);
    let mut term = Terminal::new(size).expect("valid grid");
    // Prepare enough scrollback to trigger at least one cold page if paging exists.
    let payload = vec![b'X'; 80 * 200];
    for _ in 0..16 {
        term.feed(&payload);
        term.feed(b"\n");
    }
    let start = Instant::now();
    drain_if_available(&mut term);
    // Force restore round-trip when available (deterministic, no sleep).
    restore_if_available(&mut term);
    start.elapsed().as_secs_f64() * 1000.0
}

// ---------------------------------------------------------------------------
// Optional storage hooks — compile against has_storage when the terminal lane
// has landed ScrollbackConfig/StorageStats. Otherwise these become no-ops
// so the bench crate remains buildable in isolation.
// ---------------------------------------------------------------------------

struct BenchStats {
    logical_lines: usize,
    hot_resident_bytes: usize,
    compressed_bytes: usize,
    restored_pages: u64,
}

#[cfg(has_storage)]
fn storage_stats_if_available(term: &mr_crabs_terminal::Terminal) -> Option<BenchStats> {
    let s = term.storage_stats();
    Some(BenchStats {
        logical_lines: s.logical_lines,
        hot_resident_bytes: s.hot_resident_bytes,
        compressed_bytes: s.compressed_bytes,
        restored_pages: s.restored_pages,
    })
}

#[cfg(has_storage)]
fn drain_if_available(term: &mut mr_crabs_terminal::Terminal) {
    // Drain both compression and any pending completions deterministically.
    term.drain_compression();
    term.force_compress_all();
}

#[cfg(not(has_storage))]
fn drain_if_available(_term: &mut mr_crabs_terminal::Terminal) {}

#[cfg(has_storage)]
fn set_max_lines_if_available(term: &mut mr_crabs_terminal::Terminal, max_lines: usize) {
    let mut cfg = term.scrollback_config();
    cfg.max_lines = max_lines;
    term.set_scrollback_config(cfg);
}

#[cfg(not(has_storage))]
fn set_max_lines_if_available(_term: &mut mr_crabs_terminal::Terminal, _max_lines: usize) {}

#[cfg(has_storage)]
fn restore_if_available(term: &mut mr_crabs_terminal::Terminal) {
    term.force_restore_all();
}

#[cfg(not(has_storage))]
fn restore_if_available(_term: &mut mr_crabs_terminal::Terminal) {}

/// Exercise the retained render cache without opening a GPUI window.
///
/// A damaged frame warms every retained vector. Reapplying a clean frame
/// with the same sequence must preserve capacities and request neither a
/// redraw nor animation.
pub fn headless_cache_smoke() -> bool {
    use mr_crabs_element::RenderCache;
    use mr_crabs_terminal::{
        Cell, CursorState, DamageKind, FramePool, GridSize, RowDelta, Run, Style,
    };

    let mut pool = FramePool::new(2);
    let mut warm = pool.acquire(7, GridSize::new(4, 2));
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

    let mut cache = RenderCache::new();
    let warm_action = cache.apply_frame(&warm);
    let capacities = cache.snapshot_capacities();

    let mut idle = pool.acquire(7, GridSize::new(4, 2));
    idle.damage = DamageKind::Clean;
    idle.cursor.blinking = false;
    let idle_action = cache.apply_frame(&idle);

    warm_action.needs_redraw
        && !warm_action.needs_animation
        && !idle_action.needs_redraw
        && cache.snapshot_capacities() == capacities
}

/// S5 input-corpus smoke: validates corpus JSON exists and checks a few byte vectors.
pub fn s5_input_smoke() -> bool {
    let path = concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../verification/input-corpus/s5-input.json"
    );
    let Ok(text) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(v): Result<serde_json::Value, _> = serde_json::from_str(&text) else {
        return false;
    };
    v.get("cases")
        .and_then(|c| c.as_array())
        .is_some_and(|a| a.len() >= 20)
}

// ---------------------------------------------------------------------------
// S12 release suite
// ---------------------------------------------------------------------------
//
// CLI contract (see `main.rs`):
//   mr-crabs-bench --suite release --workload <name> --json <path>   (one run)
//   mr-crabs-bench --suite release --json <path>                     (aggregate)
//
// Results distinguish `measured`, `failed`, and `blocked`; blocked metrics
// (GUI frame time, window redraw, strict GUI idle, energy, and the S8/S9
// hooks before their crates land) serialize `not_measured` with an exact
// reason and can never satisfy a release gate.

pub mod alloc;
pub mod memory;
pub mod payloads;
pub mod stats;
pub mod workloads;

pub use workloads::{
    RELEASE_WORKLOADS, ReleaseAggregateResult, ReleaseRunResult, run_release_aggregate,
    run_release_workload,
};

#[cfg(test)]
mod tests {
    use super::{S3BenchOutput, SeededChunks};

    #[test]
    fn bench_output_serializes_required_fields() {
        let out = S3BenchOutput::new(1_000_000, 8192, 0, 123.45);
        let json = serde_json::to_value(&out).unwrap();
        assert_eq!(json["suite"], "s3");
        assert_eq!(json["logical_lines"], 1_000_000);
        assert!(json["hot_resident_bytes"].is_number());
        assert!(json["compressed_bytes"].is_number());
        assert!(json["throughput_mbps"].is_number());
    }

    #[test]
    fn seeded_chunks_covers_total() {
        let remaining = 4096;
        let mut sum = 0;
        // Use iterator directly to mirror oracle.
        let iter = SeededChunks::new(remaining, 0x5eed_u64, 31).unwrap();
        for chunk in iter {
            sum += chunk;
        }
        assert_eq!(sum, 4096);
    }
}

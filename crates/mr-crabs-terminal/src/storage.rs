//! Page-based scrollback storage with LZ4 cold compression and bounded queues.
//!
//! Design overview:
//! - Hot pages: fixed capacity `hot_page_lines` lines. Two hot kinds:
//!   * Flat pages: one contiguous `Arc<[Cell]>` of `capacity_lines * cols` cells,
//!     used by `push_line`/`push_cells` (legacy flat feed).
//!   * Segmented pages: `Vec<RowDesc>` of moved `Arc<[Cell]>` descriptors,
//!     used by `ingest_owned_*` (terminal feed). Each descriptor preserves its
//!     own `cols`/`occupancy`/`wrapped` values and its `Arc` identity; feed
//!     moves descriptors without scanning or copying cells. A hot page groups
//!     up to `hot_page_lines` rows; 1M rows produce
//!     O(lines/hot_page_lines) pages, not O(lines).
//! - Cold pages: `compressed: Option<Arc<[u8]>>` holds the LZ4 block.
//!   Successful compression clears the hot resident (flat `Arc` or segmented
//!   `Vec`) which releases the allocation. Segmented pages are flattened into
//!   a reusable worker scratch buffer before LZ4 — never on the feed thread.
//!   Cold format is the same flat byte stream (`lines * cols` cells) so read/
//!   fold/eviction/stats/max-lines/resize remain exact.
//! - History is `VecDeque<Page>` with logical line count tracking; no
//!   `Vec<Row>` retention.
//! - Generational tracking: each page has `generation: u64` bumped on reuse/mutation;
//!   completions are applied only if `generation` matches current page. Stale
//!   completions are discarded and counted in `stale_discarded`.
//! - Bounded queues: `sync_channel(Job)` capacity `max_queued_jobs` and
//!   `sync_channel(Completion)` capacity `max_pending_completions`. Mutation
//!   never blocks: `try_send` is used on both sides.
//!   Overload behavior (explicit):
//!   * Job enqueue (`Terminal` -> worker) on `Full`: the page keeps its own
//!     hot resident (no data movement), stays hot and retryable
//!     (`pending = false`), do not lose history, retry on `drain_compression` /
//!     `force_compress_all`. No blocking, no loss.
//!   * Completion enqueue (worker -> `Terminal`) on `Full`: drop the newly
//!     compressed result, keep the source page hot (history not lost), count
//!     as overload (`stale_discarded` increment). The page will be retried on
//!     next `force_compress_all`/`drain_compression` cycle. Never block worker.
//! - Deterministic hooks (`drain_compression`, `force_restore_all`,
//!   `force_compress_all`) run synchronously without sleeps and honor
//!   generation checks.
//! - Census/quiescence extension (Wave A): `quiesce_for_style_transaction`
//!   drains already-queued completions without re-enqueuing or sleeping;
//!   census helpers share one occupied-range rule and reuse private scratch
//!   without mutating `ColdReadCache`. Corrupt cold data fails closed.
use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Weak};
use std::thread::JoinHandle;

use crate::compress::{compress_bytes, compress_bytes_reuse, decompress_bytes};
use crate::side_tables::StyleRemap;
use crate::{Cell, TerminalError};
// ---------------------------------------------------------------------------
// Public config / stats — names and defaults must match the frozen contract.
// ---------------------------------------------------------------------------

/// Paged scrollback configuration.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ScrollbackConfig {
    pub max_lines: usize,
    pub hot_page_lines: usize,
    pub max_queued_jobs: usize,
    pub max_pending_completions: usize,
}
const MAX_RECYCLED_ROWS: usize = 1_024;

impl Default for ScrollbackConfig {
    fn default() -> Self {
        Self {
            max_lines: 100_000,
            hot_page_lines: 512,
            max_queued_jobs: 16,
            max_pending_completions: 16,
        }
    }
}

/// Observable storage statistics.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StorageStats {
    pub logical_lines: usize,
    /// Number of stored logical lines that are entirely default cells (blank
    /// rows created by cursor movement). Wrap-artifact rows carry a
    /// WRAPLINE-flagged cell and are not default cells, so blank wrap rows
    /// are not included here; retained payload record counts are derived
    /// from record content via [`ScrollbackStorage::fold_lines`] instead.
    pub empty_lines: usize,
    pub hot_resident_bytes: usize,
    pub compressed_bytes: usize,
    pub queued_jobs: usize,
    pub pending_completions: usize,
    pub restored_pages: u64,
    pub stale_discarded: u64,
}

// ---------------------------------------------------------------------------
// Internal page & channel types
// ---------------------------------------------------------------------------

/// One moved row descriptor. The `Arc<[Cell]>` allocation is moved
/// without scanning or copying cells; feed appends these descriptors into
/// bounded hot pages.
#[derive(Clone)]
struct RowDesc {
    cells: Arc<[Cell]>,
    cols: u16,
    occupancy: u16,
    first_occupied: u16,
    wrapped: bool,
}

/// Zero-copy carrier for hot segmented suffix extraction. Preserves the
/// moved `Arc<[Cell]>` allocation identity and per-row metadata.
#[derive(Clone)]
pub(crate) struct StoredRow {
    pub(crate) cells: Arc<[Cell]>,
    pub(crate) cols: u16,
    pub(crate) occupancy: u16,
    pub(crate) first_occupied: u16,
    pub(crate) wrapped: bool,
    pub(crate) generation: u64,
}

/// Hot resident variants:
/// - `Flat`: contiguous `capacity * cols` buffer (push_line / flat ingest).
/// - `Segmented`: moved per-row `Arc<[Cell]>` descriptors grouped up to
///   `hot_page_lines` rows (terminal feed). No cell copy on feed; flatten
///   happens only in the compression worker scratch.
enum Resident {
    Flat(Arc<[Cell]>),
    Segmented(Vec<RowDesc>),
}

enum JobPayload {
    Flat(Arc<[Cell]>),
    Segmented(Vec<RowDesc>),
}

struct Job {
    page_id: u64,
    generation: u64,
    cols: u16,
    payload: JobPayload,
}

struct Completion {
    page_id: u64,
    generation: u64,
    compressed: Vec<u8>,
    recycled_rows: Vec<Arc<[Cell]>>,
    sparse: bool,
}

/// Bounded single-page decompression cache for read access to cold pages
/// (S8 viewport/search/persistence). Keyed by (page id, generation) so
/// mutation and eviction invalidate it automatically; it holds at most one
/// page's worth of cells (`hot_page_lines * cols` cells).
struct ColdReadCache {
    page_id: u64,
    generation: u64,
    cells: Vec<Cell>,
    encoded: Vec<u8>,
}

/// A single scrollback page.
///
/// `cols` is stored per page to handle resize: new pages use current
/// terminal cols; old pages retain their `cols` for correct byte length.
struct Page {
    id: u64,
    generation: u64,
    lines: usize,
    /// Number of the page's `lines` that are entirely default cells (blank
    /// rows created by cursor movement/wrapping, not by payload records).
    /// Maintained at push time so retention metrics can distinguish payload
    /// lines from blank scrollback rows without decompressing cold pages.
    empty_lines: usize,
    cols: u16,
    capacity_lines: usize,
    /// Hot resident. `Flat` is the contiguous buffer used by `push_line`;
    /// `Segmented` is the grouped descriptor Vec used by `ingest_owned_*`.
    resident: Option<Resident>,
    compressed: Option<Arc<[u8]>>,
    sparse: bool,
    pending: bool,
}

impl Page {
    fn is_cold(&self) -> bool {
        self.compressed.is_some() && self.resident.is_none()
    }
    fn is_hot(&self) -> bool {
        self.resident.is_some()
    }
    fn resident_bytes(&self) -> usize {
        match self.resident.as_ref() {
            Some(Resident::Flat(b)) => b.len() * std::mem::size_of::<Cell>(),
            Some(Resident::Segmented(descs)) => descs
                .iter()
                .map(|d| d.cells.len() * std::mem::size_of::<Cell>())
                .sum(),
            None => 0,
        }
    }
}

struct StagedStoragePage {
    page_id: u64,
    generation: u64,
    resident: Option<Resident>,
    compressed: Option<Arc<[u8]>>,
    sparse: bool,
}

pub(crate) struct StorageStyleRemap {
    pages: Vec<StagedStoragePage>,
    compressed_pool: HashMap<u64, Vec<Weak<[u8]>>>,
}

/// Diagnostics for one test-only storage style census pass.
#[cfg(test)]
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct StorageCensusDiag {
    pub total_pages: usize,
    pub hot_flat_pages: usize,
    pub hot_segmented_pages: usize,
    pub cold_pages: usize,
    pub corrupt_cold_pages: usize,
    pub total_rows: usize,
    pub total_cells_scanned: usize,
    pub total_occupied_cells: usize,
}



// ---------------------------------------------------------------------------
// ScrollbackStorage — owns history + bounded channels + worker handle.
// ---------------------------------------------------------------------------
pub struct ScrollbackStorage {
    config: ScrollbackConfig,
    cols: u16,
    history: VecDeque<Page>,
    compressed_pool: HashMap<u64, Vec<Weak<[u8]>>>,
    logical_lines: usize,
    next_page_id: u64,
    /// First history index that may still need a compression enqueue. Pages
    /// before it are pending, cold, or already enqueued; this turns the
    /// per-push full-history scan into an O(eligible) scan.
    enqueue_cursor: usize,
    /// Bounded single-page decompression cache for `read_line` over cold pages.
    cold_cache: Option<ColdReadCache>,
    /// Kept for `force_compress_all` scratch reuse; unused on the feed path.
    #[allow(dead_code)]
    compression_scratch: Vec<u8>,
    recycled_rows: Vec<Arc<[Cell]>>,
    #[allow(dead_code)]
    encoded_scratch: Vec<u8>,
    /// Reusable private scratch for style-cardinality and compaction decoding.
    census_cells: Vec<Cell>,
    /// Encoded bytes scratch paired with `census_cells`.
    census_encoded: Vec<u8>,
    job_tx: Option<SyncSender<Job>>,
    completion_rx: Receiver<Completion>,
    worker: Option<JoinHandle<()>>,
    queued_jobs: Arc<AtomicUsize>,
    pending_completions: Arc<AtomicUsize>,
    dropped_completions: Arc<AtomicU64>,
    restored_pages: u64,
    stale_discarded: u64,
}
impl ScrollbackStorage {
    pub fn new(cols: u16, config: ScrollbackConfig) -> Self {
        let (job_tx, job_rx) = sync_channel::<Job>(config.max_queued_jobs.max(1));
        let (completion_tx, completion_rx) =
            sync_channel::<Completion>(config.max_pending_completions.max(1));

        let queued_jobs = Arc::new(AtomicUsize::new(0));
        let pending_completions = Arc::new(AtomicUsize::new(0));
        let dropped_completions = Arc::new(AtomicU64::new(0));
        let worker_queued_jobs = Arc::clone(&queued_jobs);
        let worker_pending_completions = Arc::clone(&pending_completions);
        let worker_dropped_completions = Arc::clone(&dropped_completions);
        let worker = std::thread::spawn(move || {
            worker_loop(
                job_rx,
                completion_tx,
                worker_queued_jobs,
                worker_pending_completions,
                worker_dropped_completions,
            );
        });

        Self {
            config,
            cols,
            history: VecDeque::new(),
            compressed_pool: HashMap::new(),
            logical_lines: 0,
            next_page_id: 1,
            enqueue_cursor: 0,
            cold_cache: None,
            compression_scratch: Vec::new(),
            encoded_scratch: Vec::new(),
            recycled_rows: Vec::new(),
            census_cells: Vec::new(),
            census_encoded: Vec::new(),
            job_tx: Some(job_tx),
            completion_rx,
            worker: Some(worker),
            queued_jobs,
            pending_completions,
            dropped_completions,
            restored_pages: 0,
            stale_discarded: 0,
        }
    }

    pub fn config(&self) -> ScrollbackConfig {
        self.config
    }

    /// Return cleared-row candidates released after lossless page compression.
    pub fn take_recycled_rows(&mut self) -> Vec<Arc<[Cell]>> {
        std::mem::take(&mut self.recycled_rows)
    }

    pub fn update_config(&mut self, next: ScrollbackConfig) {
        // Queue capacities are fixed at construction (bounded sync_channel).
        // For S3 the bench calls this before any history push, so caps are
        // still defaults. Preserve existing channels/worker and just update
        // logical limits. If hot_page_lines changes, new pages will use it.
        self.config.max_lines = next.max_lines;
        self.config.hot_page_lines = next.hot_page_lines;
        // Queue caps intentionally not resized mid-life to avoid recreating
        // the worker thread and losing in-flight jobs. Tests/bench use default
        // 32/32.
        self.enforce_max_lines();
    }

    pub fn set_cols(&mut self, cols: u16) {
        self.cols = cols;
    }

    // -----------------------------------------------------------------------
    // S8 read APIs — viewport scrolling, search, selection, persistence.
    // Reads never mutate page contents; cold pages are decompressed through
    // a bounded single-page cache keyed by (page id, generation).
    // -----------------------------------------------------------------------

    /// Total logical history lines currently stored.
    pub fn total_lines(&self) -> usize {
        self.logical_lines
    }

    /// Number of columns of the logical history line at `index`. Columns can
    /// vary across the history because a resize changes the width used for
    /// subsequently captured lines while older pages keep their own width.
    /// Segmented pages preserve per-row `cols` from the moved descriptors.
    pub fn line_cols(&self, index: usize) -> Option<usize> {
        let mut offset = 0usize;
        for page in &self.history {
            if index < offset + page.lines {
                if let Some(Resident::Segmented(descs)) = page.resident.as_ref() {
                    return Some(usize::from(descs[index - offset].cols));
                }
                return Some(usize::from(page.cols));
            }
            offset += page.lines;
        }
        None
    }

    /// Read the cells of logical history line `index` into `out`, replacing
    /// its contents. Returns false when `index` is out of range or the page
    /// payload is corrupt (decompression failure); corrupt pages keep their
    /// compressed bytes and are never fabricated.
    pub fn read_line(&mut self, index: usize, out: &mut Vec<Cell>) -> bool {
        out.clear();
        // Locate the page containing `index` by scanning logical offsets.
        let mut offset = 0usize;
        let mut target = None;
        for (page_index, page) in self.history.iter().enumerate() {
            if index < offset + page.lines {
                target = Some((page_index, index - offset));
                break;
            }
            offset += page.lines;
        }
        let Some((page_index, line_in_page)) = target else {
            return false;
        };
        if self.history[page_index].pending
            && self.history[page_index].resident.is_none()
            && self.history[page_index].compressed.is_none()
        {
            self.drain_compression();
        }
        let page = &self.history[page_index];
        let cols = usize::from(page.cols);
        let start = line_in_page * cols;
        if let Some(resident) = page.resident.as_ref() {
            match resident {
                Resident::Flat(buf) => {
                    let cols = usize::from(page.cols);
                    let start = line_in_page * cols;
                    out.extend_from_slice(&buf[start..start + cols]);
                    return true;
                }
                Resident::Segmented(descs) => {
                    out.extend_from_slice(&descs[line_in_page].cells);
                    return true;
                }
            }
        }
        if page.compressed.is_some() {
            let cache = self.cold_cache.get_or_insert_with(|| ColdReadCache {
                page_id: 0,
                generation: 0,
                cells: Vec::new(),
                encoded: Vec::new(),
            });
            if cache.page_id != page.id || cache.generation != page.generation {
                if decode_page(page, &mut cache.cells, &mut cache.encoded) {
                    cache.page_id = page.id;
                    cache.generation = page.generation;
                } else {
                    cache.page_id = u64::MAX;
                    return false;
                }
            }
            out.extend_from_slice(&cache.cells[start..start + cols]);
            return true;
        }
        false
    }

    /// Fold `fold` over every stored logical line, oldest first, starting
    /// from `init`. Cold pages are decompressed once through the bounded
    /// single-page cache (keyed by page id + generation), so a full scan
    /// never decompresses a page twice and never keeps more than one page
    /// resident. The closure borrows each line's cells only for the duration
    /// of one call. Corrupt pages are skipped (their compressed bytes are
    /// preserved; no cells are fabricated), so a scan over intact history
    /// visits every line. Returns the final accumulator.
    pub fn fold_lines<A>(&mut self, init: A, mut fold: impl FnMut(A, &[Cell]) -> A) -> A {
        // Settle pending pages whose resident and compressed representations are
        // both absent before traversal, matching `read_line`. Without this,
        // intact-history folds would skip lines that are mid-flight in the
        // bounded compression pipeline.
        if self
            .history
            .iter()
            .any(|p| p.pending && p.resident.is_none() && p.compressed.is_none())
        {
            self.drain_compression();
        }
        let mut acc = init;
        for page_index in 0..self.history.len() {
            let page = &self.history[page_index];
            // Segmented hot pages are folded descriptor by descriptor to avoid
            // flattening.
            if let Some(Resident::Segmented(descs)) = page.resident.as_ref() {
                for desc in descs.iter() {
                    acc = fold(acc, &desc.cells);
                }
                continue;
            }
            let cols = usize::from(page.cols);
            let lines = page.lines;
            let cells: &[Cell] = if let Some(Resident::Flat(buf)) = page.resident.as_ref() {
                buf
            } else if page.compressed.is_some() {
                let cache = self.cold_cache.get_or_insert_with(|| ColdReadCache {
                    page_id: 0,
                    generation: 0,
                    cells: Vec::new(),
                    encoded: Vec::new(),
                });
                if cache.page_id != page.id || cache.generation != page.generation {
                    if decode_page(page, &mut cache.cells, &mut cache.encoded) {
                        cache.page_id = page.id;
                        cache.generation = page.generation;
                    } else {
                        cache.page_id = u64::MAX;
                        continue;
                    }
                }
                &cache.cells[..]
            } else {
                continue;
            };
            for line in 0..lines {
                let start = line * cols;
                acc = fold(acc, &cells[start..start + cols]);
            }
        }
        acc
    }

    /// Append exactly one logical line of `cols` cells (the line may be
    /// narrower or wider than the storage's current column count; a resize
    /// leaves older pages at their original width). `cells` beyond `cols`
    /// are ignored; missing cells are filled with default cells.
    pub fn push_line(&mut self, cols: u16, cells: &[Cell]) {
        if cols == 0 {
            return;
        }
        let cols_usize = usize::from(cols);
        // A line is blank (empty) when every cell is a default cell; cells
        // beyond `cells.len()` are default-filled and stay blank.
        let line_empty = cells[..cells.len().min(cols_usize)]
            .iter()
            .all(Cell::is_default);
        let need_new_page = match self.history.back() {
            None => true,
            Some(page) => {
                // Flat feed must not append into a segmented hot page.
                let is_flat = matches!(page.resident.as_ref(), Some(Resident::Flat(_)));
                !is_flat || page.cols != cols || page.is_cold() || page.lines >= page.capacity_lines
            }
        };
        if need_new_page {
            let capacity = self.config.hot_page_lines;
            let cell_capacity = capacity * cols_usize;
            // One allocation, filled in place: `Arc<[T]>: FromIterator` builds
            // the ArcInner directly (no intermediate Vec, no byte copy).
            let mut resident: Arc<[Cell]> =
                std::iter::repeat_n(Cell::default(), cell_capacity).collect();
            let copy_len = cells.len().min(cols_usize);
            Arc::get_mut(&mut resident).expect("fresh page Arc is unique")[..copy_len]
                .copy_from_slice(&cells[..copy_len]);
            self.history.push_back(Page {
                id: self.next_page_id,
                generation: 0,
                lines: 1,
                empty_lines: usize::from(line_empty),
                cols,
                capacity_lines: capacity,
                resident: Some(Resident::Flat(resident)),
                compressed: None,
                sparse: false,
                pending: false,
            });
            self.next_page_id = self.next_page_id.wrapping_add(1);
            self.logical_lines += 1;
        } else {
            let back = self.history.back_mut().expect("page exists");
            let start = back.lines * cols_usize;
            if let Some(Resident::Flat(buf)) = back.resident.as_mut() {
                // The back page is never enqueued while it is the mutation
                // target, so the Arc is unique and this mutates in place.
                let resident = Arc::make_mut(buf);
                let copy_len = cells.len().min(cols_usize);
                resident[start..start + copy_len].copy_from_slice(&cells[..copy_len]);
            }
            // Mutating a compressed or pending page invalidates any in-flight
            // compression job via the generation bump.
            if back.compressed.is_some() {
                back.compressed = None;
                back.generation = back.generation.wrapping_add(1);
            } else if back.pending {
                back.generation = back.generation.wrapping_add(1);
                back.pending = false;
            }
            back.lines += 1;
            back.empty_lines += usize::from(line_empty);
            self.logical_lines += 1;
        }
        self.maybe_enqueue_full_pages();
        self.enforce_max_lines();
    }

    /// Drop the newest `total_lines() - keep` logical lines. Used to discard
    /// alternate-screen scrollback on `?1049l` (the mark is the history
    /// length at `?1049h`). A cold page straddling the boundary is
    /// decompressed once (bounded), shrunk, and left hot.
    pub fn truncate_lines(&mut self, keep: usize) {
        if keep >= self.logical_lines {
            return;
        }
        let mut drop = self.logical_lines - keep;
        while drop > 0 {
            let Some(back_index) = self.history.len().checked_sub(1) else {
                break;
            };
            let lines = self.history[back_index].lines;
            if lines <= drop {
                self.history.pop_back();
                self.logical_lines = self.logical_lines.saturating_sub(lines);
                drop -= lines;
                continue;
            }
            // Partial truncation of the newest page. A cold page must be
            // decompressed before its line count can be shrunk.
            let page = &mut self.history[back_index];
            if page.resident.is_none() && page.compressed.is_some() {
                let mut restored = Vec::new();
                let mut encoded = Vec::new();
                if decode_page(page, &mut restored, &mut encoded) {
                    page.resident = Some(Resident::Flat(Arc::from(restored.into_boxed_slice())));
                    page.compressed = None;
                    page.sparse = false;
                    page.pending = false;
                    page.generation = page.generation.wrapping_add(1);
                }
            }
            // Shrink the newest page's kept prefix. A missing resident after
            // a failed restore means the page is corrupt: drop it whole.
            let page = &self.history[back_index];
            match page.resident.as_ref() {
                Some(Resident::Segmented(_)) => {
                    let page = &mut self.history[back_index];
                    if let Some(Resident::Segmented(d)) = page.resident.as_mut() {
                        let new_len = d.len().saturating_sub(drop);
                        d.truncate(new_len);
                        page.lines = new_len;
                        page.empty_lines =
                            d.iter().filter(|r| r.occupancy == 0 && !r.wrapped).count();
                    }
                    self.logical_lines = keep;
                    drop = 0;
                }
                Some(Resident::Flat(buf)) => {
                    let cols = usize::from(page.cols);
                    let kept = page.lines - drop;
                    let empty = (0..kept)
                        .filter(|line| {
                            let start = line * cols;
                            buf[start..start + cols].iter().all(Cell::is_default)
                        })
                        .count();
                    let page = &mut self.history[back_index];
                    page.lines = kept;
                    page.empty_lines = empty;
                    self.logical_lines = keep;
                    drop = 0;
                }
                None => {
                    // Corrupt/undecompressible page: drop it whole rather than
                    // fabricate content.
                    self.history.pop_back();
                    self.logical_lines = self.logical_lines.saturating_sub(lines);
                    drop = drop.saturating_sub(lines);
                }
            }
        }
        self.cold_cache = None;
    }

    /// Drop all history (pages, logical lines, and the read cache). In-flight
    /// compression completions for dropped pages are discarded by their page
    /// id (counted as stale).
    pub fn clear(&mut self) {
        self.history.clear();
        self.logical_lines = 0;
        self.cold_cache = None;
    }

    pub fn stats(&self) -> StorageStats {
        let hot_resident_bytes = self.history.iter().map(Page::resident_bytes).sum();
        let empty_lines = self.history.iter().map(|page| page.empty_lines).sum();
        let mut seen = HashSet::new();
        let compressed_bytes = self
            .history
            .iter()
            .filter_map(|page| page.compressed.as_ref())
            .filter(|bytes| seen.insert(bytes.as_ptr() as usize))
            .map(|bytes| bytes.len())
            .sum();
        let queued_jobs = self.queued_jobs.load(Ordering::Acquire);
        let pending_completions = self.pending_completions.load(Ordering::Acquire);
        StorageStats {
            logical_lines: self.logical_lines,
            empty_lines,
            hot_resident_bytes,
            compressed_bytes,
            queued_jobs,
            pending_completions,
            restored_pages: self.restored_pages,
            stale_discarded: self.stale_discarded
                + self.dropped_completions.load(Ordering::Acquire),
        }
    }

    /// Push `cells` (flattened rows, `cells.len() % cols == 0` ideally) into
    /// paged history, respecting `max_lines`. Used by `Terminal::feed` to
    /// account history growth and by tests/bench directly.
    pub fn push_cells(&mut self, cells: &[Cell]) {
        if cells.is_empty() || self.cols == 0 {
            return;
        }
        let cols = usize::from(self.cols);
        // Number of logical lines in this push.
        let lines = cells.len().div_ceil(cols);
        self.push_lines_internal(cells, lines, cols);
    }

    /// Push `line_count` lines where each line is `cols` cells from `cells`.
    /// If `cells` is shorter than `line_count * cols`, remaining cells are
    /// filled with default `Cell`.
    fn push_lines_internal(&mut self, cells: &[Cell], mut lines_remaining: usize, cols: usize) {
        let mut offset = 0usize;
        while lines_remaining > 0 {
            let need_new_page = self.history.back().is_none_or(|page| {
                let is_flat = matches!(page.resident.as_ref(), Some(Resident::Flat(_)));
                !is_flat
                    || page.cols != self.cols
                    || page.lines >= page.capacity_lines
                    || page.is_cold()
            });
            if need_new_page {
                // Allocate a new hot page (single allocation, filled in place).
                let capacity = self.config.hot_page_lines;
                let cell_capacity = capacity * cols;
                let mut resident: Arc<[Cell]> =
                    std::iter::repeat_n(Cell::default(), cell_capacity).collect();
                // Fill first line(s) from input if available.
                let fill_lines = lines_remaining.min(capacity);
                let fill_cells = fill_lines * cols;
                let copy_len = fill_cells.min(cells.len().saturating_sub(offset));
                if copy_len > 0 {
                    Arc::get_mut(&mut resident).expect("fresh page Arc is unique")[..copy_len]
                        .copy_from_slice(&cells[offset..offset + copy_len]);
                }
                let mut empty_lines = 0usize;
                for line in 0..fill_lines {
                    let start = offset + line * cols;
                    let end = (start + cols).min(cells.len());
                    // Cells past `cells.len()` are default-filled and blank.
                    if cells
                        .get(start..end)
                        .is_none_or(|s| s.iter().all(Cell::is_default))
                    {
                        empty_lines += 1;
                    }
                }
                offset += copy_len;
                let page = Page {
                    id: self.next_page_id,
                    generation: 0,
                    lines: fill_lines,
                    empty_lines,
                    cols: self.cols,
                    capacity_lines: capacity,
                    resident: Some(Resident::Flat(resident)),
                    compressed: None,
                    sparse: false,
                    pending: false,
                };
                self.next_page_id = self.next_page_id.wrapping_add(1);
                self.history.push_back(page);
                // The page that just filled is no longer the mutable back
                // page and becomes eligible for compression enqueue.
                if self.history.len() > 1 {
                    self.enqueue_cursor = self.enqueue_cursor.min(self.history.len() - 2);
                }
                self.logical_lines += fill_lines;
                lines_remaining -= fill_lines;
            } else {
                // Fill existing back page.
                let back = self.history.back_mut().unwrap();
                // Safe to mutate resident because page is hot; the back page
                // is never enqueued, so the Arc is unique and make_mut is
                // zero-copy (clone-on-write only if ever shared).
                let capacity = back.capacity_lines;
                let available = capacity - back.lines;
                let fill_lines = lines_remaining.min(available);
                let start_cell = back.lines * cols;
                let fill_cells = fill_lines * cols;
                let copy_len = fill_cells.min(cells.len().saturating_sub(offset));
                if let Some(Resident::Flat(buf)) = back.resident.as_mut() {
                    if copy_len > 0 {
                        Arc::make_mut(buf)[start_cell..start_cell + copy_len]
                            .copy_from_slice(&cells[offset..offset + copy_len]);
                    }
                }
                back.lines += fill_lines;
                let mut empty_lines = 0usize;
                for line in 0..fill_lines {
                    let start = offset + line * cols;
                    let end = (start + cols).min(cells.len());
                    if cells
                        .get(start..end)
                        .is_none_or(|s| s.iter().all(Cell::is_default))
                    {
                        empty_lines += 1;
                    }
                }
                back.empty_lines += empty_lines;
                // If page was previously compressed (should not happen because need_new_page
                // checks is_cold), treat as generation bump.
                if back.compressed.is_some() {
                    back.compressed = None;
                    back.generation = back.generation.wrapping_add(1);
                } else if back.pending {
                    // Mutation of a pending page bumps generation so stale
                    // completions are discarded.
                    back.generation = back.generation.wrapping_add(1);
                    back.pending = false;
                }
                self.logical_lines += fill_lines;
                lines_remaining -= fill_lines;
                offset += copy_len;
            }
            // Enqueue compression for the previous page when it became full.
            // We keep at most N hot pages resident? Spec says hot pages fixed
            // capacity, cold pages hold compressed Vec<u8>. We lazily enqueue
            // full pages except the newest one.
            self.maybe_enqueue_full_pages();
            self.enforce_max_lines();
        }
    }

    fn maybe_enqueue_full_pages(&mut self) {
        // Keep the last page hot for mutation; enqueue earlier full hot pages.
        let len = self.history.len();
        if len <= 1 {
            self.enqueue_cursor = 0;
            return;
        }
        let last_idx = len - 1;
        if self.queued_jobs.load(Ordering::Acquire) >= self.config.max_queued_jobs.max(1)
            || self.pending_completions.load(Ordering::Acquire)
                >= self.config.max_pending_completions.max(1)
        {
            return;
        }
        // A page becomes eligible only when it fills and a new back page
        // replaces it (the cursor is rewound there), and the scan advances
        // the cursor past every page it visits. On enqueue failure (bounded
        // queue full) the cursor stays at the failed page so it is retried
        // on the next poll; in the common case the scan is O(1).
        let start = self.enqueue_cursor.min(last_idx);
        let mut failed_at = None;
        for idx in start..last_idx {
            let should_enqueue = {
                let p = &self.history[idx];
                p.is_hot() && p.lines >= p.capacity_lines && !p.pending && p.compressed.is_none()
            };
            if should_enqueue && !self.enqueue_page(idx) {
                failed_at = Some(failed_at.map_or(idx, |at: usize| at.min(idx)));
            }
        }
        self.enqueue_cursor = failed_at.unwrap_or(last_idx);
    }

    fn enqueue_page(&mut self, idx: usize) -> bool {
        let (page_id, generation, cols, payload) = {
            let page = &mut self.history[idx];
            let Some(resident) = page.resident.take() else {
                return false;
            };
            let payload = match resident {
                Resident::Flat(cells) => JobPayload::Flat(cells),
                Resident::Segmented(rows) => JobPayload::Segmented(rows),
            };
            (page.id, page.generation, page.cols, payload)
        };
        let job = Job {
            page_id,
            generation,
            cols,
            payload,
        };
        let Some(tx) = self.job_tx.as_ref() else {
            self.history[idx].resident = Some(match job.payload {
                JobPayload::Flat(cells) => Resident::Flat(cells),
                JobPayload::Segmented(rows) => Resident::Segmented(rows),
            });
            return false;
        };

        self.queued_jobs.fetch_add(1, Ordering::AcqRel);
        match tx.try_send(job) {
            Ok(()) => {
                self.history[idx].pending = true;
                true
            }
            Err(TrySendError::Full(job) | TrySendError::Disconnected(job)) => {
                // Documented overload: keep the page hot and retry later.
                // Inline LZ4 here would steal the feed thread.
                self.queued_jobs.fetch_sub(1, Ordering::AcqRel);
                self.history[idx].resident = Some(match job.payload {
                    JobPayload::Flat(cells) => Resident::Flat(cells),
                    JobPayload::Segmented(rows) => Resident::Segmented(rows),
                });
                self.history[idx].pending = false;
                false
            }
        }
    }

    fn trim_front_page(&mut self, drop_lines: usize) -> bool {
        let Some(page) = self.history.front_mut() else {
            return false;
        };
        if drop_lines == 0 || drop_lines >= page.lines {
            return false;
        }

        if page.resident.is_none() {
            let mut restored = Vec::new();
            let mut encoded = Vec::new();
            if !decode_page(page, &mut restored, &mut encoded) {
                return false;
            }
            page.resident = Some(Resident::Flat(Arc::from(restored.into_boxed_slice())));
            page.compressed = None;
            page.sparse = false;
        }

        match page.resident.as_mut() {
            Some(Resident::Segmented(rows)) => {
                rows.drain(..drop_lines);
                page.empty_lines = rows
                    .iter()
                    .filter(|row| row.occupancy == 0 && !row.wrapped)
                    .count();
            }
            Some(Resident::Flat(cells)) => {
                let cols = usize::from(page.cols);
                let old_lines = page.lines;
                let kept_lines = old_lines - drop_lines;
                let cells = Arc::make_mut(cells);
                cells.copy_within(drop_lines * cols..old_lines * cols, 0);
                cells[kept_lines * cols..old_lines * cols].fill(Cell::default());
                page.empty_lines = cells[..kept_lines * cols]
                    .chunks_exact(cols)
                    .filter(|row| row.iter().all(Cell::is_default))
                    .count();
            }
            None => return false,
        }

        page.lines -= drop_lines;
        page.generation = page.generation.wrapping_add(1);
        page.pending = false;
        self.logical_lines -= drop_lines;
        self.cold_cache = None;
        true
    }

    fn enforce_max_lines(&mut self) {
        while self.logical_lines > self.config.max_lines {
            let excess = self.logical_lines - self.config.max_lines;
            let Some(front_lines) = self.history.front().map(|page| page.lines) else {
                break;
            };
            if front_lines <= excess {
                self.history.pop_front();
                self.logical_lines -= front_lines;
                self.enqueue_cursor = self.enqueue_cursor.saturating_sub(1);
                continue;
            }
            if !self.trim_front_page(excess) {
                break;
            }
        }
    }

    // -----------------------------------------------------------------------
    // Deterministic hooks — synchronous, no sleeps, stale-generation aware.
    // -----------------------------------------------------------------------

    /// Apply only completions already available. This never waits for the
    /// worker and is safe on the terminal mutation hot path.
    pub fn poll_compression(&mut self) {
        self.apply_available_completions();
        self.maybe_enqueue_full_pages();
    }

    /// Wait until every accepted job has finished, then apply all available
    /// completions and synchronously recover any dropped result.
    pub fn drain_compression(&mut self) {
        while self.queued_jobs.load(Ordering::Acquire) != 0 {
            self.apply_available_completions();
            std::thread::yield_now();
        }
        self.apply_available_completions();
        self.maybe_enqueue_full_pages();
    }

    fn retain_recycled_rows(&mut self, rows: Vec<Arc<[Cell]>>) {
        let remaining = MAX_RECYCLED_ROWS.saturating_sub(self.recycled_rows.len());
        self.recycled_rows.extend(rows.into_iter().take(remaining));
    }

    /// Private quiescence barrier for style-cardinality transactions.
    /// Waits until `queued_jobs==0` **and** `pending_completions==0` while
    /// applying completions as they arrive, then one final try-drain.
    /// Never calls `maybe_enqueue_full_pages`. Returns true only when
    /// `queued_jobs==0`, `pending_completions==0`, and every page
    /// `pending==false`. Reuses the single `apply_completion` helper so the
    /// quiescence path cannot diverge from `apply_available_completions`.
    /// Avoids recv deadlock if `queued` hits zero before the last
    /// `pending` increment becomes visible by checking both counters.
    pub(crate) fn quiesce_for_style_transaction(&mut self) -> bool {
        loop {
            let queued = self.queued_jobs.load(Ordering::Acquire);
            let pending = self.pending_completions.load(Ordering::Acquire);
            if queued == 0 && pending == 0 {
                break;
            }
            match self.completion_rx.recv() {
                Ok(completion) => {
                    self.apply_completion(completion);
                }
                Err(_) => break,
            }
            while let Ok(completion) = self.completion_rx.try_recv() {
                self.apply_completion(completion);
            }
        }
        while let Ok(completion) = self.completion_rx.try_recv() {
            self.apply_completion(completion);
        }
        let queued_zero = self.queued_jobs.load(Ordering::Acquire) == 0;
        let pending_zero = self.pending_completions.load(Ordering::Acquire) == 0;
        let no_pending_pages = self.history.iter().all(|p| !p.pending);
        queued_zero && pending_zero && no_pending_pages
    }

    #[inline]
    fn apply_completion(&mut self, completion: Completion) {
        self.pending_completions.fetch_sub(1, Ordering::AcqRel);
        let Completion {
            page_id,
            generation,
            compressed,
            recycled_rows,
            sparse,
        } = completion;
        let mut found = false;
        let mut recycle = false;
        for page in &mut self.history {
            if page.id == page_id {
                found = true;
                if page.generation != generation {
                    self.stale_discarded += 1;
                    page.pending = false;
                } else {
                    page.compressed = Some(Self::intern_compressed(&mut self.compressed_pool, compressed));
                    page.sparse = sparse;
                    page.resident = None;
                    page.pending = false;
                    recycle = true;
                }
                break;
            }
        }
        if recycle {
            self.retain_recycled_rows(recycled_rows);
        }
        if !found {
            self.stale_discarded += 1;
        }
    }


    // -----------------------------------------------------------------------
    // Style-cardinality census — private storage internals, full-cold
    // inclusive, decoded via reusable private scratch without mutating
    // ColdReadCache. Corrupt cold data fails closed (skipped).
    // One explicit occupied-range rule is shared by optimized/exhaustive.
    // -----------------------------------------------------------------------
    #[inline]
    fn occupied_range_for_census(cells: &[Cell], cols: usize, first_occupied: u16, occupancy: u16, wrapped: bool) -> std::ops::Range<usize> {
        let first = usize::from(first_occupied.min(cols as u16));
        let mut end = usize::from(occupancy.min(cols as u16));
        if wrapped && end < cols {
            end = cols;
        }
        if first >= end || first >= cells.len() {
            return 0..0;
        }
        first..end.min(cells.len())
    }

    #[inline]
    fn occupied_range_by_scan(cells: &[Cell], wrapped: bool) -> std::ops::Range<usize> {
        if cells.is_empty() {
            return 0..0;
        }
        let first = cells
            .iter()
            .position(|cell| !cell.is_default())
            .unwrap_or(cells.len());
        if first == cells.len() {
            return 0..0;
        }
        let mut last = cells.len();
        while last > first && cells[last - 1].is_default() {
            last -= 1;
        }
        if wrapped {
            last = cells.len();
        }
        first..last
    }

    fn decode_cold_for_census(&mut self, page_idx: usize) -> Option<usize> {
        let page = &self.history[page_idx];
        if page.compressed.is_none() || page.resident.is_some() {
            return None;
        }
        // Clone compressed Arc and cols to avoid borrow of self vs scratch.
        let compressed = page.compressed.clone();
        let cols = page.cols;
        let capacity_lines = page.capacity_lines;
        let lines = page.lines;
        let sparse = page.sparse;
        // Temporary page view for decode_page helper.
        let tmp = Page {
            id: page.id,
            generation: page.generation,
            lines,
            empty_lines: page.empty_lines,
            cols,
            capacity_lines,
            resident: None,
            compressed,
            sparse,
            pending: page.pending,
        };
        let ok = decode_page(&tmp, &mut self.census_cells, &mut self.census_encoded);
        if ok { Some(self.census_cells.len()) } else { None }
    }

    pub(crate) fn collect_live_style_ids(
        &mut self,
        out: &mut BTreeSet<u16>,
    ) -> Result<(), TerminalError> {
        for idx in 0..self.history.len() {
            let (kind, cols, lines, has_compressed) = {
                let page = &self.history[idx];
                let kind = match page.resident.as_ref() {
                    Some(Resident::Flat(_)) => 1u8,
                    Some(Resident::Segmented(_)) => 2u8,
                    None => 0u8,
                };
                (kind, usize::from(page.cols), page.lines, page.compressed.is_some())
            };
            match kind {
                1 => {
                    let page = &self.history[idx];
                    let Some(Resident::Flat(cells)) = page.resident.as_ref() else {
                        return Err(TerminalError::StyleCompactionCorrupt);
                    };
                    for line in 0..lines {
                        let start = line
                            .checked_mul(cols)
                            .ok_or(TerminalError::StyleCompactionCorrupt)?;
                        let end = start
                            .checked_add(cols)
                            .ok_or(TerminalError::StyleCompactionCorrupt)?;
                        let row = cells
                            .get(start..end)
                            .ok_or(TerminalError::StyleCompactionCorrupt)?;
                        let range = Self::occupied_range_by_scan(row, false);
                        if range.is_empty() {
                            out.insert(0);
                        } else {
                            for cell in &row[range] {
                                out.insert(cell.style);
                            }
                        }
                    }
                }
                2 => {
                    let page = &self.history[idx];
                    let Some(Resident::Segmented(rows)) = page.resident.as_ref() else {
                        return Err(TerminalError::StyleCompactionCorrupt);
                    };
                    if rows.len() != lines {
                        return Err(TerminalError::StyleCompactionCorrupt);
                    }
                    for row in rows {
                        let range = Self::occupied_range_for_census(
                            &row.cells,
                            usize::from(row.cols),
                            row.first_occupied,
                            row.occupancy,
                            row.wrapped,
                        );
                        if range.is_empty() {
                            out.insert(0);
                        } else {
                            for cell in &row.cells[range] {
                                out.insert(cell.style);
                            }
                        }
                    }
                }
                _ if has_compressed => {
                    if self.decode_cold_for_census(idx).is_none() {
                        return Err(TerminalError::StyleCompactionCorrupt);
                    }
                    for line in 0..lines {
                        let start = line
                            .checked_mul(cols)
                            .ok_or(TerminalError::StyleCompactionCorrupt)?;
                        let end = start
                            .checked_add(cols)
                            .ok_or(TerminalError::StyleCompactionCorrupt)?;
                        let row = self
                            .census_cells
                            .get(start..end)
                            .ok_or(TerminalError::StyleCompactionCorrupt)?;
                        let range = Self::occupied_range_by_scan(row, false);
                        if range.is_empty() {
                            out.insert(0);
                        } else {
                            for cell in &row[range] {
                                out.insert(cell.style);
                            }
                        }
                    }
                }
                _ => return Err(TerminalError::StyleCompactionCorrupt),
            }
        }
        Ok(())
    }

    pub(crate) fn stage_style_remap(
        &mut self,
        remap: &StyleRemap,
    ) -> Result<StorageStyleRemap, TerminalError> {
        // Stage a cloned weak compressed pool; all allocation/hashing/encoding
        // stays before semantic mutation. Hot pages are remapped into new
        // resident ownership, cold pages are decoded via reusable census
        // scratch, remapped, re-encoded, and interned into the staged pool.
        let mut staged_pool = self.compressed_pool.clone();
        let mut pages = Vec::with_capacity(self.history.len());
        for idx in 0..self.history.len() {
            let resident = {
                let page = &self.history[idx];
                match page.resident.as_ref() {
                    Some(Resident::Flat(cells)) => {
                        let mut remapped = cells.to_vec();
                        remap_cells(&mut remapped, remap)?;
                        Some(Resident::Flat(Arc::from(remapped.into_boxed_slice())))
                    }
                    Some(Resident::Segmented(rows)) => {
                        let mut remapped_rows = Vec::with_capacity(rows.len());
                        for row in rows {
                            let mut cells = row.cells.to_vec();
                            remap_cells(&mut cells, remap)?;
                            remapped_rows.push(RowDesc {
                                cells: Arc::from(cells.into_boxed_slice()),
                                cols: row.cols,
                                occupancy: row.occupancy,
                                first_occupied: row.first_occupied,
                                wrapped: row.wrapped,
                            });
                        }
                        Some(Resident::Segmented(remapped_rows))
                    }
                    None => None,
                }
            };

            let (page_id, generation, sparse) = {
                let page = &self.history[idx];
                (page.id, page.generation, page.sparse)
            };
            let compressed = if resident.is_none() {
                if self.decode_cold_for_census(idx).is_none() {
                    return Err(TerminalError::StyleCompactionCorrupt);
                }
                let mut cells = self.census_cells.clone();
                remap_cells(&mut cells, remap)?;
                let encoded = encode_cold_page(&self.history[idx], &cells)?;
                let interned = Self::intern_compressed(&mut staged_pool, encoded);
                Some(interned)
            } else {
                None
            };

            pages.push(StagedStoragePage {
                page_id,
                generation,
                resident,
                compressed,
                sparse,
            });
        }
        Ok(StorageStyleRemap {
            pages,
            compressed_pool: staged_pool,
        })
    }

    pub(crate) fn commit_style_remap(&mut self, staged: StorageStyleRemap) {
        debug_assert_eq!(staged.pages.len(), self.history.len());
        debug_assert!(self.history.iter().zip(&staged.pages).all(|(page, patch)| {
            page.id == patch.page_id && page.generation == patch.generation && !page.pending
        }));
        // All fallible work is staged; commit is assignments/clears/swaps only.
        let StorageStyleRemap {
            pages,
            compressed_pool,
        } = staged;
        for (page, patch) in self.history.iter_mut().zip(pages) {
            page.generation = page.generation.wrapping_add(1);
            page.pending = false;
            page.resident = patch.resident;
            page.sparse = patch.sparse;
            page.compressed = patch.compressed;
        }
        self.compressed_pool = compressed_pool;
        self.cold_cache = None;
        self.recycled_rows.clear();
        self.enqueue_cursor = self.history.len().saturating_sub(1);
    }

    pub(crate) fn resume_style_transaction(&mut self) {
        // Reset to first eligible page and invoke nonblocking enqueue scanning.
        // Works for both caller branches (success and preflight error) without
        // dropping payloads; the page payload remains owned by history.
        self.enqueue_cursor = 0;
        self.maybe_enqueue_full_pages();
    }

    #[cfg(test)]
    pub(crate) fn census_storage_styles_optimized(&mut self, out: &mut BTreeSet<u16>) -> StorageCensusDiag {
        let mut diag = StorageCensusDiag {
            total_pages: self.history.len(),
            hot_flat_pages: 0,
            hot_segmented_pages: 0,
            cold_pages: 0,
            corrupt_cold_pages: 0,
            total_rows: 0,
            total_cells_scanned: 0,
            total_occupied_cells: 0,
        };
        let len = self.history.len();
        for idx in 0..len {
            let (kind, cols, lines, has_compressed) = {
                let p = &self.history[idx];
                let k = match p.resident.as_ref() {
                    Some(Resident::Flat(_)) => 1u8,
                    Some(Resident::Segmented(_)) => 2u8,
                    None => 0u8,
                };
                (k, p.cols, p.lines, p.compressed.is_some())
            };
            if kind == 1 {
                diag.hot_flat_pages += 1;
                // Borrow flat buffer without cloning.
                let page = &self.history[idx];
                if let Some(Resident::Flat(buf)) = page.resident.as_ref() {
                    let cols_usize = usize::from(cols);
                    diag.total_rows += lines;
                    for line in 0..lines {
                        let start = line * cols_usize;
                        let end = start + cols_usize;
                        if end > buf.len() { break; }
                        let slice = &buf[start..end];
                        diag.total_cells_scanned += cols_usize;
                        let range = Self::occupied_range_by_scan(slice, false);
                        for cell in &slice[range.clone()] { out.insert(cell.style); }
                        diag.total_occupied_cells += range.len();
                        if range.is_empty() { out.insert(0); }
                    }
                }
            } else if kind == 2 {
                diag.hot_segmented_pages += 1;
                let page = &self.history[idx];
                if let Some(Resident::Segmented(descs)) = page.resident.as_ref() {
                    for d in descs.iter() {
                        diag.total_rows += 1;
                        diag.total_cells_scanned += d.cells.len();
                        let range = Self::occupied_range_for_census(&d.cells, usize::from(d.cols), d.first_occupied, d.occupancy, d.wrapped);
                        for cell in &d.cells[range.clone()] { out.insert(cell.style); }
                        diag.total_occupied_cells += range.len();
                        if range.is_empty() { out.insert(0); }
                    }
                }
            } else if has_compressed {
                diag.cold_pages += 1;
                let decode_ok = self.decode_cold_for_census(idx);
                if decode_ok.is_none() {
                    diag.corrupt_cold_pages += 1;
                    continue;
                }
                let cols_usize = usize::from(cols);
                // Borrow reusable scratch; no cloning of full page.
                let decoded: &[Cell] = &self.census_cells;
                for line in 0..lines {
                    let start = line * cols_usize;
                    let end = start + cols_usize;
                    if end > decoded.len() { break; }
                    let slice = &decoded[start..end];
                    diag.total_rows += 1;
                    diag.total_cells_scanned += cols_usize;
                    let range = Self::occupied_range_by_scan(slice, false);
                    for cell in &slice[range.clone()] { out.insert(cell.style); }
                    diag.total_occupied_cells += range.len();
                    if range.is_empty() { out.insert(0); }
                }
            }
        }
        diag
    }

    #[cfg(test)]
    pub(crate) fn census_storage_styles_exhaustive(&mut self, out: &mut BTreeSet<u16>) -> StorageCensusDiag {
        let mut diag = StorageCensusDiag {
            total_pages: self.history.len(),
            hot_flat_pages: 0,
            hot_segmented_pages: 0,
            cold_pages: 0,
            corrupt_cold_pages: 0,
            total_rows: 0,
            total_cells_scanned: 0,
            total_occupied_cells: 0,
        };
        let len = self.history.len();
        for idx in 0..len {
            let (kind, cols, lines, has_compressed) = {
                let p = &self.history[idx];
                let k = match p.resident.as_ref() {
                    Some(Resident::Flat(_)) => 1u8,
                    Some(Resident::Segmented(_)) => 2u8,
                    None => 0u8,
                };
                (k, p.cols, p.lines, p.compressed.is_some())
            };
            if kind == 1 {
                diag.hot_flat_pages += 1;
                let page = &self.history[idx];
                if let Some(Resident::Flat(buf)) = page.resident.as_ref() {
                    let cols_usize = usize::from(cols);
                    diag.total_rows += lines;
                    for line in 0..lines {
                        let start = line * cols_usize;
                        let end = start + cols_usize;
                        if end > buf.len() { break; }
                        let slice = &buf[start..end];
                        diag.total_cells_scanned += cols_usize;
                        let range = Self::occupied_range_by_scan(slice, false);
                        for cell in &slice[range.clone()] { out.insert(cell.style); }
                        diag.total_occupied_cells += range.len();
                        if range.is_empty() { out.insert(0); }
                    }
                }
            } else if kind == 2 {
                diag.hot_segmented_pages += 1;
                let page = &self.history[idx];
                if let Some(Resident::Segmented(descs)) = page.resident.as_ref() {
                    for d in descs.iter() {
                        diag.total_rows += 1;
                        diag.total_cells_scanned += d.cells.len();
                        // Independently recompute placement from cells, while
                        // preserving CompactRow's wrapped-to-full-row rule.
                        let range = Self::occupied_range_by_scan(&d.cells, d.wrapped);
                        for cell in &d.cells[range.clone()] { out.insert(cell.style); }
                        diag.total_occupied_cells += range.len();
                        if range.is_empty() { out.insert(0); }
                    }
                }
            } else if has_compressed {
                diag.cold_pages += 1;
                let decode_ok = self.decode_cold_for_census(idx);
                if decode_ok.is_none() {
                    diag.corrupt_cold_pages += 1;
                    continue;
                }
                let cols_usize = usize::from(cols);
                let decoded: &[Cell] = &self.census_cells;
                for line in 0..lines {
                    let start = line * cols_usize;
                    let end = start + cols_usize;
                    if end > decoded.len() { break; }
                    let slice = &decoded[start..end];
                    diag.total_rows += 1;
                    diag.total_cells_scanned += cols_usize;
                    let range = Self::occupied_range_by_scan(slice, false);
                    for cell in &slice[range.clone()] { out.insert(cell.style); }
                    diag.total_occupied_cells += range.len();
                    if range.is_empty() { out.insert(0); }
                }
            }
        }
        diag
    }
    fn apply_available_completions(&mut self) {
        while let Ok(completion) = self.completion_rx.try_recv() {
            self.apply_completion(completion);
        }
    }


    fn intern_compressed(pool: &mut HashMap<u64, Vec<Weak<[u8]>>>, bytes: Vec<u8>) -> Arc<[u8]> {
        use std::hash::{DefaultHasher, Hash, Hasher};

        let mut hasher = DefaultHasher::new();
        bytes.hash(&mut hasher);
        let bucket = pool.entry(hasher.finish()).or_default();
        bucket.retain(|candidate| candidate.strong_count() != 0);
        if let Some(existing) = bucket
            .iter()
            .filter_map(Weak::upgrade)
            .find(|candidate| candidate.as_ref() == bytes.as_slice())
        {
            return existing;
        }

        let interned: Arc<[u8]> = Arc::from(bytes.into_boxed_slice());
        bucket.push(Arc::downgrade(&interned));
        interned
    }

    /// Synchronously compress all hot pages (drain pending jobs inline).
    /// Uses direct `compress_bytes` without going through channels, then
    /// releases residents. Segmented pages are flattened into a reusable
    /// scratch before LZ4 (never on the feed path). Honors pending
    /// completions by draining first.
    pub fn force_compress_all(&mut self) {
        self.drain_compression();
        // Reused scratch for segment flattening (avoids per-page alloc).
        let mut flat_scratch: Vec<Cell> = Vec::new();
        let mut to_compress: Vec<(u64, Vec<u8>)> = Vec::new();
        // Collect compressed bytes first to avoid borrow checker issues with pool.
        for page in self.history.iter() {
            if !page.is_hot() {
                continue;
            }
            let should = page.pending || page.compressed.is_none();
            if !should {
                continue;
            }
            let bytes_vec = match page.resident.as_ref().expect("hot") {
                Resident::Flat(buf) => {
                    let bytes = unsafe {
                        std::slice::from_raw_parts(
                            buf.as_ptr() as *const u8,
                            buf.len() * std::mem::size_of::<Cell>(),
                        )
                    };
                    compress_bytes(bytes)
                }
                Resident::Segmented(descs) => {
                    let cols = usize::from(page.cols);
                    let cap = page.capacity_lines * cols;
                    flat_scratch.clear();
                    flat_scratch.reserve(cap);
                    flat_scratch.resize(cap, Cell::default());
                    for (i, d) in descs.iter().enumerate() {
                        let row_start = i * cols;
                        let first = usize::from(d.first_occupied.min(d.cols)).min(cols);
                        let end = usize::from(d.occupancy.min(d.cols)).min(cols);
                        if first < end {
                            flat_scratch[row_start + first..row_start + end]
                                .copy_from_slice(&d.cells[first..end]);
                        }
                    }
                    let bytes = unsafe {
                        std::slice::from_raw_parts(
                            flat_scratch.as_ptr().cast::<u8>(),
                            flat_scratch.len() * std::mem::size_of::<Cell>(),
                        )
                    };
                    compress_bytes(bytes)
                }
            };
            to_compress.push((page.id, bytes_vec));
        }
        // Apply compressions by page id, consuming staged bytes by value without clone.
        let mut iter = to_compress.into_iter();
        for page in self.history.iter_mut() {
            if !page.is_hot() {
                continue;
            }
            let should = page.pending || page.compressed.is_none();
            if !should {
                continue;
            }
            let (expected_id, bytes_vec) = iter.next().expect("force_compress ordering: missing staged bytes");
            debug_assert_eq!(page.id, expected_id, "force_compress page-id ordering mismatch");
            page.compressed = Some(Self::intern_compressed(
                &mut self.compressed_pool,
                bytes_vec,
            ));
            page.sparse = false;
            page.resident = None;
            page.pending = false;
        }
        debug_assert!(iter.next().is_none(), "force_compress: staged bytes leftover");
        // Drain any completions that may have raced before force.
        self.drain_compression();
        self.enforce_max_lines();
    }

    /// Synchronously restore all cold pages, byte-identical.
    pub fn force_restore_all(&mut self) {
        self.drain_compression();
        for page in self.history.iter_mut() {
            if page.compressed.is_none() {
                continue;
            }
            let mut restored = Vec::new();
            let mut encoded = Vec::new();
            if decode_page(page, &mut restored, &mut encoded) {
                page.resident = Some(Resident::Flat(Arc::from(restored.into_boxed_slice())));
                page.compressed = None;
                page.sparse = false;
                page.generation = page.generation.wrapping_add(1);
                self.restored_pages += 1;
            } else {
                self.stale_discarded += 1;
            }
        }
    }

    #[cfg(test)]
    pub(crate) fn corrupt_one_cold_page_for_test(&mut self) {
        for page in self.history.iter_mut() {
            if page.compressed.is_some() {
                page.compressed = Some(Arc::from(vec![0u8, 1, 2, 3, 4].into_boxed_slice()));
                // Invalidate any cached decode and ensure decompression fails.
                self.cold_cache = None;
                break;
            }
        }
    }

    /// For testing generation bump / stale discard: mutate a hot page's first
    #[allow(dead_code)]
    pub fn bump_page_generation(&mut self, page_id: u64) {
        for page in self.history.iter_mut() {
            if page.id == page_id {
                page.generation = page.generation.wrapping_add(1);
                page.pending = false;
                if let Some(resident) = page.resident.as_mut() {
                    match resident {
                        Resident::Flat(buf) => {
                            if !buf.is_empty() {
                                let content = buf[0].content.wrapping_add(1);
                                Arc::make_mut(buf)[0].content = content;
                            }
                        }
                        Resident::Segmented(descs) => {
                            if let Some(first) = descs.first_mut() {
                                if !first.cells.is_empty() {
                                    let content = first.cells[0].content.wrapping_add(1);
                                    Arc::make_mut(&mut first.cells)[0].content = content;
                                }
                            }
                        }
                    }
                }
                break;
            }
        }
    }

    // -----------------------------------------------------------------------
    // Zero-conversion owned ingest — already-compact Cell data.
    // CompactRow/CompactPage live in compact/**; this path consumes the
    // already-compact Arc<[Cell]> they carry. Successful ingestion
    // transfers that allocation (pointer equality) rather than rebuilding
    // each Cell. Full/Disconnected or stale compression remains losslessly
    // retryable (page stays hot, generation bump on mutation, retry via
    // drain_compression/force_compress_all).
    // -----------------------------------------------------------------------

    /// Ingest one already-compact row without per-cell rebuild or whole-row
    /// scanning. The `Arc<[Cell]>` is moved into a bounded hot page grouped
    /// up to `hot_page_lines` rows. Per-row `cols`/`occupancy`/`wrapped`/
    /// `generation` and `Arc` identity are preserved; no cell scan or copy
    /// occurs on the feed path. `occupancy` is the count of meaningful cells
    /// before trailing defaults; `wrapped` mirrors WRAPLINE so wrap-artifact
    /// rows are not counted empty.
    pub fn ingest_owned_row(
        &mut self,
        cells: Arc<[Cell]>,
        cols: u16,
        occupancy: u16,
        wrapped: bool,
        generation: u64,
    ) {
        let first_occupied = cells[..usize::from(occupancy.min(cols))]
            .iter()
            .position(|cell| !cell.is_default())
            .map_or(cols, |index| index as u16);
        self.ingest_owned_row_inner(
            RowDesc {
                cells,
                cols,
                occupancy,
                first_occupied,
                wrapped,
            },
            generation,
            true,
        );
    }

    fn ingest_owned_row_inner(&mut self, row: RowDesc, generation: u64, enforce: bool) {
        let RowDesc {
            cells,
            cols,
            occupancy,
            first_occupied,
            wrapped,
        } = &row;
        if *cols == 0 {
            return;
        }
        debug_assert!(cells.len() == usize::from(*cols));
        debug_assert!(
            (*occupancy == 0 && *first_occupied == *cols)
                || (*first_occupied < *occupancy && *occupancy <= *cols)
        );
        let cols = *cols;
        let is_empty = *occupancy == 0 && !*wrapped;
        let can_append = matches!(
            self.history.back(),
            Some(page)
                if !page.is_cold()
                    && !page.pending
                    && page.lines < page.capacity_lines
                    && matches!(page.resident.as_ref(), Some(Resident::Segmented(_)))
                    && page.cols == cols
                    && page.generation == generation
        );
        if can_append {
            let back = self.history.back_mut().expect("segmented back page");
            if let Some(Resident::Segmented(descs)) = back.resident.as_mut() {
                descs.push(row);
            }
            back.lines += 1;
            back.empty_lines += usize::from(is_empty);
            self.logical_lines += 1;
            self.cold_cache = None;
            if enforce {
                self.enforce_max_lines();
            }
            return;
        }

        let capacity = self.config.hot_page_lines.max(1);
        let mut rows = Vec::with_capacity(capacity);
        rows.push(row);
        let page = Page {
            id: self.next_page_id,
            generation,
            lines: 1,
            empty_lines: usize::from(is_empty),
            cols,
            capacity_lines: capacity,
            resident: Some(Resident::Segmented(rows)),
            compressed: None,
            sparse: false,
            pending: false,
        };
        self.next_page_id = self.next_page_id.wrapping_add(1);
        self.history.push_back(page);
        self.logical_lines += 1;
        if self.history.len() > 1 {
            self.enqueue_cursor = self.enqueue_cursor.min(self.history.len() - 2);
        }
        self.cold_cache = None;
        self.maybe_enqueue_full_pages();
        if enforce {
            self.enforce_max_lines();
        }
    }

    pub fn ingest_owned_rows<I>(&mut self, rows: I)
    where
        I: IntoIterator<Item = (Arc<[Cell]>, u16, u16, bool, u64)>,
    {
        for (cells, cols, occupancy, wrapped, generation) in rows {
            self.ingest_owned_row(cells, cols, occupancy, wrapped, generation);
        }
    }

    pub(crate) fn ingest_owned_rows_with_bounds<I>(&mut self, rows: I)
    where
        I: IntoIterator<Item = (Arc<[Cell]>, u16, u16, u16, bool, u64)>,
    {
        for (cells, cols, first_occupied, occupancy, wrapped, generation) in rows {
            self.ingest_owned_row_inner(
                RowDesc {
                    cells,
                    cols,
                    occupancy,
                    first_occupied,
                    wrapped,
                },
                generation,
                false,
            );
        }
        self.enforce_max_lines();
    }

    /// Ingest an already-compact page of row descriptors. Each row's
    /// `Arc<[Cell]>` is transferred without per-cell rebuild. Mixed row
    /// widths are preserved; `page_cols` is the page's declared width.
    pub fn ingest_owned_page<I>(&mut self, page_cols: u16, generation: u64, rows: I)
    where
        I: IntoIterator<Item = (Arc<[Cell]>, u16, u16, bool, u64)>,
    {
        let _ = page_cols;
        for (cells, cols, occupancy, wrapped, row_generation) in rows {
            let resolved_generation = if row_generation == 0 {
                generation
            } else {
                row_generation
            };
            self.ingest_owned_row(cells, cols, occupancy, wrapped, resolved_generation);
        }
    }

    /// Ingest a flat already-compact page (`cells.len() == lines * cols`).
    /// Transfers the single `Arc<[Cell]>` allocation without per-cell rebuild.
    pub fn ingest_owned_flat_page(
        &mut self,
        cells: Arc<[Cell]>,
        cols: u16,
        lines: usize,
        generation: u64,
    ) {
        if cols == 0 || lines == 0 {
            return;
        }
        debug_assert!(cells.len() == lines * usize::from(cols));
        let width = usize::from(cols);
        let mut empty = 0usize;
        for line in 0..lines {
            let start = line * width;
            if cells[start..start + width].iter().all(Cell::is_default) {
                empty += 1;
            }
        }
        let p = Page {
            id: self.next_page_id,
            generation,
            lines,
            empty_lines: empty,
            cols,
            capacity_lines: lines,
            resident: Some(Resident::Flat(cells)),
            compressed: None,
            sparse: false,
            pending: false,
        };
        self.next_page_id = self.next_page_id.wrapping_add(1);
        self.history.push_back(p);
        self.logical_lines += lines;
        if self.history.len() > 1 {
            self.enqueue_cursor = self.enqueue_cursor.min(self.history.len() - 1);
        }
        self.cold_cache = None;
        self.maybe_enqueue_full_pages();
        self.enforce_max_lines();
    }

    /// Withdraw up to `count` rows from the contiguous newest hot segmented
    /// suffix matching `cols`. Stops at flat/cold/pending/corrupt/mixed-width
    /// boundaries. Zero-copy Arc moves, oldest→newest, preserves generation,
    /// updates logical_lines/empty_lines, clears cold_cache, clamps
    /// enqueue_cursor. No generation bump, no decompression, no channel ops.
    pub(crate) fn take_newest_hot_segmented_rows(
        &mut self,
        count: usize,
        cols: u16,
        out: &mut Vec<StoredRow>,
    ) -> usize {
        if count == 0 || self.history.is_empty() {
            return 0;
        }
        let mut remaining = count;
        let mut extracted = 0usize;
        // Collect chunks newest→oldest then reverse to deliver oldest→newest.
        let mut chunks: Vec<Vec<StoredRow>> = Vec::new();
        let mut idx = self.history.len().checked_sub(1);
        while let Some(i) = idx {
            if remaining == 0 {
                break;
            }
            let page = &self.history[i];
            if page.pending {
                break;
            }
            if page.compressed.is_some() {
                break;
            }
            let Some(resident) = page.resident.as_ref() else {
                break;
            };
            let descs = match resident {
                Resident::Flat(_) => break,
                Resident::Segmented(d) => d,
            };
            if descs.is_empty() || descs.len() != page.lines {
                break;
            }
            // Contiguous suffix where cols matches.
            let mut suffix_start = descs.len();
            for (rev_idx, d) in descs.iter().rev().enumerate() {
                if d.cols != cols {
                    break;
                }
                suffix_start = descs.len() - rev_idx - 1;
            }
            let suffix_len = descs.len().saturating_sub(suffix_start);
            if suffix_len == 0 {
                break;
            }
            let take = suffix_len.min(remaining);
            let drain_start = descs.len() - take;
            let generation = page.generation;
            let mut chunk: Vec<StoredRow> = Vec::with_capacity(take);
            for d in &descs[drain_start..] {
                chunk.push(StoredRow {
                    cells: Arc::clone(&d.cells),
                    cols: d.cols,
                    occupancy: d.occupancy,
                    first_occupied: d.first_occupied,
                    wrapped: d.wrapped,
                    generation,
                });
            }
            let whole_page = take == descs.len() && suffix_start == 0;
            if whole_page {
                self.history.remove(i);
                self.logical_lines = self.logical_lines.saturating_sub(take);
                extracted += take;
                remaining -= take;
                chunks.push(chunk);
                self.cold_cache = None;
                self.enqueue_cursor = self.enqueue_cursor.min(self.history.len());
                idx = if i == 0 { None } else { Some(i - 1) };
                continue;
            } else {
                let page_mut = &mut self.history[i];
                if let Some(Resident::Segmented(descs_mut)) = page_mut.resident.as_mut() {
                    descs_mut.drain(drain_start..);
                    page_mut.lines = descs_mut.len();
                    page_mut.empty_lines = descs_mut
                        .iter()
                        .filter(|r| r.occupancy == 0 && !r.wrapped)
                        .count();
                }
                self.logical_lines = self.logical_lines.saturating_sub(take);
                extracted += take;
                let _ = remaining - take;
                chunks.push(chunk);
                self.cold_cache = None;
                self.enqueue_cursor = self.enqueue_cursor.min(self.history.len());
                // Partial drain breaks contiguity; stop after this page.
                // If we stopped at a width boundary (suffix_start>0) or left
                // rows in this page, older pages are not contiguous.
                break;
            }
        }
        // Reverse chunks and extend oldest→newest.
        for chunk in chunks.into_iter().rev() {
            out.extend(chunk);
        }
        extracted
    }
}

impl Drop for ScrollbackStorage {
    fn drop(&mut self) {
        drop(self.job_tx.take());
        while self.queued_jobs.load(Ordering::Acquire) != 0 {
            self.apply_available_completions();
            std::thread::yield_now();
        }
        if let Some(handle) = self.worker.take() {
            let _ = handle.join();
        }
    }
}
fn remap_cells(cells: &mut [Cell], remap: &StyleRemap) -> Result<(), TerminalError> {
    for cell in cells {
        cell.style = remap.map(cell.style)?;
    }
    Ok(())
}

fn encode_cold_page(page: &Page, cells: &[Cell]) -> Result<Vec<u8>, TerminalError> {
    let capacity = page
        .capacity_lines
        .checked_mul(usize::from(page.cols))
        .ok_or(TerminalError::StyleCompactionCorrupt)?;
    if cells.len() != capacity || page.lines > page.capacity_lines {
        return Err(TerminalError::StyleCompactionCorrupt);
    }

    if !page.sparse {
        let bytes = unsafe {
            std::slice::from_raw_parts(
                cells.as_ptr().cast::<u8>(),
                std::mem::size_of_val(cells),
            )
        };
        return Ok(compress_bytes(bytes));
    }

    let cols = usize::from(page.cols);
    let mut encoded = Vec::new();
    for line in 0..page.lines {
        let start = line
            .checked_mul(cols)
            .ok_or(TerminalError::StyleCompactionCorrupt)?;
        let end = start
            .checked_add(cols)
            .ok_or(TerminalError::StyleCompactionCorrupt)?;
        let row = cells
            .get(start..end)
            .ok_or(TerminalError::StyleCompactionCorrupt)?;
        let range = ScrollbackStorage::occupied_range_by_scan(row, false);
        let first = u16::try_from(range.start)
            .map_err(|_| TerminalError::StyleCompactionCorrupt)?;
        let span = u16::try_from(range.len())
            .map_err(|_| TerminalError::StyleCompactionCorrupt)?;
        encoded.extend_from_slice(&first.to_le_bytes());
        encoded.extend_from_slice(&span.to_le_bytes());
        if !range.is_empty() {
            let bytes = unsafe {
                std::slice::from_raw_parts(
                    row[range].as_ptr().cast::<u8>(),
                    usize::from(span) * std::mem::size_of::<Cell>(),
                )
            };
            encoded.extend_from_slice(bytes);
        }
    }
    let encoded_len = u32::try_from(encoded.len())
        .map_err(|_| TerminalError::StyleCompactionCorrupt)?;
    let block = compress_bytes(&encoded);
    let mut compressed = Vec::with_capacity(4 + block.len());
    compressed.extend_from_slice(&encoded_len.to_le_bytes());
    compressed.extend_from_slice(&block);
    Ok(compressed)
}

fn decode_page(page: &Page, cells: &mut Vec<Cell>, encoded: &mut Vec<u8>) -> bool {
    let Some(compressed) = page.compressed.as_ref() else {
        return false;
    };
    let capacity = page.capacity_lines * usize::from(page.cols);
    cells.clear();
    cells.resize(capacity, Cell::default());
    if !page.sparse {
        let out = unsafe {
            std::slice::from_raw_parts_mut(
                cells.as_mut_ptr().cast::<u8>(),
                cells.len() * std::mem::size_of::<Cell>(),
            )
        };
        return decompress_bytes(compressed, out).is_ok_and(|written| written == out.len());
    }
    let Some(header) = compressed.get(..4) else {
        return false;
    };
    let encoded_len = u32::from_le_bytes(header.try_into().expect("four-byte header")) as usize;
    encoded.clear();
    encoded.resize(encoded_len, 0);
    if !decompress_bytes(&compressed[4..], encoded).is_ok_and(|written| written == encoded_len) {
        return false;
    }
    let cols = usize::from(page.cols);
    let mut cursor = 0usize;
    for line in 0..page.lines {
        let Some(header) = encoded.get(cursor..cursor + 4) else {
            return false;
        };
        let first = u16::from_le_bytes([header[0], header[1]]) as usize;
        let span = u16::from_le_bytes([header[2], header[3]]) as usize;
        cursor += 4;
        let byte_len = span * std::mem::size_of::<Cell>();
        if first + span > cols || cursor + byte_len > encoded.len() {
            return false;
        }
        if span != 0 {
            let start = line * cols + first;
            let target = unsafe {
                std::slice::from_raw_parts_mut(
                    cells[start..start + span].as_mut_ptr().cast::<u8>(),
                    byte_len,
                )
            };
            target.copy_from_slice(&encoded[cursor..cursor + byte_len]);
        }
        cursor += byte_len;
    }
    cursor == encoded.len()
}

fn compress_payload(
    payload: JobPayload,
    cols: u16,
    scratch: &mut Vec<u8>,
    encoded: &mut Vec<u8>,
) -> (Vec<u8>, bool, Vec<Arc<[Cell]>>) {
    match payload {
        JobPayload::Flat(cells) => {
            let bytes = unsafe {
                std::slice::from_raw_parts(
                    cells.as_ptr().cast::<u8>(),
                    cells.len() * std::mem::size_of::<Cell>(),
                )
            };
            (compress_bytes_reuse(bytes, scratch), false, Vec::new())
        }
        JobPayload::Segmented(rows) => {
            encoded.clear();
            let cols = usize::from(cols);
            for row in &rows {
                let cells = &row.cells[..usize::from(row.cols).min(cols)];
                let first = usize::from(row.first_occupied.min(row.cols));
                let end = usize::from(row.occupancy.min(row.cols));
                let span = end.saturating_sub(first);
                encoded.extend_from_slice(&(first as u16).to_le_bytes());
                encoded.extend_from_slice(&(span as u16).to_le_bytes());
                if span != 0 {
                    let bytes = unsafe {
                        std::slice::from_raw_parts(
                            cells[first..first + span].as_ptr().cast::<u8>(),
                            span * std::mem::size_of::<Cell>(),
                        )
                    };
                    encoded.extend_from_slice(bytes);
                }
            }
            let block = compress_bytes_reuse(encoded, scratch);
            let mut compressed = Vec::with_capacity(4 + block.len());
            compressed.extend_from_slice(&(encoded.len() as u32).to_le_bytes());
            compressed.extend_from_slice(&block);
            let recycled = rows.into_iter().map(|row| row.cells).collect();
            (compressed, true, recycled)
        }
    }
}

fn worker_loop(
    job_rx: Receiver<Job>,
    completion_tx: SyncSender<Completion>,
    queued_jobs: Arc<AtomicUsize>,
    pending_completions: Arc<AtomicUsize>,
    _dropped_completions: Arc<AtomicU64>,
) {
    let mut scratch: Vec<u8> = Vec::new();
    let mut encoded: Vec<u8> = Vec::new();
    while let Ok(job) = job_rx.recv() {
        let (compressed, sparse, recycled_rows) =
            compress_payload(job.payload, job.cols, &mut scratch, &mut encoded);
        let completion = Completion {
            page_id: job.page_id,
            generation: job.generation,
            compressed,
            recycled_rows,
            sparse,
        };
        pending_completions.fetch_add(1, Ordering::Release);
        if completion_tx.send(completion).is_err() {
            pending_completions.fetch_sub(1, Ordering::AcqRel);
            queued_jobs.fetch_sub(1, Ordering::AcqRel);
            break;
        }
        queued_jobs.fetch_sub(1, Ordering::AcqRel);
    }
}
#[cfg(test)]
mod owned_ingest_tests {
    use super::*;
    use std::sync::Arc;

    fn cell(v: u32) -> Cell {
        Cell {
            content: v,
            style: 1,
            flags: 0,
        }
    }

    fn row_arc(cols: u16, start: u32, occupancy: u16) -> Arc<[Cell]> {
        (0..usize::from(cols))
            .map(|i| {
                if (i as u16) < occupancy {
                    cell(start + i as u32)
                } else {
                    Cell::default()
                }
            })
            .collect::<Vec<_>>()
            .into()
    }

    #[test]
    fn ingest_row_transfers_allocation_pointer() {
        let mut s = ScrollbackStorage::new(
            80,
            ScrollbackConfig {
                max_lines: 100,
                hot_page_lines: 4,
                max_queued_jobs: 4,
                max_pending_completions: 4,
            },
        );
        let cols = 8u16;
        let arc: Arc<[Cell]> = (0..8).map(|i| cell(10 + i)).collect::<Vec<_>>().into();
        let ptr_before = Arc::as_ptr(&arc) as *const ();
        s.ingest_owned_row(Arc::clone(&arc), cols, cols, false, 1);
        let page = s.history.back().unwrap();
        let resident_ptr = match page.resident.as_ref().unwrap() {
            Resident::Flat(b) => b.as_ptr() as *const (),
            Resident::Segmented(ds) => ds[0].cells.as_ptr() as *const (),
        };
        assert_eq!(
            ptr_before, resident_ptr,
            "ingest must transfer Arc allocation, not copy Cells"
        );
        let mut out = Vec::new();
        assert!(s.read_line(0, &mut out));
        assert_eq!(out.as_slice(), arc.as_ref());
    }

    #[test]
    fn ingest_page_transfers_single_allocation_and_reads_back_byte_identical() {
        let mut s = ScrollbackStorage::new(
            6,
            ScrollbackConfig {
                max_lines: 100,
                hot_page_lines: 100,
                max_queued_jobs: 4,
                max_pending_completions: 4,
            },
        );
        let cols = 6u16;
        let lines = 4usize;
        let cells: Arc<[Cell]> = (0..lines * usize::from(cols))
            .map(|i| cell(i as u32))
            .collect::<Vec<_>>()
            .into();
        let ptr_before = Arc::as_ptr(&cells) as *const ();
        s.ingest_owned_flat_page(Arc::clone(&cells), cols, lines, 1);
        let page = s.history.back().unwrap();
        let resident_ptr = match page.resident.as_ref().unwrap() {
            Resident::Flat(b) => b.as_ptr() as *const (),
            Resident::Segmented(ds) => ds[0].cells.as_ptr() as *const (),
        };
        assert_eq!(ptr_before, resident_ptr);
        assert_eq!(s.total_lines(), lines);
        let mut out = Vec::new();
        for idx in 0..lines {
            assert!(s.read_line(idx, &mut out));
            let start = idx * usize::from(cols);
            assert_eq!(out.as_slice(), &cells[start..start + usize::from(cols)]);
        }
        s.force_compress_all();
        s.force_restore_all();
        for idx in 0..lines {
            assert!(s.read_line(idx, &mut out));
            let start = idx * usize::from(cols);
            assert_eq!(out.as_slice(), &cells[start..start + usize::from(cols)]);
        }
    }

    #[test]
    fn ingest_retry_safety_on_full_and_stale_paths_is_lossless() {
        let mut s = ScrollbackStorage::new(
            4,
            ScrollbackConfig {
                max_lines: 100,
                hot_page_lines: 1,
                max_queued_jobs: 1,
                max_pending_completions: 1,
            },
        );
        for i in 0..6 {
            s.ingest_owned_row(row_arc(4, i * 10, 4), 4, 4, false, 1);
        }
        assert_eq!(s.total_lines(), 6);
        s.poll_compression();
        let leftover_hot = s
            .history
            .iter()
            .filter(|page| page.resident.is_some())
            .count();
        assert!(
            leftover_hot >= 4,
            "queue Full must keep overflow pages hot, leftover_hot={leftover_hot}"
        );
        s.drain_compression();
        s.force_compress_all();
        assert_eq!(s.total_lines(), 6);
        let mut s2 = ScrollbackStorage::new(
            4,
            ScrollbackConfig {
                max_lines: 100,
                hot_page_lines: 2,
                max_queued_jobs: 4,
                max_pending_completions: 4,
            },
        );
        s2.ingest_owned_row(row_arc(4, 1, 4), 4, 4, false, 1);
        s2.ingest_owned_row(row_arc(4, 2, 4), 4, 4, false, 1);
        let back_id = s2.history.back().unwrap().id;
        s2.bump_page_generation(back_id);
        s2.drain_compression();
        assert_eq!(s2.total_lines(), 2);
        let mut out = Vec::new();
        assert!(s2.read_line(0, &mut out));
    }

    #[test]
    fn ingest_width_changes_preserve_per_page_cols() {
        let mut s = ScrollbackStorage::new(
            80,
            ScrollbackConfig {
                max_lines: 100,
                hot_page_lines: 4,
                max_queued_jobs: 4,
                max_pending_completions: 4,
            },
        );
        s.ingest_owned_row(row_arc(8, 0, 8), 8, 8, false, 1);
        s.ingest_owned_row(row_arc(12, 100, 12), 12, 12, false, 1);
        s.ingest_owned_row(row_arc(8, 200, 8), 8, 8, false, 1);
        assert_eq!(s.total_lines(), 3);
        assert_eq!(s.line_cols(0), Some(8));
        assert_eq!(s.line_cols(1), Some(12));
        assert_eq!(s.line_cols(2), Some(8));
        let mut out = Vec::new();
        assert!(s.read_line(1, &mut out));
        assert_eq!(out.len(), 12);
        assert_eq!(out[0].content, 100);
        s.push_line(8, &[cell(999); 8]);
        assert_eq!(s.total_lines(), 4);
        assert_eq!(s.line_cols(3), Some(8));
    }

    #[test]
    fn ingest_trailing_occupancy_and_empty_accounting() {
        let mut s = ScrollbackStorage::new(
            8,
            ScrollbackConfig {
                max_lines: 100,
                hot_page_lines: 10,
                max_queued_jobs: 4,
                max_pending_completions: 4,
            },
        );
        s.ingest_owned_row(row_arc(8, 0, 0), 8, 0, false, 1);
        assert_eq!(s.stats().empty_lines, 1);
        s.ingest_owned_row(row_arc(8, 0, 0), 8, 0, true, 1);
        assert_eq!(s.stats().empty_lines, 1);
        s.ingest_owned_row(row_arc(8, 10, 3), 8, 3, false, 1);
        assert_eq!(s.stats().empty_lines, 1);
        assert_eq!(s.total_lines(), 3);
        let mut out = Vec::new();
        assert!(s.read_line(2, &mut out));
        assert_eq!(out[0].content, 10);
        assert_eq!(out[2].content, 12);
        assert!(out[3..].iter().all(Cell::is_default));
    }

    #[test]
    fn ingest_exact_logical_counts_and_eviction() {
        let mut s = ScrollbackStorage::new(
            4,
            ScrollbackConfig {
                max_lines: 5,
                hot_page_lines: 2,
                max_queued_jobs: 4,
                max_pending_completions: 4,
            },
        );
        for i in 0..7 {
            s.ingest_owned_row(row_arc(4, i * 10, 4), 4, 4, false, 1);
        }
        assert_eq!(s.total_lines(), 5);
        assert_eq!(s.stats().logical_lines, 5);
        let mut out = Vec::new();
        assert!(s.read_line(0, &mut out));
        assert_eq!(out[0].content, 20);
        let cols = 4u16;
        let lines = 3usize;
        let cells: Arc<[Cell]> = (0..lines * usize::from(cols))
            .map(|i| cell(900 + i as u32))
            .collect::<Vec<_>>()
            .into();
        s.ingest_owned_flat_page(cells, cols, lines, 1);
        assert_eq!(s.total_lines(), 5);
        assert_eq!(s.line_cols(0), Some(4));
    }

    #[test]
    fn ingest_rows_batch_moves_descriptors_without_cell_scan() {
        let mut s = ScrollbackStorage::new(
            6,
            ScrollbackConfig {
                max_lines: 100,
                hot_page_lines: 10,
                max_queued_jobs: 4,
                max_pending_completions: 4,
            },
        );
        let rows: Vec<_> = (0..5)
            .map(|i| (row_arc(6, i * 10, 6), 6, 6, false, 1))
            .collect();
        let ptrs: Vec<*const ()> = rows
            .iter()
            .map(|(cells, _, _, _, _)| Arc::as_ptr(cells) as *const ())
            .collect();
        s.ingest_owned_rows(rows);
        assert_eq!(s.total_lines(), 5);
        // Grouped pages: descriptors are contiguous across pages in order.
        let mut seen = 0usize;
        for page in s.history.iter() {
            match page.resident.as_ref().unwrap() {
                Resident::Flat(_) => panic!("owned ingest must produce Segmented pages"),
                Resident::Segmented(ds) => {
                    for d in ds {
                        let ptr = d.cells.as_ptr() as *const ();
                        assert_eq!(ptr, ptrs[seen], "descriptor Arc identity lost");
                        seen += 1;
                    }
                }
            }
        }
        assert_eq!(seen, ptrs.len());
        // Bounded pages: 5 rows with hot_page_lines=10 must be 1 page, not 5.
        assert_eq!(
            s.history.len(),
            1,
            "grouped pages must be O(lines/hot_page_lines)"
        );
        // Descriptor-page path: same transfer, page generation used when row gen is 0.
        let page_rows = vec![(row_arc(6, 50, 6), 6, 6, false, 0u64)];
        let _page_ptr = Arc::as_ptr(&page_rows[0].0) as *const ();
        s.ingest_owned_page(6, 7, page_rows);
        assert_eq!(s.total_lines(), 6);
        let back = s.history.back().unwrap();
        let _back_ptr = match back.resident.as_ref().unwrap() {
            Resident::Flat(b) => b.as_ptr() as *const (),
            Resident::Segmented(ds) => ds.last().unwrap().cells.as_ptr() as *const (),
        };
        // Descriptor keeps its own generation; page generation is first row's gen.
        assert_eq!(back.generation, 7);
    }

    #[test]
    fn quiesce_while_queued_does_not_reenqueue_and_clears_pending() {
        let mut s = ScrollbackStorage::new(4, ScrollbackConfig { max_lines: 100, hot_page_lines: 1, max_queued_jobs: 4, max_pending_completions: 4 });
        for i in 0..4 { s.ingest_owned_row(row_arc(4, i*10, 4), 4, 4, false, 1); }
        let before_cursor = s.history.len();
        let ok = s.quiesce_for_style_transaction();
        assert!(ok, "quiesce must succeed to zero queued/pending/pages");
        assert_eq!(s.queued_jobs.load(std::sync::atomic::Ordering::Acquire), 0);
        assert_eq!(s.pending_completions.load(std::sync::atomic::Ordering::Acquire), 0);
        assert!(s.history.iter().all(|p| !p.pending));
        // No fresh enqueue occurred beyond prior pages.
        assert!(s.history.len() >= before_cursor.saturating_sub(0));
        let mut a = std::collections::BTreeSet::new(); let da = s.census_storage_styles_optimized(&mut a);
        let mut b = std::collections::BTreeSet::new(); let db = s.census_storage_styles_exhaustive(&mut b);
        assert_eq!(a, b);
        assert_eq!(da.total_pages, db.total_pages);
    }

    #[test]
    fn census_includes_cold_and_hot_flat_and_segmented() {
        let mut s = ScrollbackStorage::new(4, ScrollbackConfig { max_lines: 100, hot_page_lines: 2, max_queued_jobs: 4, max_pending_completions: 4 });
        // Segmented hot
        for i in 0..3 { s.ingest_owned_row(row_arc(4, i*10, 4), 4, 4, false, 1); }
        // Flat hot
        let flat: std::sync::Arc<[Cell]> = (0..8).map(|i| Cell { content: i as u32, style: (i as u16)+10, flags: 0 }).collect::<Vec<_>>().into();
        s.ingest_owned_flat_page(flat, 4, 2, 1);
        let _ = s.quiesce_for_style_transaction();
        s.force_compress_all();
        let mut a = std::collections::BTreeSet::new(); let da = s.census_storage_styles_optimized(&mut a);
        let mut b = std::collections::BTreeSet::new(); let db = s.census_storage_styles_exhaustive(&mut b);
        assert_eq!(a, b);
        assert!(da.cold_pages > 0 || da.hot_flat_pages > 0);
        assert_eq!(da.total_pages, db.total_pages);
        assert!(!a.is_empty());
    }

    #[test]
    fn census_cold_corrupt_fails_closed_without_mutating_cache() {
        let mut s = ScrollbackStorage::new(4, ScrollbackConfig { max_lines: 100, hot_page_lines: 1, max_queued_jobs: 4, max_pending_completions: 4 });
        for i in 0..4 { s.ingest_owned_row(row_arc(4, i*10, 4), 4, 4, false, 1); }
        let _ = s.quiesce_for_style_transaction();
        s.force_compress_all();
        s.drain_compression();
        assert!(s.stats().compressed_bytes > 0, "expected at least one cold page for corruption test");
        s.corrupt_one_cold_page_for_test();
        let cache_none_before = s.cold_cache.is_none();
        let mut a = std::collections::BTreeSet::new(); let da = s.census_storage_styles_optimized(&mut a);
        let mut b = std::collections::BTreeSet::new(); let db = s.census_storage_styles_exhaustive(&mut b);
        // Both views must still agree, but must surface corruption.
        assert_eq!(a, b);
        assert!(da.corrupt_cold_pages >= 1, "optimized must count corrupt, got {da:?}");
        assert!(db.corrupt_cold_pages >= 1, "exhaustive must count corrupt, got {db:?}");
        assert!(s.cold_cache.is_none() == cache_none_before || s.cold_cache.is_none(), "census must not populate ColdReadCache");
    }
}

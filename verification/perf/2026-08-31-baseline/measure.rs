//! Headless Instant timings for Gate 5 Wave 1.
//!
//! Compiles as an out-of-tree binary (see run.sh). Does not live in the
//! workspace, so it never touches repo Cargo.toml / Cargo.lock.
//!
//! Reachable now:
//! - `EffectsModel::apply_frame` (public). Product `collect_reveals` and
//!   private `ChangeTracker::repack` run inside it; they are not separately
//!   named in these numbers.
//! - A byte-faithful public clone of the `collect_reveals` loop using
//!   `ChangeTracker::{tracked_cells,cols,bits_at,change_ms_at}`. That is
//!   not the private function; Lane I cannot treat this as a delta against
//!   the inlined body until the in-crate bench lands.
//!
//! Unreachable without a crate-private bench:
//! - `collect_reveals` itself (`model.rs` crate-private).
//! - `ChangeTracker::repack` (`key.rs` private).

use mr_crabs_config::TextAnimation;
use mr_crabs_effects::{
    CellPx, ChangeTracker, EffectsConfig, EffectsModel, TypewriterSchedule,
};
use mr_crabs_terminal::{Cell, CursorState, DamageKind, FrameDelta, GridSize, RowDelta};
use std::time::Instant;

const WARMUP: usize = 50;
const ITERS: usize = 2_000;
const LARGE_ITERS: usize = 400;
const NEVER_MS: f64 = -1_000_000.0;
const NEVER_BITS: u32 = f32::to_bits((NEVER_MS / 1000.0) as f32);

fn cell(content: u32) -> Cell {
    Cell {
        content,
        style: 0,
        flags: 0,
    }
}

fn row(row: u16, generation: u64, contents: &[u32]) -> RowDelta {
    RowDelta {
        row,
        generation,
        cells: contents.iter().copied().map(cell).collect(),
        runs: Vec::new(),
        combining: Vec::new(),
    }
}

fn frame_at(size: GridSize, seq: u64, rows: Vec<RowDelta>, cursor: CursorState) -> FrameDelta {
    let mut f = FrameDelta::empty(size);
    f.sequence = seq;
    f.damage = if rows.is_empty() {
        DamageKind::Clean
    } else {
        DamageKind::Partial
    };
    f.rows = rows;
    f.cursor = cursor;
    f
}

fn filled_row(cols: u16, row_i: u16, generation: u64) -> RowDelta {
    let cells: Vec<u32> = (0..cols)
        .map(|c| 65 + u32::from(c % 26) + u32::from(row_i % 3))
        .collect();
    row(row_i, generation, &cells)
}

fn filled_rows(size: GridSize, generation: u64) -> Vec<RowDelta> {
    (0..size.rows)
        .map(|r| filled_row(size.cols, r, generation))
        .collect()
}

fn config(mode: TextAnimation, trail: bool, max_cells: usize) -> EffectsConfig {
    EffectsConfig::new(mode, 120, 1.0, trail, 0.35, 250, max_cells)
}

struct Sample {
    name: &'static str,
    ns: Vec<u128>,
    note: &'static str,
}

impl Sample {
    fn report(&self) {
        let mut v = self.ns.clone();
        v.sort_unstable();
        let n = v.len() as f64;
        let sum: u128 = v.iter().sum();
        let mean = sum as f64 / n;
        let var = v.iter().map(|&x| {
            let d = x as f64 - mean;
            d * d
        }).sum::<f64>() / n;
        let std = var.sqrt();
        let p = |q: f64| -> u128 {
            let idx = ((q / 100.0) * (n - 1.0)).round() as usize;
            v[idx.min(v.len() - 1)]
        };
        println!(
            "name={} n={} mean_ns={:.1} std_ns={:.1} min_ns={} p50_ns={} p95_ns={} p99_ns={} max_ns={} note={}",
            self.name,
            v.len(),
            mean,
            std,
            v[0],
            p(50.0),
            p(95.0),
            p(99.0),
            v[v.len() - 1],
            self.note
        );
    }
}

fn time_loop<F: FnMut()>(name: &'static str, warmup: usize, iters: usize, note: &'static str, mut body: F) -> Sample {
    for _ in 0..warmup {
        body();
    }
    let mut ns = Vec::with_capacity(iters);
    for _ in 0..iters {
        let t = Instant::now();
        body();
        ns.push(t.elapsed().as_nanos());
    }
    Sample { name, ns, note }
}

/// Public clone of `collect_reveals` at model.rs:389-414.
fn collect_reveals_public(
    tracker: &ChangeTracker,
    mode: TextAnimation,
    duration_ms: f64,
    now: f64,
    revealing: &mut Vec<(u16, u16, f64, f64)>,
    pending: &mut Vec<(u16, u16)>,
) {
    let cols = tracker.cols();
    for i in 0..tracker.tracked_cells() {
        let bits = tracker.bits_at(i);
        if bits == NEVER_BITS {
            continue;
        }
        let change_ms = tracker.change_ms_at(i);
        let elapsed = now - change_ms;
        if elapsed >= duration_ms {
            continue;
        }
        let r = (i / cols) as u16;
        let c = (i % cols) as u16;
        if mode == TextAnimation::Typewriter && elapsed < 0.0 {
            pending.push((r, c));
        } else {
            revealing.push((r, c, change_ms, elapsed));
        }
    }
}

fn stamp_full_grid(tracker: &mut ChangeTracker, size: GridSize, generation: u64, now: f64) {
    let mut schedule = TypewriterSchedule::new(0.0);
    for r in 0..size.rows {
        let rd = filled_row(size.cols, r, generation);
        tracker.update_row(r, generation, &rd.cells, now, &mut schedule);
    }
}

fn main() {
    let sha = std::env::var("MR_CRABS_SHA").unwrap_or_else(|_| "unknown".into());
    let run_id = std::env::args().nth(1).unwrap_or_else(|| "1".into());
    println!("sha={sha} run={run_id} warmup={WARMUP} iters={ITERS} large_iters={LARGE_ITERS}");

    let small = GridSize::new(80, 24);
    let large = GridSize::new(200, 60);
    let cell_px = CellPx::new(8.0, 16.0);
    let cursor = CursorState::default();

    let mut samples = Vec::new();

    {
        samples.push(time_loop(
            "apply_frame_80x24_first_stamp_streaming_trail",
            10,
            200,
            "new model each sample; dummy Clean then Full stamp so tracker stamps, repacks, collect_reveals, trail",
            || {
                let mut model = EffectsModel::new(
                    config(TextAnimation::Streaming, true, 1 << 20),
                    small,
                    cell_px,
                );
                let _ = model.apply_frame(&frame_at(small, 1, Vec::new(), cursor), 900, true);
                let mut stamp = frame_at(small, 2, filled_rows(small, 1), cursor);
                stamp.damage = DamageKind::Full;
                let _ = model.apply_frame(&stamp, 1000, true);
            },
        ));
    }

    {
        let mut model = EffectsModel::new(config(TextAnimation::Streaming, false, 1 << 20), small, cell_px);
        let _ = model.apply_frame(&frame_at(small, 1, Vec::new(), cursor), 900, true);
        let mut stamp = frame_at(small, 2, filled_rows(small, 1), cursor);
        stamp.damage = DamageKind::Full;
        let _ = model.apply_frame(&stamp, 1000, true);
        let idle = frame_at(small, 3, Vec::new(), cursor);
        samples.push(time_loop(
            "apply_frame_80x24_clean_mid_window",
            WARMUP,
            ITERS,
            "Full stamp then Clean; no stamp/repack on measured frames; collect_reveals walks 1920 cells",
            || {
                let _ = model.apply_frame(&idle, 1060, true);
            },
        ));
    }

    {
        let mut model = EffectsModel::new(config(TextAnimation::Streaming, false, 1 << 20), small, cell_px);
        let _ = model.apply_frame(&frame_at(small, 1, Vec::new(), cursor), 900, true);
        let mut stamp = frame_at(small, 2, filled_rows(small, 1), cursor);
        stamp.damage = DamageKind::Full;
        let _ = model.apply_frame(&stamp, 1000, true);
        let idle = frame_at(small, 3, Vec::new(), cursor);
        samples.push(time_loop(
            "apply_frame_80x24_clean_after_expiry",
            WARMUP,
            ITERS,
            "elapsed>=duration so collect_reveals walks all and emits none",
            || {
                let _ = model.apply_frame(&idle, 2000, true);
            },
        ));
    }

    {
        let mut model = EffectsModel::new(config(TextAnimation::Streaming, true, 1 << 20), large, cell_px);
        let _ = model.apply_frame(&frame_at(large, 1, Vec::new(), cursor), 900, true);
        let mut stamp = frame_at(large, 2, filled_rows(large, 1), cursor);
        stamp.damage = DamageKind::Full;
        let _ = model.apply_frame(&stamp, 1000, true);
        let idle = frame_at(large, 3, Vec::new(), cursor);
        samples.push(time_loop(
            "apply_frame_200x60_clean_mid_window",
            20,
            LARGE_ITERS,
            "12000 tracked cells; Clean frame; collect_reveals plus trail; no stamp/repack",
            || {
                let _ = model.apply_frame(&idle, 1060, true);
            },
        ));
    }

    {
        let mut tracker = ChangeTracker::new(80, 24, 1 << 20);
        stamp_full_grid(&mut tracker, small, 1, 1000.0);
        let mut revealing = Vec::with_capacity(1920);
        let mut pending = Vec::new();
        samples.push(time_loop(
            "collect_reveals_public_clone_80x24_hot",
            WARMUP,
            ITERS,
            "NOT the private fn; same loop over bits_at/change_ms_at; all cells in window",
            || {
                revealing.clear();
                pending.clear();
                collect_reveals_public(
                    &tracker,
                    TextAnimation::Streaming,
                    120.0,
                    1060.0,
                    &mut revealing,
                    &mut pending,
                );
                std::hint::black_box(revealing.len());
            },
        ));
    }

    {
        let mut tracker = ChangeTracker::new(200, 60, 1 << 20);
        stamp_full_grid(&mut tracker, large, 1, 1000.0);
        let mut revealing = Vec::with_capacity(12_000);
        let mut pending = Vec::new();
        samples.push(time_loop(
            "collect_reveals_public_clone_200x60_hot",
            20,
            LARGE_ITERS,
            "NOT the private fn; 12000-cell scan",
            || {
                revealing.clear();
                pending.clear();
                collect_reveals_public(
                    &tracker,
                    TextAnimation::Streaming,
                    120.0,
                    1060.0,
                    &mut revealing,
                    &mut pending,
                );
                std::hint::black_box(revealing.len());
            },
        ));
    }

    {
        let mut tracker = ChangeTracker::new(80, 24, 1 << 20);
        stamp_full_grid(&mut tracker, small, 1, 1000.0);
        let mut revealing = Vec::new();
        let mut pending = Vec::new();
        samples.push(time_loop(
            "collect_reveals_public_clone_80x24_expired",
            WARMUP,
            ITERS,
            "all elapsed>=duration; still walks every cell",
            || {
                revealing.clear();
                pending.clear();
                collect_reveals_public(
                    &tracker,
                    TextAnimation::Streaming,
                    120.0,
                    2000.0,
                    &mut revealing,
                    &mut pending,
                );
                std::hint::black_box(revealing.len());
            },
        ));
    }

    {
        let mut model = EffectsModel::new(
            config(TextAnimation::Disabled, false, 1 << 20),
            small,
            cell_px,
        );
        let idle = frame_at(small, 1, Vec::new(), cursor);
        samples.push(time_loop(
            "apply_frame_80x24_disabled",
            WARMUP,
            ITERS,
            "no tracker so collect_reveals never runs; floor for apply_frame",
            || {
                let _ = model.apply_frame(&idle, 1000, true);
            },
        ));
    }

    for s in &samples {
        s.report();
    }
}

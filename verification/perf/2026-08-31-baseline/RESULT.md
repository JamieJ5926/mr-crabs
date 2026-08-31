# Lane F: frame-timing baseline

Status: PASS. Output is untracked. No `crates/` writes. No git mutation.

SHA: `537d5bed16438f2370e8ef64850a6a21972ab34a` (`main`).

Host: Darwin 25.5.0 arm64 Apple M4 Pro. `CARGO_TARGET_DIR=/Users/jamie/.cargo-target`. Release binary, `std::time::Instant`.

Command (rerun twice, both captured):

```
verification/perf/2026-08-31-baseline/run.sh 1
verification/perf/2026-08-31-baseline/run.sh 2
```

`run.sh` copies `measure.rs` into a temp crate outside the workspace, so repo `Cargo.toml` and `Cargo.lock` stay untouched. Someone who did not write this lane runs the same two lines from the repo root.

## Reachability

Reached.

- `EffectsModel::apply_frame` (public). Product `collect_reveals` (`model.rs:389`, single call at `model.rs:309`) and private `ChangeTracker::repack` (`key.rs:494`) execute inside it. Their cost is mixed into `apply_frame_*` samples, not named separately.
- Public clone of the `collect_reveals` loop via `ChangeTracker::{tracked_cells, cols, bits_at, change_ms_at}`. Same control flow as `model.rs:397-413`. It is not the private function. Lane I must not treat these numbers as a delta against a later inlined body.

Not reached.

- `collect_reveals` the crate-private function. No public wrapper, no `#[cfg(test)]` timer, no in-crate bench. An out-of-tree binary cannot name it.
- `ChangeTracker::repack`. Private. Lane H notes it packs every tracked cell into unused rgba8 bytes on every stamp. It only fires when timestamps change, so it is inside `apply_frame_80x24_first_stamp_*` and absent from the Clean-frame samples.

Existing in-tree timing: `mr-crabs-bench` already times `apply_frame` in `effects_workload` (`workloads.rs:1276-1384`) as `frame_build_{mean,p50,p95,p99}_ns`. That path feeds the terminal then applies frames. It does not isolate `collect_reveals` or `repack`. No `[[bench]]` target exists. No Criterion in any `Cargo.toml`. The workspace already has Instant-based benches; Gate 5 should match that, not add Criterion (that would rewrite `Cargo.lock`, which Gate 1 checks).

## Numbers

Times in nanoseconds. Two independent process runs. `n` is per-sample Instant intervals after warmup.

| name | run | n | mean | std | min | p50 | p95 | p99 | max |
|---|---|---|---|---|---|---|---|---|---|
| apply_frame_80x24_first_stamp_streaming_trail | 1 | 200 | 47317.5 | 2722.4 | 45167 | 46583 | 49834 | 56708 | 78458 |
| apply_frame_80x24_first_stamp_streaming_trail | 2 | 200 | 46091.9 | 3469.2 | 43416 | 45167 | 48333 | 63125 | 75916 |
| apply_frame_80x24_clean_mid_window | 1 | 2000 | 1675.4 | 1672.3 | 1292 | 1583 | 1708 | 4167 | 73541 |
| apply_frame_80x24_clean_mid_window | 2 | 2000 | 1612.8 | 504.4 | 1333 | 1583 | 1667 | 1959 | 14917 |
| apply_frame_80x24_clean_after_expiry | 1 | 2000 | 975.9 | 26.6 | 916 | 959 | 1041 | 1083 | 1125 |
| apply_frame_80x24_clean_after_expiry | 2 | 2000 | 961.3 | 191.0 | 791 | 1000 | 1042 | 1084 | 8750 |
| apply_frame_200x60_clean_mid_window | 1 | 400 | 10268.1 | 686.2 | 9917 | 10250 | 10458 | 10709 | 23625 |
| apply_frame_200x60_clean_mid_window | 2 | 400 | 10304.3 | 952.3 | 9916 | 10250 | 10458 | 10834 | 25375 |
| collect_reveals_public_clone_80x24_hot | 1 | 2000 | 1792.2 | 180.5 | 1583 | 1792 | 1875 | 1875 | 9250 |
| collect_reveals_public_clone_80x24_hot | 2 | 2000 | 1802.3 | 65.1 | 1583 | 1792 | 1875 | 1917 | 2917 |
| collect_reveals_public_clone_200x60_hot | 1 | 400 | 11507.2 | 1334.5 | 11083 | 11333 | 11708 | 16708 | 31416 |
| collect_reveals_public_clone_200x60_hot | 2 | 400 | 11405.3 | 207.6 | 11042 | 11416 | 11583 | 11875 | 14500 |
| collect_reveals_public_clone_80x24_expired | 1 | 2000 | 1259.7 | 433.7 | 833 | 1209 | 1333 | 1500 | 10417 |
| collect_reveals_public_clone_80x24_expired | 2 | 2000 | 923.8 | 574.9 | 791 | 875 | 1042 | 1250 | 23750 |
| apply_frame_80x24_disabled | 1 | 2000 | 17.6 | 20.6 | 0 | 0 | 42 | 42 | 83 |
| apply_frame_80x24_disabled | 2 | 2000 | 17.9 | 20.6 | 0 | 0 | 42 | 42 | 42 |

Disabled p50 of 0 ns is timer resolution, not zero work. Sub-40 ns Instant on this host often reads 0.

Cross-run p50 agreement is tight on the scan paths (~1.58 µs mid-window 80x24, ~10.25 µs mid-window 200x60). Means are noisier because of a long tail (max 10–70 µs). Gate 5 should compare p50, not mean.

Scale check: 12000/1920 = 6.25. Mid-window apply_frame p50 ratio 10250/1583 ≈ 6.47. Public-clone hot p50 11333/1792 ≈ 6.32. The scan is linear in tracked cells, as the source says.

First-stamp (~46 µs p50) includes model construction, a dummy Clean, Full-grid stamp, `repack` of 1920 cells, `collect_reveals`, and trail. It is not a `collect_reveals` number.

Raw logs: `run-1.txt`, `run-2.txt`.

Not covered: GPUI paint, `GradientCache::get`, terminal `build_frame_delta`, GUI frame time (already `blocked` in S12). metalterm 0.22 ms is GPUI, unused here.

## Manifest / lockfile

`git status --porcelain` after this work: no `Cargo.toml`, no `Cargo.lock`. Artifacts under `verification/perf/2026-08-31-baseline/` are untracked.

## Benchmark seam (quoted, not applied)

Do not add Criterion. Match `mr-crabs-bench`: Instant, per-sample vectors, p50/p95/p99.

Target: `crates/mr-crabs-effects`, because only that crate can call private `collect_reveals` and `repack`. `mr-crabs-bench` already depends on effects and should keep the composite `effects` workload; isolation lives next to the functions.

### Manifest entry (`crates/mr-crabs-effects/Cargo.toml`)

```toml
[[bench]]
name = "frame_cost"
harness = false
```

No new `[dev-dependencies]`. Instant only.

### File

`crates/mr-crabs-effects/benches/frame_cost.rs`

### Shape

`cargo bench -p mr-crabs-effects --bench frame_cost` with `CARGO_TARGET_DIR=~/.cargo-target`. `harness = false` so the file is a `fn main` like this lane's `measure.rs`. Print the same `name=... mean_ns=... std_ns=... p50_ns=...` lines. Gate 5 diffs p50 on the named samples below.

Functions timed:

1. `collect_reveals` (private, `model.rs:389`). Direct call after a stamped tracker. 80x24 hot, 80x24 expired, 200x60 hot.
2. `ChangeTracker::repack` (private, `key.rs:494`). Direct call on a full-cap tracker after one stamp. 80x24 and 200x60.
3. `EffectsModel::apply_frame` Clean mid-window and disabled (public, regression floor). Same grids.

`repack` needs `pub(crate)` or the bench in the same crate (`benches/` is an external crate, so `pub(crate)` is not enough). Two honest options:

- A. Put the timer in `#[cfg(test)]` under `model.rs` / `key.rs` and run `cargo test -p mr-crabs-effects --release -- --ignored timing_`. Tests in the crate see private items. No `[[bench]]`. Matches "existing test suites reach internals".
- B. `pub(crate)` plus `benches/frame_cost.rs` in the package does **not** see `pub(crate)`. Need `#[cfg(feature = "bench-internals")] pub fn` or move the bench into `tests/frame_cost.rs` with `extern crate` and a `pub use` of the two functions behind `#[cfg(any(test, feature = "bench-internals"))]`.

Recommended: option A. Zero manifest change beyond nothing if tests already exist; or add only `[[bench]]` if a later unit insists on `cargo bench`. For Gate 5 reruns, `cargo test -p mr-crabs-effects --release --test unused` is worse. Cleanest later patch that still avoids Criterion and avoids lockfile churn:

Make `collect_reveals` and `repack` `pub(crate)`, add `crates/mr-crabs-effects/tests/frame_cost.rs` (integration tests still cannot see `pub(crate)`). So they must stay unit tests:

`crates/mr-crabs-effects/src/model.rs` `#[cfg(test)]` module already exists. Add ignored timing tests there, plus one in `key.rs` tests for `repack`.

### Quoted patch (do not apply)

```diff
--- a/crates/mr-crabs-effects/src/model.rs
+++ b/crates/mr-crabs-effects/src/model.rs
@@ -386,7 +386,7 @@ impl EffectsModel {
 /// Build the per-frame reveal/pending lists from the tracked cells,
 /// clipping to changed cells only (sentinel texels are skipped). Cells past
 /// the reveal window are skipped as well.
-fn collect_reveals(
+pub(crate) fn collect_reveals(
     tracker: &ChangeTracker,
     mode: TextAnimation,
     duration_ms: f64,
     now: f64,
     out: &mut EffectsFrame,
 ) {

--- a/crates/mr-crabs-effects/src/key.rs
+++ b/crates/mr-crabs-effects/src/key.rs
@@ -491,7 +491,7 @@ impl ChangeTracker {
     }

     /// Repack every change time into the rgba8 texel byte layout.
-    fn repack(&mut self) {
+    pub(crate) fn repack(&mut self) {
         for (i, &bits) in self.change_times.iter().enumerate() {
             let b = i * 4;
             self.packed[b] = bits as u8;
```

Then in `model.rs` `#[cfg(test)]` (same file, so private access works even without `pub(crate)`; the `pub(crate)` is only if a later binary in the package needs it):

```rust
    #[test]
    #[ignore = "timing; Gate 5"]
    fn timing_collect_reveals_80x24_hot() {
        // stamp tracker via EffectsModel Full path, then:
        let t0 = std::time::Instant::now();
        for _ in 0..2000 {
            frame.revealing.clear();
            frame.pending.clear();
            collect_reveals(tracker, TextAnimation::Streaming, 120.0, 1060.0, &mut frame);
        }
        // print mean/std/p50 like measure.rs
    }
```

And in `key.rs` tests:

```rust
    #[test]
    #[ignore = "timing; Gate 5"]
    fn timing_repack_80x24() {
        let mut tracker = ChangeTracker::new(80, 24, 1 << 20);
        // stamp once, then:
        for _ in 0..2000 {
            tracker.repack();
        }
    }
```

Run filter for Gate 5:

```
CARGO_TARGET_DIR=~/.cargo-target cargo test -p mr-crabs-effects --release --lib -- --ignored --nocapture timing_
```

No lockfile change. No Criterion. Isolates `collect_reveals` and `repack` so Lane I's changed-cell bound and a later `repack` deletion each show a named p50 delta.

## Deviations

- First-stamp sample includes model construction. Call it composite, not `collect_reveals`.
- Public clone of `collect_reveals` is slightly slower than `apply_frame` Clean mid-window on 80x24 (1.79 µs vs 1.58 µs p50). Extra Vec tuple pushes vs `CellReveal`. Directionally the same scan.
- Disabled Instant often reads 0 ns. Report it as noise floor, not a budget.

## Follow-ups

- Later unit applies the quoted `pub(crate)` plus ignored tests, then Gate 5 re-runs the `--ignored timing_` filter on the integrated head.
- Lane I bounds `collect_reveals`; compare `timing_collect_reveals_*` p50, not `apply_frame` first-stamp.
- After A+B, deleting `repack` should drop `timing_repack_*` to near zero and shrink first-stamp.

Principles used: prove-it-works (two real process runs, not a compile), build-the-lever (`run.sh` + `measure.rs` are the rerunnable artifact), unslop (this file).

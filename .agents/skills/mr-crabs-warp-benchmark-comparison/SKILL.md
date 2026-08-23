---
name: mr-crabs-warp-benchmark-comparison
description: "Run and compare pinned vtebench/termbench against the real Mr Crabs GPUI+PTY path, preserving process safety and historical-reference caveats."
---

# Run the Mr Crabs Warp-reference benchmarks

Use the canonical Rust Mr Crabs checkout only. Do not build or launch Warp or another terminal. Keep fixtures/results under `/tmp`; do not edit the repository unless the user separately asks to fix a discovered defect.

## 1. Freeze provenance and safety

- Record `git rev-parse HEAD`, platform, toolchain, and current Mr Crabs PIDs.
- Preserve any daily-driver process. Launch a dedicated benchmark process only; cleanup by its exact supervised PID.
- The Warp page is historical reference data, not a same-machine oracle: <https://docs.warp.dev/terminal/comparisons/performance/>.

## 2. Run the internal release gate

```sh
cargo build --release --locked -p mr-crabs-bench --bin mr-crabs-bench
python3 verification/tools/release_gate.py \
  --bench "$HOME/.cargo-target/release/mr-crabs-bench" \
  --out /tmp/mr-crabs-benchmark-$(date +%Y%m%d).json
```

Report measured/pass/fail/blocked exactly. GUI and energy probes may be explicitly blocked by design.

## 3. Build pinned vtebench

Use pin `93bcc32b6e0f7560e9b1a5a8b0998c04fbf9b50d` in `/tmp/mr-crabs-warp-benchmark/fixtures/vtebench`. Build with an isolated target directory.

Select exactly these Warp-published cases into a temporary directory:

- `dense_cells`
- `scrolling`
- `scrolling_bottom_region`
- `scrolling_bottom_small_region`
- `scrolling_fullscreen`
- `scrolling_top_region`
- `scrolling_top_small_region`
- `unicode`

Create a temporary executable wrapper that runs:

```sh
exec /tmp/mr-crabs-warp-benchmark/fixtures/vtebench/target/release/vtebench \
  --silent --warmup 1 --min-bytes 1048576 --max-secs 10 \
  --benchmarks /tmp/mr-crabs-warp-benchmark/fixtures/vte-selected \
  --dat /tmp/mr-crabs-warp-benchmark/results-vtebench.dat
```

Launch the existing release Mr Crabs binary as a dedicated supervised process:

```text
--shell=<wrapper>
--initial-window-size=208x54
--close-on-exit=always
--startup-fetch=false
--cursor-trail=false
--cursor-style-blink=false
--text-animation=none
--window-padding-x=0
--window-padding-y=0
```

Verify the child process is vtebench, wait for exit 0, and confirm no benchmark-owned process remains. Treat `208x54` as configured unless runtime geometry is independently captured.

## 4. Summarize DAT correctly

- First row contains case names.
- Remaining columns are integer milliseconds; `_` means no sample for that case.
- Compute arithmetic mean and nearest-rank p90: sorted index `ceil(0.90*n)-1`.
- Preserve raw DAT and SHA-256.

## 5. Run pinned termbench

Use `cmuratori/termbench` pin `82afbc69256b4e22de913f0f02f82e0480f3dac5` in `/tmp`.

On Apple Silicon, patch only fixture metadata/portability:

- Include `<cpuid.h>` only for x86.
- Execute the CPUID brand loop only on x86; label arm64 `Apple Silicon`.
- Do not alter timed generators, byte counts, writes, timers, or modes.

Compile with:

```sh
clang++ -O2 -std=c++17 termbench.cpp -o termbench
```

Run `small` and `normal` in separate dedicated Mr Crabs processes using the same disabling flags. A failed run is a benchmark outcome; do not invent a score or fix source unless requested.

Current known boundary at e5a4b26: both modes panic at `crates/mr-crabs-terminal/src/side_tables.rs:35` with `style table overflow u16`, because per-character truecolor creates more than 65,536 stable styles. Re-verify after terminal-engine changes.

## 6. Compare with Warp tables

Copy Warp’s published averages, p90, and termbench values verbatim. Add fresh Mr Crabs values in a separate column. State:

- lower is better;
- Warp values are historical 2022 results;
- hardware, OS, versions, and grids differ;
- the comparison is directional, not controlled same-machine evidence;
- termbench failure is `No score`, with the exact panic, never zero.

---
name: mr-crabs-rust-compact-performance-gate
description: Continue or verify the pure-Rust Mr Crabs compact terminal engine and its Ghostty performance release gate.
---

# Mr Crabs Rust compact-terminal performance gate

Repository: `/Users/jamie/Documents/Projects/active/mr-crabs-rust`.

## Frozen known-good point

Commit `b79dada9d` completed the compact-grid cutover. Verify current repository state before relying on this pin. Later storage/scroll work: keep overflow pages hot on compress-queue Full (`521aa21cc` lineage) and full-screen `pop_front`/`push_back` (`6678a47da`).

## Core invariants

- Active compact rows use `first_occupied` and `occupancy`; bytes outside that interval may be stale and MUST materialize as default cells on reads, snapshots, and compression.
- Active/recycled row allocations are uniquely owned before the unsafe unique-write path. Rows may enter the recycle pool only after successful compression detached the page resident; stale completions MUST NOT recycle shared rows.
- Recycled rows from an old column width must be skipped after resize.
- Segmented pages preserve row `Arc<[Cell]>` ownership on feed; flatten only during cold compression/forced compression.
- Compression must preserve occupied bounds and enforce `max_lines` after pending jobs settle.
- A full compress job queue MUST restore the page resident and retry on drain. NEVER LZ4 on the feed thread (`enqueue_page` `TrySendError::Full`).
- Full-screen history scroll (origin 0, region == active len) MUST `pop_front` + `push_back`. DECSTBM region scroll keeps `remove`/`insert`.
- `Terminal::feed` `FEED_SLICE_BYTES` stays `2560`. Raising it to `20480` dropped scrollback_1m to ~49 MiB/s and blew RSS.
- The paired-SGR fast path only accepts streams whose sole escape is adjacent `ESC[31mESC[0m`, validates UTF-8 continuation state before mutation, ends at the default SGR state, and must match bytewise VTE behavior across chunk boundaries.
- The scrollback benchmark corpus remains exactly `line\n` repeated 1,000,000 times with its pinned FNV identity. Generate bounded batches; do not retain a duplicate 5 MiB source buffer during memory measurement.
- Never launch GUI workloads unless Jamie explicitly authorizes it.

## Verification ladder

```bash
cargo fmt --all
cargo check --workspace --all-targets --locked
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked -- --test-threads=1
cargo build --release --locked -p mr-crabs-bench
python3 verification/tools/release_gate.py --out verification/release-gate.json
```

For a scrollback-only check: 1 warmup + 5 isolated `mr-crabs-bench --suite release --workload scrollback_1m` processes. Compare median to `verification/baselines/oracle-baseline.json` (Ghostty ~123.94 MiB/s, RSS 10059776). Do not treat stale `verification/release-gate.json` as evidence.

Expected known-good gate at `b79dada9d`: `overall=PASS pass=15 fail=0 blocked=4`. The blocked workloads are GUI frame time, window redraw, strict GUI idle, and root-only energy; never report them as PASS without measurement.

Post-`6678a47da` measured median ~119.9 MiB/s, RSS ~8.9 MiB (RSS beats oracle; throughput still short). Do not weaken the gate.

## Regression focus

Run the bytewise VTE parity test for the paired-SGR path and the complete differential Ghostty corpus after protocol changes. Run history replay/viewport resize tests after recycle or row-width changes. Exact retained scrollback records must remain 1,000,000. Focused after scroll edits: `cargo test -p mr-crabs-terminal --locked --lib compact:: -- --test-threads=1`.

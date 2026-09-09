# Phase 8. CPU scroll pass

Back to [overview](overview.md).

## Goal

One measured improvement on the actual hot path. Scroll and storage, not paint.

## Changes

`verification/research/gpu-terminal-architecture.md` ranks `CompactEngine::scroll_up_relative`, ingest, and storage above GPUI paint. Re-run `verification/perf/2026-08-31-baseline` first. Then one change in `crates/mr-crabs-terminal/src/compact/engine.rs` with a keep gate of at least 5 percent on the named sample, no missed-frame claim, Cell stays 8 bytes, FramePool stays 4.

Do not add wgpu, a private atlas, or a second renderer. If the baseline does not move, revert.

## Data structures

No new types. Keep `Cell` at 8 bytes.

## Verification

Static. `cargo test -p mr-crabs-terminal --offline`.

Runtime. Headless baseline `run.sh` before and after. Record p50 of the targeted sample. GUI smoke that scrollback still paints. No Metal System Trace required unless the number moves and Jamie wants it.

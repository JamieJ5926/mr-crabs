---
name: mr-crabs-terminal-performance-diagnosis
description: "Diagnose and rank Mr Crabs terminal performance changes using workload semantics, PTY-floor controls, phase splits, and the 8-byte style-capacity gate."
---

# Performance diagnosis workflow

Use this workflow before changing Mr Crabs terminal performance code.

## 1. Freeze workload semantics

- Read the pinned vtebench/termbench generator and timer implementation.
- Separate setup bytes from timed benchmark bytes.
- State what the timer actually covers. vtebench write+flush time is not GPU-completion time.
- Normalize throughput by payload bytes; do not compare raw historical milliseconds across different grids or hardware as causal evidence.

## 2. Establish the floor

- Compare terminal workloads against the nearest transport-only control such as `pty_echo`.
- Compare primary-screen and alternate/region variants that bypass storage.
- If both match the transport control, reject parser/grid/storage rewrites until an internal phase split proves otherwise.

## 3. Trace current source before borrowing competitor ideas

- Verify whether the proposed primitive already exists: row rotation, recycling, dirty rows, ASCII batching, style interning.
- Treat Warp/iTerm/Terminal.app explanations as directional unless source-backed at the relevant pin.
- Never infer a language-level cause from benchmark rankings.

## 4. Instrument the first divergence

Measure independently:

1. PTY read/drain;
2. terminal feed/parser/grid mutation;
3. history ingest/compression;
4. frame-delta construction;
5. paint scheduling and paint.

Also record allocations, queue occupancy, payload throughput, and workload-specific counters.

## 5. Style-capacity gate

For truecolor stress:

- Record total style-table length and reachable live style IDs.
- Preserve `Cell { content: u32, style: u16, flags: u16 }` unless explicit evidence justifies changing the 8-byte invariant.
- If total IDs overflow but live IDs remain bounded, compact atomically: remap current and saved pens, primary/alternate rows, history, hot/cold storage; invalidate stale compression with page generations; publish Full damage/style epoch.
- Reject bare LRU reuse, lossy hashes/truncation, and frame-local remapping as engine-overflow fixes.
- A direct-RGB row sidecar is a broader storage/snapshot/delta contract, not a small fallback.

## 6. Rank experiments

1. Correctness failures that invalidate benchmarks.
2. Measured phase bottleneck with a workload-specific discriminator.
3. Small source-local experiment preserving invariants.
4. Focused behavioral tests plus the exact benchmark.
5. Adversarial review before accepting the causal claim.

---
name: mr-crabs-vtebench-termbench-live-run
description: "Run pinned vtebench and termbench through a dedicated real Mr Crabs PTY, capture trustworthy results, and recognize the current u16 style-table termbench failure."
---

# Run terminal-stream benchmarks against Mr Crabs

1. Use the canonical release binary and launch only a dedicated extra Mr Crabs process; leave user instances untouched.
2. Keep all fixtures/results under `/tmp/mr-crabs-warp-benchmark` and do not edit the repository for a measurement run.
3. Pin vtebench to `93bcc32b6e0f7560e9b1a5a8b0998c04fbf9b50d`. Run the eight comparison cases through Mr Crabs as the PTY child with `--warmup 1 --min-bytes 1048576 --max-secs 10 --silent --dat <path>`.
4. Launch Mr Crabs with animations/startup fetch disabled, zero padding, and a dedicated child shell/wrapper. Use `hub` supervision and wait for exact process exit. Confirm no benchmark PIDs remain.
5. Parse DAT columns independently; report sample count, mean, median, nearest-rank p90/p95, min, and max. Preserve raw DAT and SHA-256.
6. Pin termbench to `82afbc69256b4e22de913f0f02f82e0480f3dac5`. On arm64, guard only the x86 `<cpuid.h>`/`__get_cpuid` metadata path; leave timed workload generation and writes unchanged.
7. Current known outcome at Mr Crabs e5a4b26: termbench Small and Normal panic at `crates/mr-crabs-terminal/src/side_tables.rs:35` with `style table overflow u16`; record this as benchmark failure/no score. Do not suppress it or modify workload colors to obtain a number.
8. Report measured data, exact pins/commit/platform, requested versus independently observed geometry, failure evidence, artifact paths/hashes, and cleanup state. Never imply vtebench measures GPU completion; it measures blocking PTY-output acceptance.

---
name: mr-crabs-terminal-performance-comparison
description: "Repeat the controlled process-level speed, memory, energy, output, OMP, and concurrent-agent comparison between mr-crabs-rust and installed stock Ghostty on macOS."
---

# Mr Crabs versus stock Ghostty benchmark

Use when comparing `/Users/jamie/Documents/Projects/active/mr-crabs-rust/target/release/ghostty` with `/Applications/Ghostty.app/Contents/MacOS/ghostty`.

## Safety

- Never disturb the user's existing terminal processes.
- Launch benchmark-only processes under unique daemon/service names.
- Never launch GUI terminals unless the user explicitly authorized the comparison.
- Stop only benchmark-owned PIDs/services; preserve the user's ordinary Ghostty and Mr Crabs windows.
- Keep evidence outside the fixture directory before cleanup.

## Freeze identities

Record for both binaries:

- exact path
- version/build/commit
- SHA-256
- byte size

Stock Ghostty 1.3.1 is a full product; Mr Crabs may be an incomplete rewrite. State this scope asymmetry in the verdict.

## Controlled surface

Use the same:

- font family and logical size
- padding
- cell-height adjustment
- background opacity
- animation state
- shell integration state
- shell and isolated `ZDOTDIR`
- approximately equal GUI window dimensions

Disable stock user configuration with an explicit temporary config. Verify the resulting window dimensions and shell grid rather than assuming flags took effect.

## Measurements

Alternate A/B order and discard warmups.

1. Binary and CLI startup: at least 30 runs using `/usr/bin/time -l` or monotonic subprocess timing.
2. GUI window availability: at least 10 launches each; identify the exact benchmark PID/window.
3. Shell-ready marker: isolated `.zshrc` writes a marker after startup; at least 15 runs each.
4. Idle: one controlled window each, 30–60 seconds. Sample CPU, RSS, physical footprint, instructions, cycles, interrupt wakeups, and process-attributed energy with `proc_pid_rusage` v6.
5. Output latency/throughput using a FIFO-triggered shell so automation does not depend on GUI typing:
   - 4 KiB burst, at least 15 runs
   - 1 MiB burst, at least 5 runs
   - 20 MiB sustained output, at least 5 runs
   Completion marker must be emitted after the child finishes writing. Explicitly note that final paint may still be queued.
6. Agent workloads:
   - profile one real OMP process independently of either terminal
   - profile three concurrent OMP processes
   - save actual transcripts
   - replay byte-identical single and combined transcripts through both terminals
   - optionally run one live request in each terminal, but never compare live wall time because provider latency/content varies
7. Codex: report BLOCKED rather than inventing a result if authentication/provider/model routing fails.

## Statistics

Report medians and p95/ranges. For key time ratios, produce deterministic nonparametric bootstrap 95% confidence intervals (10,000 resamples). Keep live-provider timing separate from deterministic transcript replay.

## Interpretation

Separate these costs:

- agent/model process
- terminal process
- network/provider latency
- transcript rendering

Process-attributed `ri_energy_nj` is not whole-system wall power and excludes the remote model server. Summed per-process peak footprints are not simultaneous system peak RSS unless sampled concurrently.

Prioritize measured bottlenecks. In the 2026-08-15 baseline, Mr Crabs won startup and footprint while stock Ghostty won sustained output and wakeup efficiency; identical OMP transcript rendering was effectively tied. Current runs must re-measure rather than assume that baseline still holds.

## Cleanup

- Copy final JSON evidence to a stable `/tmp/...results.json` path.
- Validate JSON with `jq -e`.
- Stop benchmark services.
- Remove benchmark fixtures.
- Verify user-owned terminal PIDs remain alive.

# fetchdiag 2026-09-01

## Return-None hypothesis space (`capture_rustfetch_from`)

Line numbers after removing the unreachable post-disconnect `try_recv` drain.

1. **L358** `PtySize::new(...).ok()?` — invalid size (not reachable with 120x40).
2. **L363** spawn `Err` — `PtySession::spawn` failed.
3. **L375** deadline elapsed inside the output loop (`remaining.is_zero()`).
4. **L381** captured bytes would exceed `RUSTFETCH_CAPTURE_MAX_BYTES`.
5. **L404** output disconnected, status still missing, remaining deadline is zero, `output` empty.
6. **L413** output empty and `exit_rx` never delivered a status.
7. **L421** output empty after a status arrived (zero-byte successful or failed child).

UTF-8 conversion never returns `None` (lossy fallback).

## Drain verdict: DEAD / unreachable

`std::sync::mpsc::Receiver::recv_timeout` returns `Disconnected` only after the channel is empty and all senders are dropped. Messages sent before disconnect are delivered as `Ok` first. The previous `try_recv` loop after `Disconnected` could only observe `Empty` or `Disconnected`. Removed.

## Pre-fix instrumented runs

- Filtered two tests, 200 cargo invocations: **failures=0**. All 400 capture traces `branch=ok`.
- Full `cargo test -p mr-crabs-app --lib`, 20 invocations: **failures=0**. 40 `branch=ok` (the two exact_capture tests) plus 20 `branch=4_overflow` from `capture_overflow_reaps_and_returns_none` (expected).

## NOT REPRODUCED

No failing iteration in 200 instrumented filtered runs. Prior flake evidence (`verification/2026-09-01-usable-run/flake/run-32.txt`, `run-34.txt`) failed both tests under a loaded 423-test suite.

Most likely remaining branch: **7 (empty output after reader disconnect)** if the PTY reader hits EIO/EOF before the first successful read of a short-lived child's bytes; next is **6** if the reader disconnects and exit status is also missed.

Probe that would settle it: keep per-run traces of `reader_loop` read errno (especially EIO vs n=0) plus whether any `try_send` succeeded before drop, under the same full-suite load as run-32/34. That requires instrumenting `crates/mr-crabs-pty/src/session.rs` (sibling-owned this run).

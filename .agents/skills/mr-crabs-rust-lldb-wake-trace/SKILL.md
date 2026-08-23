---
name: mr-crabs-rust-lldb-wake-trace
description: Trace the mr-crabs-rust PTY wake chain (reader → output_wake → spawn_wake_task → AppModel::pump) with batch LLDB using the diag binary built with LTO off and full debug symbols.
---

# mr-crabs-rust LLDB wake-chain trace

## When
Diagnosing the mr-crabs-rust GUI showing an empty frame with a cursor but no shell prompt/output/input echo (working-terminal recovery plan).

## Build the diagnostic binary (LTO strips DWARF!)
- `cargo build --release --locked -p mr-crabs-app --bin mr-crabs` in the repo produces a binary WITHOUT usable debug info even with `CARGO_PROFILE_RELEASE_DEBUG=2` because `lto = "thin"` in `[profile.release]` drops DWARF at link.
- Use: `CARGO_TARGET_DIR=/tmp/mr-crabs-diag-target cargo build --release --locked -p mr-crabs-app --bin mr-crabs --config 'profile.release.lto=false' --config 'profile.release.debug=2' --config 'profile.release.codegen-units=16'`
- Verify: `dwarfdump --debug-info /tmp/mr-crabs-diag-target/release/mr-crabs | grep -c DW_TAG_subprogram` should be nonzero; object files in `release/deps/*.rcgu.o` must have `__DWARF` sections.

## LLDB breakpoint names
- Rust method demangled names need angle brackets: `<mr_crabs_app::model::pane::PaneModel>::rebuild_frame`, NOT `mr_crabs_app::...::rebuild_frame`. Free functions use plain `mr_crabs_pty::session::reader_loop`.
- Entry addresses (unslid, from `nm -a`): `Terminal::feed` 0x1000c7ea0, `PaneModel::pump` 0x100035fec, `spawn_with_output_wake` 0x100033614, `rebuild_frame` 0x10003460c, `reader_loop` 0x1000f47dc, `AppModel::pump` 0x100077f88 (diag build; re-verify with nm each build).

## Batch trace recipe
- Interactive `hub start lldb` is unusable: PTY echoes every keystroke with the app title interleaved. Use batch mode instead:
```
rm -f /tmp/mr-crabs-trace.log
timeout 30 lldb --batch -o "target create /tmp/mr-crabs-diag-target/release/mr-crabs" -o "command script import /tmp/mr_crabs_trace.py" 2>/dev/null
pkill -9 -f "mr-crabs-diag-target"
```
- `/tmp/mr_crabs_trace.py` (no dashes in the filename!): `__lldb_init_module` sets `SBBreakpoint` per name with `SetScriptCallbackBody` writing `HIT <label> x0..x3` to /tmp/mr-crabs-trace.log, launches via `target.Launch`, event-loops with `listener.WaitForEvent(1,...)` and `proc.Continue()` on stops, hard deadline ~6s, then `os._exit(0)` (proc.Kill() hangs batch lldb on a GUI inferior).
- Callback body: plain string concatenation (`"import time\n" + ...`), NEVER nested `%` formatting; `frame.FindRegister('x0').GetValueAsUnsigned()`.

## Interpreting the trace (known-good baseline)
Expected healthy sequence: SPAWN → REBUILD_FRAME (empty, commit_geometry) → READER_LOOP → APP_PUMP/PUMP (wake task) → FEED → REBUILD_FRAME (with prompt). Broken current state: SPAWN/REBUILD_FRAME/READER_LOOP fire, then nothing — the wake boundary is the first divergence; engine/paint are NOT yet implicated.

## Decisive PTY-path headless check
`crates/mr-crabs-pty/examples/diag_spawn.rs`: `CommandBuilder::new("/bin/sh").shell(None)`, `PtySession::spawn`, `rx.recv_timeout(200ms)` loop. Production zsh without args IS interactive and prints the prompt (132 bytes observed). This proves spawn+reader+enqueue; the bug is downstream of enqueue (wake channel/task).

## Gotchas
- Interactive lldb breakpoints by mangled name (`__RNv...`) never resolve; use demangled names.
- GUI app under lldb prints benign linkd/WindowTab stderr noise; ignore.
- Always `pkill -9 -f mr-crabs-diag-target` after a trace run.

# Lane D command-block snapshot

routing: cli-proxy/dddai.grok-4.6 (workstation metadata; no live fallback)

Status: PASS

Principles that changed choices: principle-model-the-domain (typed phase machine next to live cursor state, not OSC flags); principle-laziness-protocol (no gutter range, no history table, no OSC parser); principle-prove-it-works (injected clock plus feed tests); unslop on this file.

## API shape

`CommandBlockSnapshot` in `crates/mr-crabs-protocols/src/semantic_prompt.rs`:

- `id`: `CommandBlockId::Aid(String)` from OSC `aid`, else `Generated(u64)`
- `phase`: `Prompt | Input | Running | Finished`
- `prompt_kind`: last A/P/N `k=`
- `input_start`: `(row, col)` at B/I; cleared on C/D/P/A
- `exit_code`: this D's positional integer, or `None` if omitted
- `failed`: `exit_code.is_some_and(|c| c != 0)` only
- `duration_ms`: C clock to D clock; `None` until D after a C
- `cmdline`: decoded C `cmdline` / `cmdline_url` via existing `write_command_line`

No `gutter_rows`. Viewport block geometry is not recorded yet.

Live `SemanticPromptState` fields are unchanged. Snapshot lives as private fields on that struct. Accessors:

- `SemanticPromptState::command_block_snapshot()`
- `SemanticPromptState::last_finished_command()`
- `SemanticPromptState::apply_at(..., now_ms)` for tests
- `Terminal::command_block_snapshot()`
- `AppCore::command_block_snapshot()`

## What it does

On A/N/P, start a new current block at Prompt. On B/I, Input plus input_start. On C, Running, start clock, optional cmdline. On D, Finished with this D's exit (or None), failed only for nonzero Some, duration from C-to-D. `last_exit_code` still sticks across the next prompt. Running phase never copies that sticky code.

## The one fact it is safe because of

Live cursor fields (`content`, `row`, `last_exit_code`, `input_start_*`, `cursor_is_at_prompt`) keep the same assignments they had before this unit. The snapshot is additive.

Proof, step 4. Existing tests still pass:

- `shell::tests::semantic_prompt_transitions`
- `shell::tests::end_command_clears_row_after_a_b_d`
- `shell::tests::prompt_start_redraw_clears_stale_input_start_coordinates`

Plus new tests:

- `command_block_phases_a_b_c_d` (duration 150ms under injected 100 then 250)
- `sticky_last_exit_code_is_not_running_phase`
- `missing_exit_code_is_unknown_not_failure`
- `tests::command_block_snapshot_follows_osc_133` (terminal feed)
- `tests::app_core_command_block_snapshot_tracks_osc_133`

`cargo check -p mr-crabs-app --offline` passed.

## Risks

Production `monotonic_now_ms` now uses a process-local `LazyLock<Instant>` epoch and saturates elapsed millis at `u64::MAX`. Tests still inject `apply_at`. A D without a prior C yields `duration_ms: None`.

Repair (post-D DiagnoseDClock FRESH): wall `SystemTime` replaced with Instant so C-to-D duration cannot follow clock skew.

A new A after Finished starts a new id. History is only `last_finished_command`, not a list.

## Cleared

No `element.rs` edits. No OSC parse outside existing `SemanticPrompt` / `write_command_line`. Missing D integer is not failure. Sticky `last_exit_code` is not Running phase.

## Commands

```
CARGO_TARGET_DIR=~/.cargo-target cargo test -p mr-crabs-protocols --offline --lib -- command_block sticky_last missing_exit semantic_prompt_transitions end_command_clears prompt_start_redraw
CARGO_TARGET_DIR=~/.cargo-target cargo check -p mr-crabs-app --offline
```

Clock repair re-ran those two. Both passed. `cargo check` still warns unused `focused`/`now_ms` in `element.rs` (pre-existing, not this change).

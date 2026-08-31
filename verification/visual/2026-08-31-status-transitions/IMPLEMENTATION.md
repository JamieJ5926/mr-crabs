# Lane M implementation: live command status

routing: cli-proxy/dddai.grok-4.6 (workstation; no live fallback observed)

Status: PASS for the pure model. UI label deferred.

Principles: principle-model-the-domain (one `StatusView`); principle-boundary-discipline (snapshot + policy inject, no OSC, no `detect()`); principle-laziness-protocol (no new chrome).

## Search record (find-not-invent, code reuse)

Stage 1 repo:

- glob `crates/mr-crabs-app/src/model/**`
- grep `LiveCommandStatus|StatusView|command_status` in `crates` (none)
- grep `CommandBlockSnapshot` (D API in protocols + `AppCore`/`Terminal` accessors)
- grep pane chrome status labels in `mr-crabs-app` (none)

Findings:

- `model/input_dock.rs` owns the live prompt-row overlay and hides when B coords leave the cursor row. Rejected as owner. Status is pane-level.
- `model/presentation.rs` owns chat vs terminal surface mode. Rejected. Different domain.
- `model/shell_integration.rs` is spawn/features, not live status.

Verdict: BUILD-JUSTIFIED `crates/mr-crabs-app/src/model/command_status.rs`. Boundary against `input_dock` (overlay) and D command blocks (typed snapshot, no OSC parse here).

Stage 5 CLOSED by brief (no external research).

## API

```
StatusView::from_semantic(semantic, snapshot, ever_seen_osc133)
StatusView::from_snapshot(snapshot, ever_seen_osc133)
StatusView::from_live(semantic, ever_seen_osc133)
reduce(policy, from, to, started_at) -> StatusTransition
```

Running/Finished come from `CommandBlockSnapshot.phase`. Sticky `last_exit_code` is unused. Missing `exit_code` on Finished is `ExitOutcome::Unknown`. `duration_ms` is copied from the snapshot. `reduce` uses `policy.allows_motion()` only.

## Transition contract (repair)

`reduce` returns `Duration::ZERO` when:

- `from == to`
- `!policy.allows_motion()`
- either side is Hidden
- AtPrompt <-> Editing
- Output kind/exit/duration-only changes
- any other edge outside the allowed command-output pair

Nonzero `MOTION_CROSSFADE` (120ms) only for AtPrompt/Editing -> Output and Output -> AtPrompt when motion is allowed.

## UI decision

No existing pane chrome label. `workspace.rs` was not edited. Display deferred.

## Blast radius

One fact: mapping is additive and does not parse OSC or call `detect()`. Animation is gated on status-kind edges, not `from != to`.

Proof step 4: `cargo test -p mr-crabs-app --offline --lib -- command_status` and `cargo check -p mr-crabs-app --offline`.

## Tests

Hidden, AtPrompt, Editing, Running with sticky exit 1, Finished success/failure/unknown, live Unknown kind.

Focused reduce tests: `hidden_to_prompt_zero`, `prompt_to_editing_zero`, `output_running_to_finished_zero`, `editing_to_output_nonzero`, `output_to_at_prompt_nonzero`, `reduce_motion_zero_duration`.

Forbidden: no `element.rs` edits, no OSC parser, no `detect()`.

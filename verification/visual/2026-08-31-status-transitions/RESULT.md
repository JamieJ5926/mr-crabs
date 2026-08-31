# Lane M design: command and status transitions

routing: unverified (cli-proxy/dddai.grok-4.6 in workstation metadata; no live fallback observation)

Status: PASS. Artifact only. No crate edits. No GUI.

throughput checkpoint: n/a, read-only investigation

Principles that changed choices: principle-model-the-domain (one `LiveCommandStatus` enum instead of painting from `content`/`row` flags); principle-boundary-discipline (M reads `SemanticPromptState` and later `CommandBlockSnapshot`, never OSC, never NSWorkspace); principle-experience-first (Reduce Motion snaps status, does not interpolate); unslop on this file.

Playbook: Investigation via how Explain. Seat cannot spawn children. skip: 2a/2b explorer and explainer Tasks. Direct read of D's contract plus `shell.rs` and `accessibility_policy.rs`.

## Overview

M owns live command and status presentation that is not gutter geometry. HEAD has a cursor-local OSC 133 machine (`SemanticPromptState`) and no block list, no duration clock, and no gutter. This unit maps that live machine onto a presentation enum, names what waits on D, and records the later implementation seam. Paint stays out of `element.rs`. Reduce Motion comes from Lane E's `AccessibilityPolicy.allows_motion()` only.

## Current legal HEAD status mapping

Source of truth is D's table in `verification/visual/2026-08-31-command-blocks/LANE-M-CONTRACT.md`. M may read `pane.core.semantic_state()` and `pane.ever_seen_osc133()`. M must not parse OSC.

Pure map, no duration:

| `ever_seen_osc133` | `content` | `cursor_is_at_prompt()` | `last_exit_code` | `LiveCommandStatus` |
| --- | --- | --- | --- | --- |
| false | any | any | any | Hidden |
| true | Prompt | true | ignored | AtPrompt |
| true | Input | true | ignored | Editing |
| true | Output | false | ignored for phase | Running if last OSC was C; Finished if last OSC was D |

HEAD cannot distinguish Running from Finished by fields alone. Both sit on `content == Output`, `row == None`, `cursor_is_at_prompt() == false`. `last_exit_code` is the previous D integer and stays until the next D with a parseable integer. A C after a failed D still shows that old code. Using `last_exit_code.is_some()` as "finished" is wrong.

Legal HEAD labels:

- Hidden. Never seen OSC 133. Apple `/bin/bash` and other Hidden injects stay here.
- AtPrompt. A / N / P. `content` Prompt, `row` Prompt, at prompt.
- Editing. B / I. `content` Input, `row` Input, at prompt.
- Running. After C. Same Output fields as Finished. HEAD has no extra bit. M must not invent one.
- Finished. After D. Same Output fields. Exit outcome is a separate field, not the phase.

Exit outcome (orthogonal, only meaningful in Finished, and even then only when the integer exists):

- Success. `last_exit_code == Some(0)`
- Failure. `last_exit_code` is Some and not 0
- Unknown. `last_exit_code` is None. Not failure. D without a positional integer leaves the previous code, so Unknown on a just-finished command is rare and only if no D in the session ever carried an integer.

`RowSemantic::Command` and `PromptContinuation` are never assigned. Ignore them. OSC 133 L does not change content or row. `cmdline` is parsed and dropped. `aid` is unused.

Proposed HEAD type (lives in app, not protocols):

```text
enum LiveCommandStatus {
  Hidden,
  AtPrompt,
  Editing,
  Output { kind: OutputKind },  // Running vs Finished is OutputKind::Unknown until D
}

enum OutputKind {
  Unknown,   // HEAD: C and D look identical
  Running,   // after CommandBlockSnapshot.phase == Running
  Finished,  // after snapshot.phase == Finished
}

enum ExitOutcome {
  None,      // not in Finished, or no integer ever
  Success,   // Some(0)
  Failure,   // Some(n) n != 0
  Unknown,   // Finished and snapshot.exit_code is None
}

struct StatusView {
  status: LiveCommandStatus,
  exit: ExitOutcome,
  duration_ms: Option<u64>,  // always None on HEAD
}
```

On HEAD, `OutputKind` is always `Unknown` when `content` is Output. Do not guess Running vs Finished. The UI copy for that cell is "output" or a neutral mark, not a spinner that claims the command is still running.

## What waits for D's later `CommandBlockSnapshot`

Do not implement until D publishes the type named in the contract:

```text
CommandBlockSnapshot
  id
  phase: Prompt | Input | Running | Finished
  prompt_kind
  input_start
  exit_code
  failed
  duration_ms
  cmdline
  gutter_rows
```

Blocked on that snapshot:

1. Running vs Finished as distinct labels.
2. Duration copy or a duration bar. No paint-time `Instant`. No fake elapsed.
3. Historical failure marks for previous commands. HEAD keeps one `last_exit_code`. The next D overwrites it.
4. Gutter geometry and gutter paint. D owns `element.rs` block layout. M never paints those quads.
5. `failed` as a published bool. Until then M may derive Failure only from `last_exit_code` Some and not 0, and only as the live last command, never as a row gutter.
6. `cmdline` display. Parsed and discarded today.

M may keep reading the live machine after the snapshot exists. D's contract says the live fields stay cursor truth. Snapshot is history plus clock. Overlay status uses snapshot.phase when present, else the HEAD map.

## Proposed transition data shape (Reduce Motion via E)

Transitions are presentation of `StatusView`, not a second OSC parser.

```text
struct StatusTransition {
  from: StatusView,
  to: StatusView,
  started_at: Instant,           // UI clock only, not command duration
  duration: Duration,            // 0 if !policy.allows_motion()
}

fn reduce(policy: AccessibilityPolicy, from: StatusView, to: StatusView) -> StatusTransition
```

Rules:

- Consume `AccessibilityPolicy` as a parameter. Call `allows_motion()`. Never call `detect()`. Never read NSWorkspace. Never read `reduce_motion` except through that method.
- If `!policy.allows_motion()`, `duration` is zero and the view snaps to `to`. No opacity lerp, no spinner rotation, no color crossfade.
- If motion is allowed, a short crossfade or spinner is legal only for AtPrompt/Editing -> Output and Output -> AtPrompt. Do not animate Hidden.
- Reduce Transparency is C. Status text is opaque. Ignore `allows_transparency()` here.
- Command duration (`duration_ms` on the snapshot) is data, not motion. Showing "1.2s" is allowed under Reduce Motion. Animating a growing bar is not.

Identity: same `StatusView` in, no transition object. P and A clearing `input_start_*` must not start a transition if status stays AtPrompt.

## Exact later implementation seam

M does not edit `crates/mr-crabs-element/src/element.rs`. Preferences assign that file by function. D owns block layout and gutter painting.

Suggested files when a later write unit is authorized:

| Path | Role |
| --- | --- |
| `crates/mr-crabs-app/src/model/command_status.rs` (new) | Pure `live_status(semantic, ever_seen_osc133) -> StatusView`. Optional `from_snapshot(CommandBlockSnapshot)`. No GPUI. No OSC. |
| `crates/mr-crabs-app/src/ui/` chrome that already shows pane chrome, not the cell grid | Bind `StatusView` to a label or badge. Pass `AccessibilityPolicy` from the same app snapshot E already feeds B and C. |
| `crates/mr-crabs-app/src/model/pane.rs` | Read only `semantic_state` and `ever_seen_osc133`. No new OSC latch. |
| `crates/mr-crabs-app/src/accessibility_policy.rs` | E. Do not edit. Inject `from_flags` in tests. |
| `crates/mr-crabs-protocols/src/shell.rs` | Do not edit for presentation. D may add the snapshot table beside the live machine later. |
| `crates/mr-crabs-element/src/element.rs` | Forbidden for M. |

Call chain:

1. Pane already latches `ever_seen_osc133` in `pane.rs` `latch_osc133`.
2. `live_status` reads `SemanticPromptState` fields named in D's contract.
3. Workspace or pane chrome builds `StatusView` once per frame.
4. If a previous `StatusView` differs and `allows_motion()`, start `StatusTransition`. Else snap.

Do not hang this off `input_dock.rs`. The dock is a live prompt overlay and hides when `input_start_row` != cursor row after P/A. Status is pane-level, not input-span-level.

## Tests (later write unit)

No tests run in this unit. Named for the implementer:

1. Table test on `live_status` covering D's OSC rows A/N/P, B/I, C, D, L, plus never-seen. Assert Hidden, AtPrompt, Editing, Output{Unknown}. Assert `last_exit_code == Some(1)` after C is still Output, not Failure-as-phase.
2. `Some(0)` -> Success, `Some(1)` -> Failure, `None` -> Unknown. Missing code is not Failure.
3. `AccessibilityPolicy::from_flags(true, false)` -> transition duration zero. `from_flags(false, false)` may be non-zero. Never call `detect()` in these tests.
4. Snapshot adapter test once D lands `CommandBlockSnapshot`. Running vs Finished and `duration_ms` Some vs None. None means hide duration copy, not `0ms`.
5. Existing protocol tests stay D/protocol owned: `semantic_prompt_transitions`, `end_command_clears_row_after_a_b_d`, `exit_code_is_positional`. M does not duplicate OSC parsing tests.

## Rejected options

- Paint status in `element.rs` cell gutters. D owns that file region. Preferences forbid other lanes.
- Infer Running vs Finished from `last_exit_code`. C leaves the previous D's code in place.
- Fake duration from paint `Instant` or frame count. Contract forbids it until D's C-to-D clock.
- Treat missing exit code as failure. Unknown is the third state.
- Call Reduce Motion APIs in M. E owns detection.
- Drive status from `InputDockSnapshot`. Dock is not a command block and drops on P/A coordinate clear.
- Use `RowSemantic::Command`. Dead.
- Treat OSC 1337 as command markers. Different protocol.

## Gotchas

Fish C at column 0 on a prompt row appends `SemanticAction::None`. No grid effect. Status still goes to Output.

P and A clear `input_start_*`. Do not key a status overlay on B coordinates.

`last_exit_code` is sticky across the next prompt and the next C.

Two on-screen windows break `cua-driver hotkey`. Later visual proof uses one window. This unit has no GUI.

## Dependency on D

Phase 1 (this design, later small write): live labels Hidden / AtPrompt / Editing / Output, plus exit outcome from `last_exit_code` only. No duration. No history.

Phase 2: consume `CommandBlockSnapshot` for Running vs Finished, duration copy, historical marks.

Phase 3: gutter remains D. M never takes it.

## Verification

Read D's `LANE-M-CONTRACT.md`, `shell.rs` `SemanticPromptState`/`apply`/`cursor_is_at_prompt`, `accessibility_policy.rs` `allows_motion`, `lib.rs` `semantic_state`, `pane.rs` `ever_seen_osc133`. No build. No source changes. No git mutation.

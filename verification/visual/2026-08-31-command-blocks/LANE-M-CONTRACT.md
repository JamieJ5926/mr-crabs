# Lane M contract: command and status transitions

routing: unverified (cli-proxy/dddai.grok-4.6 in workstation metadata; no live fallback observation)

Status: PASS. Survey only. No crate edits.

throughput checkpoint: n/a, read-only investigation

Principles that changed choices: principle-model-the-domain (contract is a typed block machine, not scattered OSC flags); principle-boundary-discipline (Lane M consumes a later D-owned block snapshot, never OSC bytes); principle-never-block-on-the-human (duration lives in D's later unit, not a product question); unslop on this file.

## Overview

OSC 133 already parses and drives a live cursor-local state machine. There is no command-block UI, no row-semantic table, and no duration clock. Lane M may depend on the live `SemanticPromptState` fields named below for current-command status, plus `last_exit_code` for the most recent finished command. Duration, historical blocks, and gutter geometry are not in HEAD. They are D's later layout unit. M must not invent them.

## Key concepts

`SemanticPrompt` (`crates/mr-crabs-protocols/src/semantic_prompt.rs`). Parsed OSC 133 command. Action letter plus raw options.

`SemanticPromptState` (`crates/mr-crabs-protocols/src/shell.rs`). Cursor-local machine. One current content, one current row kind, last exit code, last prompt kind, last input start cell.

`SemanticAction` (`shell.rs`). Intents the terminal should apply. MarkPrompt / MarkInput / MarkOutput are no-ops in `crates/mr-crabs-terminal/src/protocol.rs:1078-1086`. Comment says S8 side tables. Those tables are not present.

`InputDockSnapshot` (`crates/mr-crabs-app/src/model/input_dock.rs`). Live prompt overlay. Not a command block.

## How it works

1. Bytes hit VTE. OSC `133` lands in `crates/mr-crabs-protocols/src/osc.rs` state `N133`, then `parsers::semantic_prompt`.
2. First payload byte is the action: A B I C D L N P. Options after `;` stay unvalidated.
3. `protocol.rs:1019-1023` applies `SemanticPromptState::apply` with the live cursor, then forwards the command to `ProtocolSink::semantic_prompt`.
4. `PaneModel::latch_osc133` sets `ever_seen_osc133` once content leaves `None`.
5. App readers: `Terminal::semantic_state()`, `input_dock::derive_input_dock`, chat eligibility in `presentation.rs`. Nobody paints a block.

Live cycle for M:

- Prompt / idle at prompt: `content` is Prompt or Input, `row` is Prompt or Input, `cursor_is_at_prompt()` is true. Input dock may be Active.
- Running: after OSC 133 C (`EndInputStartOutput`). `content` is Output, `row` is None, `cursor_is_at_prompt()` is false. `last_exit_code` is still the previous D, if any.
- Finished: after OSC 133 D (`EndCommand`). `content` is Output, `row` is None. If D carried a positional integer, `last_exit_code` is that value. Missing or non-integer D leaves the previous code.

`cmdline` / `cmdline_url` parse on C (`write_command_line`). No production caller stores them. `aid` parses and is unused. Exit code is positional on D, not `exit_code=`.

## Where things live

| Path | Role |
| --- | --- |
| `crates/mr-crabs-protocols/src/osc.rs` | OSC 133 vs 1337 split |
| `crates/mr-crabs-protocols/src/osc/parsers.rs` | Action letter parse |
| `crates/mr-crabs-protocols/src/semantic_prompt.rs` | Options, PromptKind, ExitCode |
| `crates/mr-crabs-protocols/src/shell.rs` | State machine |
| `crates/mr-crabs-terminal/src/protocol.rs` | Apply + sink |
| `crates/mr-crabs-app/src/lib.rs` | `semantic_state()` |
| `crates/mr-crabs-app/src/model/pane.rs` | `ever_seen_osc133` |
| `crates/mr-crabs-app/src/model/input_dock.rs` | Live prompt consumer |
| `crates/mr-crabs-element/src/element.rs` | No search_matches, no command blocks. D later owns gutter paint. Do not edit in this wave. |

## State shape a later renderer should consume

Do not paint from OSC. Consume a D-owned snapshot (proposed, not in HEAD):

```text
CommandBlockSnapshot
  id: opaque (aid if present, else generated)
  phase: Prompt | Input | Running | Finished
  prompt_kind: Option<PromptKind>
  input_start: Option<(row, col)>   // viewport at B/I; stale on P/A
  exit_code: Option<i32>            // last D integer; None if D omitted it
  failed: bool                      // exit_code.is_some_and(|c| c != 0)
  duration_ms: Option<u64>          // C Instant to D Instant; None until D owns a clock
  cmdline: Option<bytes>            // from C cmdline / cmdline_url; currently discarded
  gutter_rows: Range<u16>           // layout, D later
```

Until that type exists, M's only legal HEAD fields are on `SemanticPromptState`:

- `content: SemanticContent`  None | Prompt | Input | Output
- `row: RowSemantic`  None | Prompt | PromptContinuation | Input | Command
  (`PromptContinuation` and `Command` are never assigned in `apply`)
- `prompt_kind: Option<PromptKind>`
- `input_start_col` / `input_start_row`
- `last_exit_code: Option<i32>`
- `cursor_is_at_prompt()`

Plus pane `ever_seen_osc133: bool`.

`RowSemantic::Command` is dead. Treat Command as unused.

## What Lane M may depend on

Duration. Not in HEAD. M must not fake elapsed time from paint. D later records Instant at C and D. Until then duration UI is off or shows unknown.

Failure gutter. `last_exit_code != Some(0)` is the only failure signal. Zero is success. Absent code is unknown, not failure. Gutter geometry is D's later `element.rs` block layout. M may consume `failed` once D publishes it. M must not paint quads in `element.rs`.

Live status.

| OSC | content | row | at_prompt | M label |
| --- | --- | --- | --- | --- |
| A / N / P | Prompt | Prompt | true | at prompt |
| B / I | Input | Input | true | editing |
| C | Output | None | false | running |
| D | Output | None | false | finished |
| L | unchanged | unchanged | prior | layout only |

A new A/N after D starts the next prompt. `last_exit_code` stays until the next D with a parseable integer.

## Tests that cover markers

Ran with `CARGO_TARGET_DIR=~/.cargo-target`. 9 passed.

- `mr-crabs-protocols` `shell::tests::semantic_prompt_transitions`
- `shell::tests::end_command_clears_row_after_a_b_d`
- `shell::tests::prompt_start_redraw_clears_stale_input_start_coordinates`
- `semantic_prompt::tests::options_parse`
- `semantic_prompt::tests::cmdline_url_decode`
- `semantic_prompt::tests::malformed_options_are_ignored`
- `semantic_prompt::tests::exit_code_is_positional`
- `semantic_prompt::tests::redraw_last`
- `osc::parsers::tests::semantic_prompt_actions`

Related, not run this unit (not command-marker core): `input_dock` OSC 133 visibility tests, `accessibility.rs` chat eligibility feeds, `presentation.rs` trusted OSC latch.

## Gaps

1. No command-block list. One live machine, last exit code only. History is gone after the next D.
2. MarkPrompt / MarkInput / MarkOutput do not mark rows.
3. No timestamps. Duration is blocked on D.
4. `cmdline` is parsed and dropped.
5. `RowSemantic::Command` and `PromptContinuation` unused.
6. EndCommand without a valid integer does not clear `last_exit_code`.
7. No production `set_search_query` / search_matches paint (Lane L). Irrelevant to M except do not share `element.rs` this wave.
8. OSC 1337 is a different protocol. Do not treat animation or iTerm File= as command markers.

## Lane M implementation contract

M owns command and status transition presentation that is not gutter geometry and not `element.rs`.

M may read:

- `pane.core.semantic_state()`
- `pane.ever_seen_osc133()`
- later, a D-owned `CommandBlockSnapshot` if D lands it

M must not:

- edit `crates/mr-crabs-element/src/element.rs`
- parse OSC itself
- call Reduce Motion APIs (Lane E)
- treat missing exit code as failure
- claim duration without D's clock

Suggested M phases:

1. Map live status from `content` + `cursor_is_at_prompt` + `last_exit_code`.
2. Wait for D's block snapshot before duration or historical failure marks.
3. Gutter paint stays D.

## Gotchas

Fish C at column 0 on a prompt row appends a dummy `SemanticAction::None`. No grid effect.

P and A clear `input_start_*`. Input dock hides if `input_start_row` != cursor row. Same trap for any M overlay keyed off B coordinates.

Apple `/bin/bash` is not auto-detected. zsh-first inject in `shell_integration.rs`. Other bundled shells stay Hidden (no OSC 133) until that slice expands.

Two on-screen windows break `cua-driver hotkey`. Capture against a single window.

## Design for D's later unit (not this wave)

Keep GPUI paint. No Metal pass.

Add a small block table next to `SemanticPromptState`, keyed by aid or generation, recording C start Instant, optional cmdline, D Instant and exit code, and the viewport row range at C/D. Feed that table into D's gutter painter. Subtract nothing from the live machine. The live fields stay the cursor truth.

## Verification

Existing marker tests above. PASS. No source changes. No git mutation.

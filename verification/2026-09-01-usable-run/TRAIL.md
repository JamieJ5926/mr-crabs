# Usable-state autonomous run, 2026-09-01

Base: main @ f2d5fb2. Installed artifact: `~/Applications/Mr Crabs.app` (dev.jamie.mr-crabs, adhoc+runtime, 12406160 bytes, 17:41).

## Exit predicate

Done when all seven hold, verified by artifact:

1. `cargo test --workspace` returns zero failures on two consecutive runs, and the 2/420 flake seen at 18:0x is either fixed or named with a root cause.
2. `cargo check --workspace` emits no warning from first-party crates.
3. Installed app launches, pid confirmed live, and a scripted smoke observes: shell prompt, typed command output, new tab, split, copy, paste, scrollback, config reload, resize. Each PASS or a logged defect with root cause.
4. Every action in the registry is either bound in the default keymap or deliberately listed as palette-only.
5. `README.md` no longer contradicts the shipped keymap.
6. One `STATE.md` supersedes RESUME.md and the stale handoff sections.
7. No feature degraded. `inventory-before.txt` vs `inventory-after.txt` shows no removed action, config key, binding, or CLI flag.

## Baseline findings from inventory-before.txt

28 actions, 20 bound. Unbound: CheckForUpdates, CloseTab, NextPane, PreviousPane, ToggleCursorTrail, SetTextAnimationNone, SetTextAnimationStreaming, SetTextAnimationTypewriter. Cmd+W maps to CloseWindow while CloseTab has no key at all, which is backwards for a tabbed terminal.

Config-key list contains apparent alias pairs needing confirmation: cursor-blink vs cursor-style-blink, padding-x vs window-padding-x, scrollback-lines vs scrollback-limit, line-height-adjust-percent vs adjust-cell-height, text-animation-duration vs text-animation-duration-ms.

## Iterations

| # | Change | Predicate moved | Kept |
|---|---|---|---|
| 0 | Built feature-inventory.sh lever, captured baseline, corrected install-location error | 7 baseline established | yes |
| 1 | element.rs dead fields `focused`/`now_ms` deleted (WarnClean) | 2 met, zero first-party warnings | yes |
| 2 | keymap gaps: Cmd+W CloseTab, Cmd+Shift+W CloseWindow, Cmd+Alt+W ClosePane, Cmd+Alt+]/[ pane cycle; 23/28 bound (KeymapGaps) | 4 met | yes |
| 3 | README rewritten to shipped reality, STATE.md authored, RESUME/HANDOFF marked superseded (DocsTruth) | 5, 6 met | yes |
| 4 | Docs reconciled to 23/28 after keymap landed (DocsReconcile) | 5, 6 held | yes |
| 5 | 18-path GUI smoke captured against installed app, pid 3647 live throughout (SmokeCapture) | 3 evidence gathered | yes |
| 6 | Root read the screenshots. Three defects confirmed, listed below | 3 blocked on fixes | n/a |

## Confirmed defects, root-observed in screenshots

D1. A freshly created surface renders no prompt. `01-prompt-idle.png` shows an empty grid at launch. `04-cmd-t-new-tab.png` shows an empty grid after Cmd+T. `05-cmd-d-split-right.png` is decisive: the pre-existing left pane shows `jamie@Jamies-MacBook-Pro / %` while the newly split right pane shows only a dim cursor band and no prompt. The prompt appears only after a later damage event. This is the single biggest usability bug. Open a terminal, see a black window.

D2. Prompt duplicates mid-line on redraw. `18-tui-render.png` line reads `jamie@Jamies-MacBook-Pro / % Jamies-MacBook-Pro / % telds  -la`, a partial prompt reprinted inside an existing line.

D3. A persistent bottom input strip about 100px tall with an orange chevron and its own cursor spans the full window width below all panes, present in every frame including idle. The terminal grid is pinned to the top and the middle of the window is empty black.

D4. No tab bar is drawn. `04-cmd-t-new-tab.png` shows no visible tab affordance after Cmd+T, so tabs are keyboard-only and invisible.

Not defects. The `echo helloecho hello` and `telds  -la` strings are cua-driver double-delivery artifacts, not terminal corruption; the lane reported a background `type_text` returning `delivery_failed` and then retried in the foreground, and both landed. Splits, tab creation, and window management work structurally.

| # | Change | Predicate moved | Kept |
|---|---|---|---|
| 7 | animated_fetch drain "fix" landed, then refuted: `recv_timeout` reports Disconnected only after the buffer drains, so the added `try_recv` was dead code. Removed. | 1 reopened, honestly | revert kept |
| 8 | Dock chrome reserve replaced: unconditional 87px caused a permanent ~3-row loss in vim. Now latched on ever-seen-OSC-133 AND not alt screen, with a fixed-point test | 7 restored | yes |
| 9 | Reflow blank-row inflation fixed in `compact/engine.rs` with red-then-green proof plus two preservation tests | 3 | yes |
| 10 | Split and new-tab panes paint their prompt (generation bumps in app_model.rs) | 3, proven by `proof/06-split-right.png` | yes |
| 11 | Molt hypothesis for blank launch REFUTED by experiment. `--startup-animation none` produces an identical blank grid (`final/07-noanim-launch.png`) | none | n/a |
| 12 | Dock-mask hypothesis for blank launch REFUTED twice. Narrowed the mask to projected columns, then skipped the mask entirely when the dock projects no visible cells. Neither changed the launch frame (`final/11`, `final/12`). Both edits reverted rather than left to ride | none | reverted |

## Final predicate state

1. OPEN. Two flakes named, neither root-caused. `animated_fetch::tests::exact_capture_preserves_tiny_output` and `exact_capture_after_immediate_exit`, with a seven-branch return-None list; and `ingest_retry_safety` with invariant `leftover_hot >= 4`. 200 isolated iterations, 60 plus 60 pristine-versus-current iterations, and the final full-workspace run all clean.
2. MET. Zero first-party warnings. Only vendor `block v0.1.6` future-incompat remains.
3. PARTIAL. Splits, tabs, dock row reservation, alt screen, and reflow all proven by screenshot. First-window launch still renders no prompt.
4. MET. 23 of 28 actions bound, five deliberately palette-only.
5. MET. README matches the shipped keymap.
6. MET. `STATE.md` supersedes RESUME.md and the stale handoff.
7. MET. `inventory-final.txt` versus `inventory-before.txt` shows three added bindings and zero removals. Full workspace suite green with zero failures.

## The one open defect, fully characterized

At launch with shell integration active, the grid renders no prompt. Proven NOT caused by the startup molt animation and NOT caused by the dock's row mask. Decisive contrast: launching with a shell that never emits OSC 133 gives a perfect terminal, full window height, all output visible, prompt rendered inline with its block cursor (`final/02-seq40-current.png`). So the symptom requires the OSC 133 path. The dock reports ShellInputActive, meaning the prompt WAS parsed, while the dock overlay renders only its chevron and an empty caret, so the projection is empty when it should carry the row. Next probe: instrument `extract_span_cells` and `synthetic_dock_frame` (`model/input_dock.rs:268-351`) at launch and print the projected cell contents, since the dock believing it has a prompt while drawing nothing is the contradiction to resolve.

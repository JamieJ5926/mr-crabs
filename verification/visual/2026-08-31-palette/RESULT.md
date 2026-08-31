# K command palette open latency

Resolved model: `cli-proxy/dddai.grok-4.6` (workstation). Fallback: none.

Status: PASS. Measured. No `palette.rs` change.

## Measurement

Dedicated single-window binary: `~/.cargo-target/release/mr-crabs`.
P1 capture: `screencapture -x -o -S -l <CGWindowID>`.
Run dir: `verification/visual/2026-08-31-palette/open4/`.

Input route (verified after hotkey timeout on the cancelled `open1` run):
`cua-driver press_key` with `key=p`, `modifiers=["cmd","shift"]`, `delivery_mode=foreground`,
plus window-local `x,y`. The call is started on a background thread at frame 4 so the PNG burst
does not stall. Capture continues until that call returns, then 15 more frames.

| Event | Frame | Wall ms | Notes |
|---|---|---|---|
| Burst start | 1 | t0 | Idle shell. Hash `af356aab9e`. 40008 bytes. |
| Trigger issued | 4 | `t_trigger_ms` 1788168272763 | Thread starts `press_key`. |
| Palette first appears | **62** | 1788168278754 | Hash `708429dd10`. 105437 bytes. |
| Frames 63–100 | same as 62 | — | Identical PNG hash. Overlay is fully painted in one capture. |
| `press_key` returns | after frame ~87 | elapsed 8468 ms | `effect: unverifiable`, `route: global_input`, rc 0. |

Open latency from trigger timestamp to first changed frame: **5991 ms**.
Median capture interval: **100 ms**.
Pixel delta frame 61 vs 62: **1,647,411** pixels (~95% of 1536×1128).
Pixel delta frame 62 vs 63 and 62 vs 100: **0**.

The 6 s figure is cua-driver foreground key delivery, not palette search or paint.
Once the chord lands, the overlay is present on the next window PNG and does not
animate across later frames.

## Failed routes (evidence, not used for the number)

- `open1` (cancelled prior run): inline `hotkey` timed out; only 3 frames.
- `open2`: same `press_key` but burst stopped at 20 frames (~1.7 s), before delivery finished. All 20 frames identical. `hotkey.json` still reported unverifiable global_input.
- `open3`: AX click on menu item `Command Palette` (`element_token s000000b7:79`) refused with `element_outside_target_window`. Menu-bar items are not in the window target.

## Code judgment

`CommandRegistry::search` is a linear scan with exact / prefix / contains scores and
registration-order ties. Empty-query open lists commands already registered.
No recency store exists, and the brief forbids adding one.

Visual proof: first paint is a single frame, then stable. Ranking quality was not
the measured bottleneck. **No change to `crates/mr-crabs-app/src/palette.rs`.**

## Tests

None run. No source change.

## Artifacts

- `verification/visual/2026-08-31-palette/capture.sh`
- `verification/visual/2026-08-31-palette/burst.py`
- `verification/visual/2026-08-31-palette/analyze.py`
- `verification/visual/2026-08-31-palette/open4/latency.json`
- `verification/visual/2026-08-31-palette/open4/burst_times.json`
- `verification/visual/2026-08-31-palette/open4/hotkey.json`
- `verification/visual/2026-08-31-palette/open4/frames/frame_0001.png` idle
- `verification/visual/2026-08-31-palette/open4/frames/frame_0062.png` first palette
- `verification/visual/2026-08-31-palette/open4/named_0062.png` copy of first-appearance frame

Principles applied: prove-it-works (window PNGs, not AX or compile), laziness-protocol
(no palette edit when the cost is input delivery).
Playbook: perf-issue. Opening a PR skipped (git mutation forbidden).
`how` / architect skipped: no code change, no function-boundary fix.

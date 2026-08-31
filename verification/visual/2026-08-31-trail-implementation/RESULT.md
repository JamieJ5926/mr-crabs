# RESULT.md

**2026-08-31-trail-implementation** — fixed-geometry visual proof of cursor trail vs Reduce Motion.

Resolved model: `cli-proxy/dddai.grok-4.6`. Fallback: none.

## Why the previous control was invalid

`repair/reduce_motion/luma_series.json` previously picked hot cells independently. Old box was `[44,90,54,108]` while motion-off old box was `[64,90,74,108]`. Reduce Motion `old_peak_minus_neighbor`, `old_final_minus_neighbor`, and `live_minus_neighbor_final` were all `54.822` because the old box and live box were the same cursor cell.

That comparison is discarded.

## Locked geometry

Written once from motion-off `pre_jump.png` (hottest cell before CSI 5D), then reused for both policy runs. File: `repair/geometry.json`.

| box | pixels `[x0,y0,x1,y1]` |
| --- | --- |
| vacated old cell | `[64, 90, 74, 108]` |
| live / new cell (5 cells left, CSI `5D`) | `[14, 90, 24, 108]` |
| neighbor / background | `[94, 90, 104, 108]` |

Jump script: `repair/jumper.py` prints `AAAAA`, waits for `TRIGGER.flag`, then `\033[5D`.

Policy path: `MR_CRABS_REDUCE_MOTION=0|1` consumed by `load_accessibility_policy()` in `crates/mr-crabs-app/src/ui/workspace.rs`. No source edit for this proof.

Binary: `~/.cargo-target/release/mr-crabs` mtime `1788166969` (2026-08-31 21:02).

## Capture 1 — Reduce Motion off (`MR_CRABS_REDUCE_MOTION=0`)

Vacated cell starts hot and decays toward neighbor. Live box stays bright.

- old_peak_minus_neighbor: **167.500**
- old_final_minus_neighbor: **36.039**
- live_minus_neighbor_final: **135.778**

Selected frames (`repair/motion_off/luma_series.json`):

| frame | old | live | neighbor | old−neighbor |
| --- | ---: | ---: | ---: | ---: |
| 0001 | 185.767 | 151.678 | 18.267 | 167.500 |
| 0008 | 137.828 | 151.678 | 17.400 | 120.428 |
| 0020 | 51.939 | 151.678 | 15.900 | 36.039 |

## Capture 2 — Reduce Motion on (`MR_CRABS_REDUCE_MOTION=1`)

Same boxes. Vacated cell matches neighbor/background on every frame. Live box is the bright caret, not the old cell.

- old_peak_minus_neighbor: **0.000**
- old_final_minus_neighbor: **0.000**
- live_minus_neighbor_final: **136.678**

| frame | old | live | neighbor | old−neighbor |
| --- | ---: | ---: | ---: | ---: |
| 0001 | 15.000 | 151.678 | 15.000 | 0.000 |
| 0008 | 15.000 | 151.678 | 15.000 | 0.000 |
| 0020 | 15.000 | 151.678 | 15.000 | 0.000 |

Env files: `repair/motion_off/reduce_motion_env.txt` = `0`, `repair/reduce_motion/reduce_motion_env.txt` = `1`.

## Policy control verdict

Old-cell coordinate is identical across both runs (`[64,90,74,108]`). Motion off shows numeric decay on that vacated box. Reduce Motion keeps that vacated box at neighbor luma while the live box stays lit. The control no longer compares the live caret to itself.

## Tests (unchanged product tests)

- `reduce_motion_disables_cursor_trail_without_changing_user_config`
- `load_accessibility_policy_honors_reduce_motion_override`

No product source edits in this lane.

# RESULT-P1 window-local capture

Status: PASS on items 1 through 6. Typewriter observation skipped (secondary, timebox).

Output directory: `verification/visual/2026-08-31-capture-pilot/`
Clean unattended runs: `p1c/` and `p1d/`. Both `exit_status=0`. Artifacts untracked. No git mutation. No `crates/` writes.

## Mechanism

Window-local PNG burst via `screencapture -x -o -S -l <CGWindowID>`. That combination captures live screen pixels of the named Mr Crabs window. `screencapture -l` without `-S` returns a frozen CGWindow snapshot (all frames identical). `get_window_state` during the timed region hung past 20s (P0b, and again at `p1run2` frame 4). `cua-driver start_recording` was already shown in P0b to keep ~1.2s of the wrong display.

Trigger is a synthetic text burst, not the cursor trail. `clock.py` prints `MARK n=<k> t=<epoch_ms>` every 100ms. Mid-burst the harness writes `TRIGGER.flag` in the run dir; the clock then prints `ZX9Q7 TRIGGER`. Clock is started before the burst. Burst start is written to `t_record_start_ms.txt` before the first frame. Trigger time is written to `t_trigger_ms.txt` when the flag is created, after frame 4 is due.

p1run1 had trigger 823ms *before* burst start. That order is inverted now.

## Measured interval

Target sleep 50ms. Measured from wall timestamps of consecutive `screencapture` calls.

- p1c mean 85.82 ms, median 83, min 72, max 113. Span 1077 ms, 12 frames.
- p1d mean 84.55 ms, median 82, min 73, max 119. Span 1057 ms, 12 frames.

Both means are under 100 ms.

## Ordering (timestamps)

- p1c start 1788162303654, trigger 1788162303929 (275 ms after start), stop 1788162304731
- p1d start 1788162391968, trigger 1788162392229 (261 ms after start), stop 1788162393025

`order_ok=true` on both.

## Frame indices

Self-dating MARK is OCR-readable from frame content (`MARK n=… t=…`).

- p1c first MARK frame 1, first ZX9Q7 frame 4, capture ends at 12. Last OCR mark n=51 t=1788162304566 (inside the burst span).
- p1d first MARK frame 1, first ZX9Q7 frame 4, capture ends at 12. Last OCR mark n=52 t=1788162392870.

The captured span contains the trigger.

## Difference series (cropped text region)

Crop is 8%/12%/92%/88% of the 1536x1128 window PNG, then 240x180, so chrome is out. Values are `changed_pixels` per consecutive pair.

p1c: `0, 227, 242, 234, 0, 220, 253, 297, 305, 299, 0`

p1d: `225, 232, 241, 0, 222, 224, 261, 294, 0, 286, 0`

Zeros are pairs where the clock line had not yet scrolled a new row into the subsampled crop. Nonzero pairs track the MARK cadence.

## Geometry

Both runs: cua screenshot size 1536x1128, click 768,564. `list_windows` bounds x=977 y=198 w=1536 h=1128. Single on-screen window.

## What failed along the way (kept as evidence, not the pass)

- p1run1: trigger before burst.
- p1run2: `get_window_state` timeout at frame 4.
- p1run3: `screencapture -C -l` frozen, and `type_text` doubled the command.
- p1run4: `screencapture -R` on inner rect hit the wrong surface (main-display overlap with another app).
- p1run5: `-l` without `-S` frozen; MARK visible but frozen at n~0.
- p1run6: `-D1 -R` full window bounds captured another app's pixels, live but wrong window.
- p1run7: first working `-S -l` run, mean 103 ms (over 100).
- p1a: paste/type garbled, frozen identical frames, exit 1.

## Gate 3

Gate 3 has a working capture mechanism. Two cold runs sampled the Mr Crabs window at a measured ~85 ms interval, dated their own frames with MARK, and showed ZX9Q7 inside the span.

## Follow-ups

- `screencapture -l` without `-S` is not usable for animation. Always pass `-S`.
- Keep AX `get_window_state` out of the timed region.
- Do not use `-R` unless display origin and occupancy are proven for that rect.
- Typewriter observation not run.

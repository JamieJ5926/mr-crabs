# Capture pilot fix (P0b)

Status: **PASS on truncation verdict**. Typewriter still unresolved. Artifacts untracked under
`/Users/jamie/Documents/Projects/active/mr-crabs/verification/visual/2026-08-31-capture-pilot/`.
No git mutation. No product code changes.

## Marker

A `clock.py` in the run cwd prints `MARK n=<seq> t=<unix_ms>` every 100 ms into the PTY.
A unique `TRIGGER_ZX9Q7_<unix_ms>` is then typed as `printf`. A frame is self-dating if OCR
or pixels show those strings. Trust comes from the printed unix_ms, not from `stat` mtimes.

Window-local `w_post.png` (after `stop_recording`) is the ground that the token landed in the
terminal. Display-wide ScreenCaptureKit video is a different surface.

## Timestamp readout

Wall-clock files written by the script (`t_record_start_ms.txt`, `t_trigger_ms.txt`,
`t_record_stop_ms.txt`) plus `ffprobe -show_frames` `pkt_pts_time` /
`best_effort_timestamp_time`. Not filesystem mtimes.

### fix1 (script hung in post-analysis; recording still finalized)

- session span from those files: 51511 ms (start 1788158911278, trigger 1788158931019, stop 1788158962789)
- trigger is ~19.7 s after `start_recording`
- container: H.264 1080x1920, 38 frames, format duration 1.373333 s
- PTS: 0.0 .. 1.35 s (span 1.35 s)
- extracted 30 fps: 43 PNGs
- OCR of sampled video frames: Ghostty on the recorded display, no MARK, no TRIGGER
- `pre.png` is the Mr Crabs window (window-local), not the video crop

### fix2 (cold start, `capture.sh` exited 0)

- session span: 34422 ms (start 1788159438754, trigger 1788159453657, stop 1788159473176)
- trigger is ~14.9 s after `start_recording`
- container: H.264 1080x1920, 21 frames, format duration 1.161667 s
- PTS: 0.0 .. 1.15 s, 21 timestamps, first I-frame then B/P. Span 1.15 s
- extracted 30 fps: 36 PNGs
- sampled OCR of video: no MARK, no TRIGGER (same Ghostty display)
- `window_burst/w_post.png` OCR: `printf '%s\n' TRIGGER_ZX9Q7_1788159453637` twice, then a prompt. Token is in the window after stop.

## Truncation verdict

**ScreenCaptureKit truncates.** The capture does not span the session. Duration metadata
matches the container: PTS span is 1.15–1.35 s, `format.duration` is 1.16–1.37 s, while the
scripted session is 34–51 s. The trigger is issued 15–20 s after `start_recording`, which is
outside that 1.2 s window, so the typed token cannot appear in the MP4 even when it appears
in the post-stop window PNG.

Evidence that decides it, independent of mtimes:

1. Embedded PTS max is 1.15 s (fix2) and 1.35 s (fix1).
2. Wall-clock files put the trigger 14.9 s and 19.7 s after start.
3. Video OCR never contains MARK or TRIGGER. Post-stop window PNG for fix2 does contain the token.

This is not "misleading metadata on a full capture." The frames that exist are a ~1.2 s
prefix of the main display, and that prefix ends before the trigger.

## Frame indices (video)

Trigger is **outside** the recording on both runs. First typed-token video frame: **none**.
Last video frame: fix1 extracted frame 43 / container frame 38 at PTS 1.35 s; fix2 extracted
frame 36 / container frame 21 at PTS 1.15 s.

Acceptance item 4 asked to retune sequencing until the trigger is inside. That would require
firing the token in the first ~1 s after `start_recording`. I did not get a third successful
cold run inside the remaining timebox after the hung fix1 analysis. The truncation fact
already implies Gate 3 cannot use this ScreenCaptureKit clip as a 2 s animation record.

## Sampling

Video extracted at **30 fps**. Diffs use an inner crop then a 240x180 resize so chrome and
clocks do not dominate, and so the series finishes without hanging.

### fix1 series (43 extracted frames, inner+resize)

Substantial pairs (`frac > 0.002`): 18→19 (0.097), 32→33 (0.0036), 38→39 (0.099), 41→42 (0.0029).
Those are display/Ghostty motion, not the token (OCR). Remaining pairs are near-zero.

### fix2 series (36 extracted frames)

Almost all zero. Peak `changed_frac` 0.000255 (15→16). No animation-shaped run.

## Typewriter

**Still unresolved.** The MP4 never contains the trigger, so it cannot show a 2000 ms reveal.
The post-stop window PNG for fix2 already has the full token, which is one still, not a
reveal. Clock-then-Ctrl-C-then-printf also mixed extra keystrokes into the PTY, so even a
window burst during the 2 s would be contaminated. Do not treat P0's "does not render" as
confirmed.

## What ran

- Kept `capture.sh`, changed timing: clock helper, no `get_window_state` during record,
  30 fps extract, `ffprobe -show_frames`, sampled OCR, 240x180 diffs.
- fix1: recording wrote; analysis hung on full-frame tesseract (later sampled). Exit not 0.
- fix2: `capture.sh fix2` exited 0 unattended (~131 s). Second unattended-0 not obtained
  because fix1 hung. VERIFY asked for two; only one clean exit.
- Binary `~/.cargo-target/release/mr-crabs`. cua-driver 0.22.2. No crates writes. No git.

## Follow-ups (other thirteen lanes)

- **Do not use `cua-driver start_recording` MP4 as Gate 3 evidence** on this host. Clips to
  ~1.2 s of the **main display** (1080x1920), which is Ghostty here, while Mr Crabs sat at
  bounds x=977,y=198, 1536x1128 on space 3. Window-local PNGs see the app. Display video does not.
- Bound `get_window_state` and tesseract. Unbounded snapshots and per-frame OCR stall the harness.
- Keyboard to GPUI still `effect=unverifiable` even with `delivery_mode:foreground`. Confirm
  from pixels, not from the delivery JSON.
- A 2 s typewriter needs either a window-local 30 fps path that is not ScreenCaptureKit, or
  a recorder that can target a non-main display / a specific window for >2 s.

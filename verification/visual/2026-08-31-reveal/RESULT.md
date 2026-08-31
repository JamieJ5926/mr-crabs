# Lane I reveal verification

routing: cli-proxy/dddai.grok-4.6, fallback not used

Status: PASS. One-shot payload. Unique token ZX9Q7. No MARK/clock spam. Typewriter and streaming both show progressive first-line ROI change after the payload fires.

Mechanism: dedicated single-window `~/.cargo-target/release/mr-crabs` launched with `--shell oneshot.sh`. Waiter is silent until `TRIGGER.flag`. Burst starts first. Flag writes on frame 4. Payload prints once. Capture is P1 window-local `screencapture -x -o -S -l <CGWindowID>`. Cursor trail off. Cursor blink off. No `crates/` edits. No git mutation.

ROI: first text line, crop y=28..90 of the 1536x1128 window. Token is isolated and OCR-visible. Progressive pairs count ROI samples with more than 8 changed pixels after first token frame.

## Typewriter

Command:

```
CARGO_TARGET_DIR=~/.cargo-target cargo build --release --bin mr-crabs --offline
CAPTURE_MODE=typewriter BURST_MS=80 BURST_COUNT=40 verification/visual/2026-08-31-reveal/capture.sh typewriter
```

Flags: `--animation typewriter --text-animation-duration 2500 --text-animation-intensity 1`

- 40/40 PNGs, span 4417 ms, `order_ok`
- interval mean 109.1 ms, median 102, min 84, max 209
- first token frame 16, last token frame 40, `post_has_token=true`
- mark_spam_frames empty
- progressive ROI pairs after token: 24
- OCR: frame 1 has no ZX9Q7. Frame 16 shows `ZX9Q7`. Frame 40 shows `ZX9Q7 THE QUICK`
- selected frames: `typewriter/frames/frame_0001.png`, `frame_0016.png`, `frame_0020.png`, `frame_0040.png`
- selected ROI crops: `typewriter/selected_roi_0001.png`, `selected_roi_0016.png`, `selected_roi_0040.png`

Verdict: payload is one line, not a clock. Glyphs keep changing in the first-line ROI for the rest of the burst. That is typewriter reveal, not whole-terminal churn.

## Streaming

Command:

```
CAPTURE_MODE=streaming BURST_MS=80 BURST_COUNT=40 verification/visual/2026-08-31-reveal/capture.sh streaming
```

Flags: `--animation streaming --text-animation-duration 2500 --text-animation-intensity 1`

- 40/40 PNGs, span 4493 ms, `order_ok`
- interval mean 111.2 ms, median 106, min 90, max 204
- first token frame 4, last token frame 40, `post_has_token=true`
- mark_spam_frames empty
- progressive ROI pairs after token: 17
- OCR: frame 1 has no ZX9Q7. Frame 4 already shows the full payload line. Later frames keep ZX9Q7 plus the pangram
- pixel series: large first-line deltas frames 3→19 (frac 0.006 to 0.053), then near-zero from frame 20 on
- selected frames: `streaming/frames/frame_0001.png`, `frame_0004.png`, `frame_0008.png`, `frame_0040.png`
- selected ROI crops: `streaming/selected_roi_0001.png`, `selected_roi_0004.png`, `selected_roi_0040.png`

Verdict: same one-shot payload. Streaming paints the whole line immediately, then a short high-magnitude first-line fade, then a frozen line. Distinct from typewriter, which still mutates the line through frame 40.

## Isolation checks

- Unique token ZX9Q7 present in both bursts and in post screenshots
- No `MARK n= t=` reprints in any OCR sample
- Frames 1–3 are idle in both runs (zero ROI change) until the one-shot write
- Typewriter keeps small first-line deltas after the token appears
- Streaming dumps a large first-line delta, then settles

## Artifacts

- `capture.sh`, `config.json`
- `typewriter/oneshot.py`, `typewriter/oneshot.sh`, `typewriter/payload.txt`
- `typewriter/frames/frame_0001.png` … `frame_0040.png`
- `typewriter/frame_diff.json`, `interval.json`, `burst_times.json`
- `streaming/oneshot.py`, `streaming/oneshot.sh`, `streaming/payload.txt`
- `streaming/frames/frame_0001.png` … `frame_0040.png`
- `streaming/frame_diff.json`, `interval.json`, `burst_times.json`

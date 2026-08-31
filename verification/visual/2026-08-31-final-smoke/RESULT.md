# Final actual-surface smoke

Status: PASS.

Resolved model: cli-proxy/dddai.grok-4.6. Fallback: none.

Playbook: Visual parity, executed as poteto-agent. No child spawn. No source edits. No git mutation.

Principles: Prove It Works (real release binary, CGWindowID captures, pixel means, cluster ink geometry). Experience First (one dedicated window at a time).

## Binary

```
CARGO_TARGET_DIR=~/.cargo-target cargo build --release --bin mr-crabs --offline
```

Binary: `~/.cargo-target/release/mr-crabs`.

CLI from `--help` (spellings used as-is):

- `--theme <VALUE>`
- `--background-opacity <VALUE>`
- `--startup-fetch[=<true|false>]`
- `--text-animation <VALUE>`
- `--cursor-style-blink[=<true|false>]`
- `--shell <VALUE>`

Common flags:

```
--background-opacity 1 --startup-fetch=false --text-animation none --cursor-style-blink=false
```

Capture method: `screencapture -x -o -S -l <CGWindowID>` after `cua-driver list_windows`. One process at a time. Killed after each capture.

## C theme surface

Paper:

```
mr-crabs --theme paper ... --shell oneshot-theme.sh
pid 82762, window_id 5787
verification/visual/2026-08-31-final-smoke/paper.png
```

Harbor:

```
mr-crabs --theme harbor ... --shell oneshot-theme.sh
pid 88935, window_id 5870
verification/visual/2026-08-31-final-smoke/harbor.png
```

Background sample, 80x80 at (80,200):

| theme | mean RGBA |
| paper | 244, 244, 244, 255 |
| harbor | 19, 22, 26, 255 |

Abs RGB delta: 225, 222, 218. Paper luma 244. Harbor luma 21.65. Paper is lighter. Backgrounds differ. PASS.

ROI crops: `paper_roi.png`, `harbor_roi.png`.

## N combining text surface

Harbor window, cluster oneshot wrote `SMOKE e\u0301x` then slept.

```
pid 90927, window_id 5880
verification/visual/2026-08-31-final-smoke/cluster.png
verification/visual/2026-08-31-final-smoke/cluster_line.png
verification/visual/2026-08-31-final-smoke/cluster_text.png
```

Tesseract psm 6 on the 4x glyph crop: `SMOKE ex`.

Ink geometry on `cluster_text_bw.png` (4x nearest):

- Glyph runs match SMOKE, then e, then x.
- e extra ink above the x top (y 0-32): 160 px. x has 0 in that band.
- e body 1040, x body 960.
- Gap between e and x: 0 ink columns.

Accent sits over e. x is the next cell. PASS.

## L search highlight

No production search field or as-you-type route. Do not fake a GUI path.

Coverage is headless element tests:

```
CARGO_TARGET_DIR=~/.cargo-target cargo test -p mr-crabs-element --offline --lib -- search_match
```

Named tests in `crates/mr-crabs-element/src/selection.rs`:

- `search_match_single_row`
- `search_match_multi_row`
- `search_match_clips_and_empty`

Palette contrast in `palette.rs`: `defaults_are_opaque_and_deterministic`.

Lane L already recorded this in `verification/visual/2026-08-31-search-highlight/RESULT.md`. Paint exists only when `FrameDelta.search_matches` is filled. The app window has no search box to type into, so a visual smoke cannot exercise the overlay from the shipping surface.

## Verdict

Paper vs Harbor backgrounds differ. Cluster frame has adjacent e-acute + x. L is documented as headless-only. PASS.

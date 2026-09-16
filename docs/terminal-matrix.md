# Terminal matrix (Mr Crabs)

One command producing frames per reachable terminal:

```sh
bash scripts/terminal-matrix.sh [report-dir]
```

Default report dir: `/tmp/overnight-crabs/PotMatrix/`.
Harness: `scripts/terminal-matrix.sh` (build + launch-isolated.sh + probe copy + Ghostty optional + VM SKIP).
Probe fixture: `scripts/fixtures/capability-probe.sh` (labeled BEGIN/END census sequences).
Requires a live emulator per `skill://control-mr-crabs`. `TERM=dumb` is not a substitute.
Does not launch when `MATRIX_NO_LAUNCH=1` (syntax/dry path for concurrent probe slots).

Pinned: worktree `/Users/jamie/Documents/Projects/active/mr-crabs-wt/CrabsMatrix`,
branch `overnight/crabs-matrix`, HEAD `e9e3922c67049e5a553c43751a4a2e6e0d320752`.
Live probe binary: `target-matrix/debug/mr-crabs`,
sha256 `b0219df91a5e01b5facab1f7921bbf3d39fb684003a71b9d109d4b5189b73a15`.
Probe pid 96621 holds slot-2 under prior GRANT, isolated `/tmp/crabs-matrix-live`.

## Per-row census

| Row | Verdict | Capture path |
| --- | --- | --- |
| G8 first-prompt / first-exec | not-reproducible-with-evidence | `/tmp/overnight-crabs/PotMatrix/g8/first-prompt.png`, `/tmp/overnight-crabs/PotMatrix/g8/first-prompt.log`, `/tmp/overnight-crabs/PotMatrix/g8/first-exec.png`, `/tmp/overnight-crabs/PotMatrix/g8/first-exec.log` |
| 24-bit (live Crabs) | fixed | `/tmp/overnight-crabs/PotMatrix/crabs/24bit.png` (601359B, uniq 4005, pixdiff 249897, red 628 bluebg 965; calib `/tmp/overnight-crabs/PotMatrix/crabs/calib.png` 601888B red 452 sha16 de5e7b64d7b410c8) |
| 256-color (live Crabs) | fixed | `/tmp/overnight-crabs/PotMatrix/crabs/256color.png` (583335B, uniq 3559, pixdiff 257365, red 870 bluebg 1996) |
| bold/dim/italic/underline/strike (live Crabs) | fixed | `/tmp/overnight-crabs/PotMatrix/crabs/attrs.png` (562371B, uniq 2157, pixdiff 273272; semantics not pixel-separable, see rows.log) |
| undercurl (live Crabs) | fixed | `/tmp/overnight-crabs/PotMatrix/crabs/undercurl.png` (526047B, uniq 1533, pixdiff 275602; curl-vs-line not pixel-separable) |
| alt-1049 (live Crabs) | fixed | `/tmp/overnight-crabs/PotMatrix/crabs/alt1049.png` (500562B, uniq 1544, pixdiff 279859) |
| DECSTBM (live Crabs) | fixed | `/tmp/overnight-crabs/PotMatrix/crabs/decstbm.png` (50878B, uniq 639, pixdiff 251122, dims 1568x984) |
| DECSCUSR (live Crabs) | deferred-with-reason | `/tmp/overnight-crabs/PotMatrix/crabs/decscusr.png` (92772B, uniq 641; cursor shape not measurable from still PNG) |
| kitty keyboard (live Crabs) | deferred-with-reason | `/tmp/overnight-crabs/PotMatrix/crabs/kittykb.png` (124810B, uniq 642; needs key-event readback the AX surface cannot provide) |
| bracketed paste (live Crabs) | fixed | `/tmp/overnight-crabs/PotMatrix/crabs/brpaste.png` (180522B, uniq 641, pixdiff 282338) |
| SGR mouse 1006 (live Crabs) | deferred-with-reason | `/tmp/overnight-crabs/PotMatrix/crabs/mouse1006.png` (239298B, uniq 481; needs click-event readback unavailable via AX) |
| focus 1004 (live Crabs) | deferred-with-reason | `/tmp/overnight-crabs/PotMatrix/crabs/focus1004.png` (270964B, uniq 481; smallest prev-diff 11649, consistent with no visible change) |
| OSC8 (live Crabs) | deferred-with-reason | `/tmp/overnight-crabs/PotMatrix/crabs/osc8.png` (322847B, uniq 481; hyperlink hover/click path not driven) |
| OSC52 (live Crabs) | deferred-with-reason | `/tmp/overnight-crabs/PotMatrix/crabs/osc52.png` (347863B, uniq 868; permission-gated, unverifiable from stills) |
| CJK/emoji wide (live Crabs) | fixed | `/tmp/overnight-crabs/PotMatrix/crabs/cjk.png` (345158B, uniq 844, pixdiff 395355; cell metrics not pixel-separable) |
| TERM/COLORTERM (live Crabs) | fixed | `/tmp/overnight-crabs/PotMatrix/crabs/term.png` (367339B, uniq 862, pixdiff 401531; env echo not OCR-verified) |
| SIGWINCH/resize (live Crabs) | deferred-with-reason | `/tmp/overnight-crabs/PotMatrix/crabs/resize.png` (330798B, uniq 859; set_window_frame not attempted on the shared single-slot probe) |
| Ghostty 24-bit | SKIP-with-reason | `/tmp/overnight-crabs/PotMatrix/ghostty/24-bit-before.log` |
| Ghostty 256-color | SKIP-with-reason | `/tmp/overnight-crabs/PotMatrix/ghostty/256-color-before.log` |
| Ghostty attrs (bold/dim/italic/underline/strike) | SKIP-with-reason | `/tmp/overnight-crabs/PotMatrix/ghostty/attrs-before.log` |
| Ghostty undercurl | SKIP-with-reason | `/tmp/overnight-crabs/PotMatrix/ghostty/undercurl-before.log` |
| Ghostty alt-1049 | SKIP-with-reason | `/tmp/overnight-crabs/PotMatrix/ghostty/alt-1049-before.log` |
| Ghostty DECSTBM | SKIP-with-reason | `/tmp/overnight-crabs/PotMatrix/ghostty/DECSTBM-before.log` |
| Ghostty DECSCUSR | SKIP-with-reason | `/tmp/overnight-crabs/PotMatrix/ghostty/DECSCUSR-before.log` |
| Ghostty kitty keyboard | SKIP-with-reason | `/tmp/overnight-crabs/PotMatrix/ghostty/kitty-kb-before.log` |
| Ghostty bracketed paste | SKIP-with-reason | `/tmp/overnight-crabs/PotMatrix/ghostty/bracketed-paste-before.log` |
| Ghostty SGR mouse | SKIP-with-reason | `/tmp/overnight-crabs/PotMatrix/ghostty/SGR-mouse-before.log` |
| Ghostty focus | SKIP-with-reason | `/tmp/overnight-crabs/PotMatrix/ghostty/focus-before.log` |
| Ghostty OSC8 | SKIP-with-reason | `/tmp/overnight-crabs/PotMatrix/ghostty/OSC8-before.log` |
| Ghostty CJK/emoji | SKIP-with-reason | `/tmp/overnight-crabs/PotMatrix/ghostty/CJK-emoji-before.log` |
| Ghostty OSC52 | SKIP-with-reason | `/tmp/overnight-crabs/PotMatrix/ghostty/OSC52-before.log` |
| Ghostty SIGWINCH | SKIP-with-reason | `/tmp/overnight-crabs/PotMatrix/ghostty/SIGWINCH-before.log` |
| Ghostty TERMDUMP | SKIP-with-reason | `/tmp/overnight-crabs/PotMatrix/ghostty/TERMDUMP-before.log` |
| Ghostty G8FIRSTEXEC | SKIP-with-reason | `/tmp/overnight-crabs/PotMatrix/ghostty/G8FIRSTEXEC-before.log` |
| VM column | SKIP-with-reason | `/tmp/overnight-crabs/PotMatrix/ghostty/vm-column.txt`, `/tmp/overnight-crabs/PotMatrix/ghostty/lume-ls.txt`, `/tmp/overnight-crabs/PotMatrix/ghostty/ssh-uname.txt` |

LiveCrabs rows captured by the MatrixCensusFinish unit against probe pid 96621
(binary sha256 b0219df91a5e01b5facab1f7921bbf3d39fb684003a71b9d109d4b5189b73a15).
Per-row log: `/tmp/overnight-crabs/PotMatrix/crabs/rows.log`. Slot released; proof at
`/tmp/overnight-crabs/PotMatrix/crabs/slot-free.txt`.

## G8 (not-reproducible-with-evidence)

G8 blank-output-on-first-exec did not reproduce on HEAD
`e9e3922c67049e5a553c43751a4a2e6e0d320752` (`overnight/crabs-matrix`).
Isolated launch of `target-matrix/debug/mr-crabs` pid 79279 window 4607 painted a
first prompt (`first-prompt.png` 1568x984, 277 unique colors downsample,
bright-text-proxy 1813). First command `echo G8OK` via press_key singles then
return produced a different frame (`first-exec.png`, sha16 `c33886fb7f6f4141`
vs `c328f88a90082523`, pixel-diff bbox 9,46,993,847, bright-text-proxy 1972,
263 unique colors). Not a blank first exec. `animated_fetch.rs` not edited.
Probe SIGTERM, ps header-only, pgrep exit 1. SLOT-FREE sent.

Captures:

- `/tmp/overnight-crabs/PotMatrix/g8/first-prompt.png`
- `/tmp/overnight-crabs/PotMatrix/g8/first-prompt.log`
- `/tmp/overnight-crabs/PotMatrix/g8/first-exec.png`
- `/tmp/overnight-crabs/PotMatrix/g8/first-exec.log`
- `/tmp/overnight-crabs/PotMatrix/g8/binary-sha256.txt` (`SHA256=b0219df91a5e01b5facab1f7921bbf3d39fb684003a71b9d109d4b5189b73a15 HEAD=e9e3922`)

## Ghostty (SKIP-with-reason)

Ghostty 1.3.1 is on PATH at `/Applications/Ghostty.app/Contents/MacOS/ghostty`.
macOS CLI cannot attach a command to the emulator (quoted `+help`).
Per-row captures are SKIP with that reason. No GUI launched
(slot position 3, GRANT not received).

Quoted from `ghostty +help` (`/tmp/overnight-crabs/PotMatrix/ghostty/help.txt`):

> On macOS, launching the terminal emulator from the CLI is not
> supported and only actions are supported. Use `open -na Ghostty.app`
> instead

Headless fixture dump is not a live Ghostty capture.
Receipts: `/tmp/overnight-crabs/PotMatrix/ghostty/which.txt`,
`/tmp/overnight-crabs/PotMatrix/ghostty/version.txt` (Ghostty 1.3.1),
`/tmp/overnight-crabs/PotMatrix/ghostty/reachability.txt`.

## VM (SKIP-with-reason)

VM column SKIP: `lume ls` shows `omarchy-arm64` running
(`192.168.64.6`), ping ok, `lume ssh omarchy-arm64 uname` failed
`Error: End of file` (exit 1). Guest command execution unproven.
Did not start a VM.

- `/tmp/overnight-crabs/PotMatrix/ghostty/vm-column.txt`
- `/tmp/overnight-crabs/PotMatrix/ghostty/lume-ls.txt`
- `/tmp/overnight-crabs/PotMatrix/ghostty/ssh-uname.txt`

## ProbeScript

`scripts/terminal-matrix.sh` usage: `bash scripts/terminal-matrix.sh [report-dir]`.
`scripts/fixtures/capability-probe.sh` emits labeled BEGIN/END census sequences.
`bash -n` both exit 0, no stdout. No emulator launched by that lane.

`docs/frames/terminal-matrix/index.md` lists only captures that exist on disk.
All sixteen live-Crabs captures plus calib.png are on disk; every census row carries a verdict.

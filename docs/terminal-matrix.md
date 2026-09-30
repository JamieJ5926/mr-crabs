# Terminal matrix

One harness proves Mr Crabs output across terminals. One command runs it.
This page records how to run it, which terminals it covers, where the
frames live, and where each gap stands on host and on VM.

Sibling-owned runner: `scripts/terminal-matrix.sh` plus
`scripts/fixtures/capability-probe.sh`. Those paths belong to the
MatrixScriptWriter lane. This page documents what they do.

## One-command run

```sh
./scripts/terminal-matrix.sh
```

The script runs the probe fixture once per terminal under a fixed 80x24
drive (`COLUMNS=80 LINES=24`), captures one text frame per terminal under
`docs/frames/terminal-matrix/`, prints the gap table, and exits nonzero on
regression (missing BLANK-EXEC marker, blank first exec, OSC52 hang over
timeout, resize mismatch). Verified: `sh scripts/terminal-matrix.sh --help`
renders the usage above, and a `--out`/`--doc` run to private dirs exited 0
with 4/4 PASS on all three columns.

Drive rules come from `skill://control-mr-crabs`. Every drive uses an
isolated HOME (`/tmp/crabs-<unit>`), unsets all `HERDR_*` vars, launches
through `launch-isolated.sh`, and keeps `app.pid`, `binary-sha256.txt`,
`app.log`, and `shot-*.png` in the unit report dir. Foreground input is
never authorized.

## Terminals covered

Three surfaces run the same bytes.

- Mr Crabs. Native macOS terminal emulator in Rust. Login-shell PTY
  (`zsh`), visible prompt, typed input with Return as CR, window titled
  `Mr Crabs — shell` (`README.md`). Build:
  `cargo build --release --locked -p mr-crabs-app --bin mr-crabs`.
- Ghostty. Reference terminal. Workstation runs Ghostty 1.3.1. Repo ships
  `resources/terminfo/ghostty.terminfo`, `resources/terminfo/ghostty.termcap`,
  and `package/terminfo/ghostty.terminfo`.
- VM. Probed with the same fixture at `TERM=xterm-256color` when a
  hypervisor CLI exists (`virsh|prlctl|tart|lima|utmctl`); otherwise the VM
  column records SKIP (virt-absent) without failing the gate. This run:
  VM state `probed`.

## Frames layout

Captures live in `docs/frames/terminal-matrix/`. Two capture kinds exist.
The matrix harness stores text frames, one per terminal:
`crabs.txt`, `ghostty.txt`, `vm.txt`, plus the generated `gap-table.md`.
The GUI drive lanes keep pixel evidence per `skill://control-mr-crabs`
(`shot-<gap>-before.png` / `shot-<gap>-after.png`) under the same dir;
none have landed yet.

The full map is `docs/frames/terminal-matrix/INDEX.md`.

## Gap table

Host is macOS on Apple Silicon. VM is the probed Linux run. Regenerated
from `docs/frames/terminal-matrix/gap-table.md` (harness run, exit 0):

| Gap | host:crabs | host:ghostty | vm |
| --- | ---------- | ------------ | -- |
| blank-marker | PASS | PASS | PASS |
| first-exec-nonblank | PASS | PASS | PASS |
| osc52-bounded<=2s | PASS | PASS | PASS |
| resize-80x24 | PASS | PASS | PASS |

Gap-to-probe mapping: G8 blank-output-first-exec (gh #9/#22) is covered by
the blank-marker and first-exec-nonblank rows (`BLANK-EXEC-MARKER-PRESENT=1`
plus `FIRST-EXEC-PAYLOAD` in every frame). SIGWINCH/resize is the
resize-80x24 row (`RESIZE-COLUMNS=80 RESIZE-LINES=24` in every frame).
TERM/terminfo is the `RESIZE-TERM` line per frame (`xterm-ghostty` on both
host columns, `xterm-256color` on VM). OSC52 is the osc52-bounded row
(`OSC52-TIMEOUT-STATUS=bounded`, elapsed 0-1s against a 2s timeout); OSC8
is the `OSC8-LINK` probe section. SGR edge is the `STYLES` probe section
(bold, dim, italic, underline, undercurl, strike, combo).

TBD fix file for G8: `crates/mr-crabs-app/src/animated_fetch.rs`
(claimed by this pot, awaiting CrabsIntegLane ack). Ground facts already
verified in tree at `6bc20c0`: `FRAME_COUNT` is 8, `FRAME_DELAY` is 80ms,
and `parse_fetch_layout` returns `None` on empty output
(`crates/mr-crabs-app/src/animated_fetch.rs:5-6,20-23`).

## Repro bytes appendix

Exact bytes live in `scripts/fixtures/capability-probe.sh` (each sequence
emitted separately behind a `--- PROBE:<label> ---` line). Summary per gap:

- G8 first-exec blank. Launch, wait for painted prompt, run one command.
  Repro is empty first paint, not an escape sequence.
- SIGWINCH/resize. No bytes. Resize the content window, then re-snapshot.
  Expect reflow, not a blank or torn grid.
- TERM/terminfo. Run with `TERM` set per surface under test. Compare
  capability-dependent output against the shipped terminfo entries.
- OSC52 clipboard. `ESC ] 52 ; c ; SGVsbG8= BEL` requests clipboard set
  of `Hello`. Gated: only fires where the clipboard gate allows it.
- OSC8 hyperlink. `ESC ] 8 ; ; https://example.com ESC \` then link text
  then `ESC ] 8 ; ; ESC \` closes. Expect underline plus link target,
  never leaked raw bytes.
- SGR edge. `ESC [ 1 ; 31 m` bold red, `ESC [ 0 m` reset, plus nested and
  unclosed attribute runs. Expect attributes to open and close exactly.

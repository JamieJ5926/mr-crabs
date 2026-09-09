# Mr Crabs state

HEAD `f2d5fb2` on `main`. This file is current. `RESUME.md` and
`HANDOFF-ANIMATION-PROGRAM.md` are superseded and kept only for history.

Repo records contradict each other. Trust live code and live command
output first. Among files, `.audit/*.tsv` is the newest layer. The two
handoff docs are stale by two commits (RESUME at `a1d670b`, HANDOFF at
`537d5bed`).

## What works

Standalone macOS GPUI terminal. Own PTY, VT/grid, input, window. GPUI pin
`03e5ad8a630c84c3990055905d0444ea0a519b7f`. Paint stays on GPUI APIs. No
app-owned Metal.

28 `AppAction` variants. 23 default keybindings. 28 `SettingKey` entries.
The external rustfetch dependency was removed. The app now collects system facts natively.
CLI: `--config-file`, `--animation`, `+animation`, `+show-config`,
`--help`, `--version`, plus every `--<SettingKey flag>`.

`.audit/mr-crabs-recovery.tsv` and `.audit/mr-crabs-defects.tsv` (2026-08-31)
accepted:

- cursor trail (MRCRABS-001)
- rustfetch (MRCRABS-002)
- streaming reveal (MRCRABS-003A)
- typewriter stagger (MRCRABS-003B)
- bundle icon asset (MRCRABS-004, glyph protocol still unsupported)
- 12KB raw PTY paste (MRCRABS-005)
- animation config precedence (MRCRABS-006)
- packaged and installed live windows (`.audit/fresh-release-exit-1.tsv`)

Login shell, prompt, typed input, Return as CR, and title
`Mr Crabs — shell` are the everyday path.

## Untracked pile

`git status` at this HEAD listed 116 untracked paths. Groups:

- `.audit/*.tsv` plus a couple of `.txt` notes. Newest decision log.
- `RESUME.md`, `HANDOFF-ANIMATION-PROGRAM.md`, `DOGFOOD-CRABS.md`. History.
- `verification/visual/2026-08-31-*` and `verification/perf/2026-08-31-baseline/`.
  Visual and perf captures from the animation program.
- `verification/2026-09-01-usable-run/`. Feature inventory gate for this
  daily-driver pass.
- `planning/`. Scratch plans.
- `.DS_Store` junk under crates, vendor, package, verification.

Do not treat those markdown handoffs as live status.

## Known open

- Custom glyph protocol still unsupported (MRCRABS-004 remainder).
- Molt visual proof was historically INCONCLUSIVE in older notes. Code
  defines a 600 ms dissolve. Do not claim video proof from this file.
- Cursor trail was accepted in `.audit` after earlier invisible-paint
  diagnoses. HANDOFF still says the trail is invisible. That sentence is
  stale.
- Control-N tab erase and workspace icons are G-Spot, not this binary
  (`ROUTE-GSPOT-001`, `ROUTE-GSPOT-002`).
- OMP `exit 1` on a live or hub-stopped GUI is a supervisor encoding
  (`ROUTE-HOST-001`). Not a Mr Crabs source bug.
- `.audit/commit-readiness.tsv` last row is an independent review still
  pending. No product commit from that audit.
- Unmerged origin feature branches named in RESUME
  (`animated-rustfetch`, `terminal-animation-polish`,
  `chat-agent-session`) were never decided here.

Installed launchable path remains
`~/Applications/Mr Crabs.app/Contents/MacOS/mr-crabs`. Refresh with
rm-then-copy.

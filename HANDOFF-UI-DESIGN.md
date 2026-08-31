# Mr Crabs UI/design handoff

For the next UI/design session. Vault is `/Users/jamie/Obsidean/mr-crabs-vault`. Canonical checkout is `/Users/jamie/Documents/Projects/active/mr-crabs`.

This handoff did not inspect the historical animations worktree. That tree is gone from disk.

## First action (strict)

1. Read this file.
2. Inspect live Git at `/Users/jamie/Documents/Projects/active/mr-crabs` and the vault current-state / decision log.
3. Run a read-only comparison of tags `feature-terminal-animation-polish` (`9f548eb7`) and `feature-animated-rustfetch` (`6bc20c0d`) against `db58a63`.
4. Only then write UI code.

Do not copy historical files wholesale. Compare symbol by symbol. Mark shell-adjacent historical diffs as unsafe to port.

## What happened

Interactive terminal animations landed on public `main` as `db58a632b811a92fbfd6c844885ed98b99954b08` (`db58a63`) `feat: add interactive terminal animations` and were pushed to `origin/main`. Current `main` and `origin/main` are that commit.

The PTY-backed TUI can drive animation controls in session. It cannot Save persistent animation config. PTY output is not a trusted host channel. Until a trusted host channel exists, there is no Save.

Rustfetch capture and TUI scanner state are bounded. Old effects and public app APIs were kept. G-Spot still consumes a pinned Git revision on its own tree. It is not this land.

Validation before push (landing session, not re-run here): 1113 locked workspace tests passed, locked all-target check passed with one existing dead-code warning, release binary built, git consumer check passed, final code and security reviews approved.

No packaging or install refresh followed `db58a63`. `~/Applications/Mr Crabs.app` may still be the earlier 2026-08-31 package.

## What changed

On `db58a63`: streaming / typewriter / cursor-trail controls, animation TUI, animation config (no PTY Save), bounded rustfetch capture, bounded TUI scanner, preserved public APIs and old effects.

Still open from 2026-08-23 and not closed by this land: S0 oracle hash refresh, then CPU scroll/storage/ingest profiling.

## What is on GitHub

- Repo `https://github.com/JamieJ5926/mr-crabs`
- `main` / `origin/main` at `db58a632b811a92fbfd6c844885ed98b99954b08`

## What remains untracked / local

Current untracked classes, before this handoff file was added. None of these belong to `db58a63`:

- 11 `.DS_Store` files
- four `.audit/*.tsv` files
- `DOGFOOD-CRABS.md`
- `RESUME.md`

`README.md` is tracked. It documents animation commands (`+animation`, `--animation`, streaming, typewriter, cursor-trail, `all`). `RESUME.md` and `DOGFOOD-CRABS.md` are stale untracked session records, not HEAD docs.

Historical worktrees are absent, not untracked:

- `/Users/jamie/Documents/Projects/active/mr-crabs/.worktrees/animations` gone. Vault last recorded `4eac0013` on `feature/metal-effects`.
- `/Users/jamie/Documents/Projects/active/mr-crabs-metal-effects` gone.

Installed app is a local package that may predate `db58a63`.

## How to launch the installed app

Do not rebuild, package, or install unless Jamie asks.

```
open "/Users/jamie/Applications/Mr Crabs.app"
```

Treat that binary as possibly older than `db58a63`. Do not disturb a running instance. Do not use the installed app as proof of HEAD.

Source checkout for design work:

```
cd /Users/jamie/Documents/Projects/active/mr-crabs
```

## UI ownership map (exact paths)

Mr Crabs owns its existing minimal GPUI harness, terminal rendering, animation controls, and app settings. It must not expand into G-Spot product-shell composition or multipane product direction.

| Area | Path |
|---|---|
| Shell chrome | `crates/mr-crabs-app/src/ui/shell.rs` |
| Workspace layout / input | `crates/mr-crabs-app/src/ui/workspace.rs` |
| Animation TUI | `crates/mr-crabs-app/src/animation_tui.rs` |
| Animation control | `crates/mr-crabs-app/src/animation_control.rs` |
| Animation config | `crates/mr-crabs-app/src/animation_config.rs` |
| Fetch animation driver | `crates/mr-crabs-app/src/model/fetch_animation.rs` |
| ANSI rustfetch animation | `crates/mr-crabs-app/src/animated_fetch.rs` |
| Pane animation tick / region | `crates/mr-crabs-app/src/model/pane.rs` |
| Renderer / terminal element | `crates/mr-crabs-element/src/element.rs` |
| Palettes | `crates/mr-crabs-element/src/palette.rs` |
| App settings | `crates/mr-crabs-app/src/settings.rs` |
| Config crate | `crates/mr-crabs-config/src/lib.rs` |
| Package / install | `package/macos/package.sh` |

## Historical Metal / animation evidence

The historical animations worktree is absent and was not inspected.

Absent on disk:

- `.worktrees/animations` (vault: `4eac0013`, `feature/metal-effects`)
- sibling `mr-crabs-metal-effects`

Recoverable refs are evidence only. They were not inspected as a live tree in this session:

- `feature-terminal-animation-polish` → `9f548eb7`
- `feature-animated-rustfetch` → `6bc20c0d`

Vault characterization of that work (not a live checkout): renderer, cursor-trail, runtime-animation, input-regression, plus product-shell-adjacent dirt. Shell-adjacent historical changes are unsafe to port.

Recommend: read-only tag-to-`db58a63` comparison before any UI design. Never whole-file copy.

Canonical animation modules already on main include `animation_tui.rs`, `animation_control.rs`, `animation_config.rs`, `animated_fetch.rs`, `fetch_animation.rs`. Use those as the live base.

No Metal/shader source was found in the canonical inventory by the explorer lane. Generated `target` Metal artifacts are not design sources.

## Scope that stays explicit

- Mr Crabs owns PTY/VT, frame/render, input, scrollback/selection, terminal clipboard, cursor, text animation, cursor-trail, plus the existing minimal GPUI single-terminal harness, animation controls, and app settings.
- G-Spot product-shell composition and multipane product direction stay separate. G-Spot is pinned to its own Git revision.
- No daemon, detach, named sessions, persistent live PTYs, multi-client, or local multipane product direction.
- PTY cannot authorize persistent animation config writes.

## Candidate UI/design directions (not orders)

Candidates only. Next session chooses after the tag comparison.

1. Animation control presentation and preview in the existing TUI, without adding Save over PTY.
2. Settings / palette hierarchy inside terminal-owned config paths, not a G-Spot product settings shell.
3. Cursor-trail visual polish on the current renderer/effects path.
4. Terminal chrome only: `ui/shell.rs` / `ui/workspace.rs`. Stay inside the existing minimal GPUI harness. Do not grow into G-Spot product-shell composition.

## Vault updated this session

- `Context/Project Overview.md`
- `01 Plan/Decision Log.md`
- `05 Features/Done.md`
- `05 Features/Open.md`

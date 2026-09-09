## Superseded

Current state is `STATE.md`. This file is history only.

# Resume notes — 2026-08-25 session

Untracked scratch file. Delete when stale.

## State right now

- `main` @ `a1d670b`, pushed to origin. Clean tree, no stashes, no extra worktrees.
- Session commits: `f2e815e` AGENTS.md, `7fc6505` merge of `origin/pr/animation-followup`, `a1d670b` `--animation` CLI presets.
- Installed `~/Applications/Mr Crabs.app`, `dist/Mr Crabs.app`, and `./target/release/mr-crabs` all rebuilt from `a1d670b`. A persistent dev window runs under omp hub as `mr-crabs` (restart with `hub restart mr-crabs`).
- Cargo writes to `~/.cargo-target` (not `./target`). After a rebuild, refresh the stale path with a fresh inode or macOS SIGKILLs it:
  `rm target/release/mr-crabs && cp ~/.cargo-target/release/mr-crabs target/release/`

## What shipped: --animation

- `mr-crabs --animation` / `--animation list` prints the preset menu; `--animation <name>` launches with it. Presets: none, streaming (default), typewriter, cursor-trail, all. Later explicit `--text-animation`/`--cursor-trail` wins.
- Registry `ANIMATION_PRESETS` in `crates/mr-crabs-app/src/settings.rs` is the single source for menu, help, and overrides. Docs in README "Animations".
- Verified: 39 settings + 8 bin tests, effects 60/60, element 105/105, GUI mid-animation capture via cua-driver.

## Yesterday's scare, resolved

- "Fixes gone" was a stale Aug 23 binary/app bundle, not lost work. Current main supersedes `origin/fix/issue-19-text-animation-render-dedup` and `origin/fix/issue-22-typewriter-latency` (newer reimplementation via merged repair/recovery line; `issue_19_repeated_output` passes 7/7; repro clean in streaming and typewriter). Do NOT merge those two branches — they conflict against the newer code.

## Open threads for tomorrow

1. Unmerged feature branches, deliberately left out (features, not fixes). Decide keep/merge/archive:
   - `origin/feature/animated-rustfetch` (animated startup logo)
   - `origin/feature/terminal-animation-polish`
   - `origin/feature/chat-agent-session`
2. Cursor-trail glow was config-verified but never caught on a screenshot (250 ms fade). If visual proof matters, record with `cua-driver recording start`.
3. `+show-config` reviewer note: presets read through the user config overlay unless `--default` is passed; a user config setting text-animation/cursor-trail changes what `+show-config --animation <x>` echoes. Cosmetic, not a bug.
4. Stale fix/pr branches on origin (issue-19, issue-22, pr/animation-followup now merged) could be archived like the other `archive/2026-08-23/*` branches.
5. g-spot consumes pinned revs; if it should pick up `--animation`, bump its pins and run `sh verification/tools/git_consumer_check.sh`.

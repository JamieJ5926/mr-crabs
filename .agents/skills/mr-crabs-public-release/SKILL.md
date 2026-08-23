---
name: mr-crabs-public-release
description: Publish Mr Crabs Rust to the clean public GitHub repo without the Ghostty 17k-commit history
---

# Mr Crabs public release (clean history)

Use when publishing or syncing the standalone Rust terminal to GitHub.

## Repos (do not mix)

| Tree | Path | Remote | History |
|---|---|---|---|
| Public product | `/Users/jamie/Documents/Projects/active/mr-crabs-terminal` | `https://github.com/JamieJ5926/mr-crabs.git` | Clean (`main`, started `e3cb442`) |
| Dev worktree | `/Users/jamie/Documents/Projects/active/mr-crabs-rust` | still linked to the old Ghostty-fork remotes | ~17k Ghostty commits |

Never `git push` from `mr-crabs-rust` to `JamieJ5926/mr-crabs`. Export files, then commit in `mr-crabs-terminal`.

`JamieJ5926/mr-crabs-terminal` was renamed to `JamieJ5926/mr-crabs`. Old name redirects.

## Export recipe

From the rust worktree (after harden commits land):

```bash
rsync -a \
  --exclude .git --exclude target --exclude dist --exclude .omp \
  --exclude AGENTS.md --exclude CLAUDE.md \
  /Users/jamie/Documents/Projects/active/mr-crabs-rust/ \
  /Users/jamie/Documents/Projects/active/mr-crabs-terminal/
```

Then in `mr-crabs-terminal`: review `git status`, commit, `git push origin main`.

## Required public files

- `LICENSE` — MIT © JamieJ5926 for original code; point at `resources/THIRD_PARTY_NOTICES.txt`
- `README.md` — honest alpha: macOS, zsh PTY, type + Return; not a Ghostty fork
- Do not claim DMG/notarization unless those artifacts exist

## Product sequence (user 2026-08-18)

Standalone harden first (scrollback measure/fix, quit, config, then public sync). Animations stay in `mr-crabs-effects` until a later spec. G-Spot integration is not a one-seam swap (different GPUI pin, GPL host, `TerminalFrame` vs `FrameDelta`).

---
name: mr-crabs-tui-animation-flicker
description: "Diagnose and fix Mr Crabs live streaming-text flicker or TUI wrap (OMP status bars, leftover letters) without dropping Metal or rewriting paint."
---

# Mr Crabs TUI / streaming-text flicker

Repo: `/Users/jamie/Documents/Projects/active/mr-crabs-rust`.

## When to use

OMP or another alt-screen TUI starts flickering, hides random letters (`orking`, blotchy `intent`), or wraps a status bar so a second cwd/prompt appears.

## Two distinct bugs

1. **Concealment flicker.** `EffectsModel` restamps every changed cell. `paint_effects` overlays background over revealing/pending cells. On alt-screen TUIs or dumps this hides live glyphs for the 120ms window.
2. **Wrong cell width.** `compact/width.rs` must use `unicode-width` (`UnicodeWidthChar::width`). A hand East-Asian table misses emoji (e.g. U+1F7E3 = 2). OMP then wraps the status bar.

Do not treat leftover `mr-crabs-rust` as an erase-line bug until widths match.

## Required paint rules

In `crates/mr-crabs-element/src/element.rs` `paint_effects`:

- Skip text concealment when `frame.viewport.alternate_screen` is true **or** `revealing.len() + pending.len() > 16`.
- Still draw cursor trail.
- Request animation frames only while blink, trail, or a *non-burst* primary-screen reveal is active.

Default product settings are streaming 120ms + trail on. `{"text_animation":"none"}` remains the off switch.

## Width contract

`crates/mr-crabs-terminal/src/compact/width.rs`:

- C0 / DEL / C1 → `None` (no cell).
- Everything else → `unicode-width` 0.2.2.
- Pin `unicode-width = "=0.2.2"` in `mr-crabs-terminal/Cargo.toml`.

Test: `cargo test -p mr-crabs-terminal --lib ascii_and_emoji`.

## Do not

- Drop Zig Metal GLSL into GPUI.
- Weaken the scrollback gate.
- Push the 17k Ghostty graph from `mr-crabs-rust`.
- Launch extra GUI windows unless Jamie asks.

## Known commits (verify HEAD)

- `5e7cb7400` burst conceal skip
- `818feefee` unicode-width + alt-screen skip

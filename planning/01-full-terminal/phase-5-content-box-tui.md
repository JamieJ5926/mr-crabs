# Phase 5. Content-box TUI

Back to [overview](overview.md).

## Goal

The user can restyle the terminal content box from inside the window. No G-Spot settings shell.

## Changes

Extend `crates/mr-crabs-app/src/animation_tui.rs` and the matching palette actions. Surface the controls that already exist as flags: theme, font size, cell height, padding, cursor shape from phase 4, text animation.

JSON and CLI remain the persistent store. PTY still cannot Save. Same rule as animation config.

Do not add a preferences window.

## Data structures

Reuse `AppSettings`. The TUI writes the same overlay the CLI writes.

## Verification

Static. Existing animation TUI tests plus one for theme and cursor-shape rows.

Runtime. Open `+animation`. Change theme paper to harbor. Snapshot. Background mean moves. Change cursor shape. Snapshot. Caret geometry moves. Input after close must hit the shell, not the TUI.

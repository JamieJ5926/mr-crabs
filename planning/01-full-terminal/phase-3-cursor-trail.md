# Phase 3. Cursor trail paint

Back to [overview](overview.md).

## Goal

A vacated cell leaves a decaying leftover the user can see. This is the one Metalterm-class motion we keep.

## Changes

`crates/mr-crabs-element/src/element.rs` `paint_trail`. Diagnosis already names this drawer. The model activates. Focus wiring is fine. Do not reopen `mr-crabs-effects` unless the paint-boundary probe shows `trail_active` false.

Likely fault is colour, alpha, geometry, or the content mask clipping a current-cell drop shadow. Draw a leftover of the vacated rect, not a glow of the live cell.

Do not add a second trail style.

## Data structures

Keep `TrailFrame` as it is. `active`, `alpha`, `leftover_rect`, `segment`.

## Verification

Static. `cargo test -p mr-crabs-effects --offline --lib -- trail` and the existing Reduce Motion config test.

Runtime. Phase 1 helper. Cursor jump with `--cursor-trail-opacity=1 --cursor-trail-duration=2000ms`. Fixed vacated-cell box must start hot and decay toward neighbor luma. A still of the live caret is failure. This is the Jamie go-gate for the trail bug. Do not start this phase until Jamie says go.

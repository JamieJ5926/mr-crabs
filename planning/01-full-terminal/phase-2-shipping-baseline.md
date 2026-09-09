# Phase 2. Shipping baseline

Back to [overview](overview.md).

## Goal

A dated capture set of what the dock app already shows, so later phases do not argue with memory.

## Changes

No crate edits. Using the phase 1 helper, capture one dedicated window each for:

- `--theme paper`
- `--theme harbor`
- `--startup-animation molt`
- `--startup-animation rustfetch`
- `--animation typewriter`
- `--animation streaming`

Write results under `verification/visual/YYYY-MM-DD-shipping-baseline/`. Include pid, window id, binary SHA, and a one-line verdict per scenario.

Molt needs a burst across about 900 ms, not a still. A still of an idle window is not a molt record.

## Data structures

A small JSON sidecar per scenario. `pid`, `window_id`, `argv`, `sha256`, `frame_count`.

## Verification

Static. None.

Runtime. Each scenario directory has more than one PNG. Harbor and paper background means differ. Typewriter frames keep changing the first line after the payload. Streaming paints the line then fades. Molt frames show a shrinking hole or the run is marked inconclusive, not green.

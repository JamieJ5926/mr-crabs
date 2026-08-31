# Lane L search highlight quads

Status: PASS.

Resolved model: cli-proxy/dddai.grok-4.6. Fallback: none.

Playbook: Feature, executed as implementer. No child spawn.

Principles: Laziness Protocol (reuse `selection_rects`, no new file), Prove It Works (named tests plus `cargo check`), find-not-invent ADOPT-EXISTING.

## What changed

`FrameDelta.search_matches` is painted in `mr-crabs-element` only.

Paint sits under the existing selection overlay in `element.rs`. Regular matches use `search_match_color`. The current match uses `search_current_color`. Selection still paints on top with `selection_color`, so the current-match selection overlay and cursor are not replaced by search fill.

Geometry is `search_match_rects` in `selection.rs`. It feeds a half-open `FrameRange` into `selection_rects`, so clipping, multi-row per-row rects, reversed anchors, and past-the-edge column advance stay identical to selection.

Production search field and as-you-type remain out of scope. Search engine, regex, pane query, command palette, and app actions were not edited.

## Find-not-invent

Class: code reuse.

Stage 1 (repo): `selection_rects` already owns half-open row-major cell spans (`crates/mr-crabs-element/src/selection.rs`). Palette already owns overlay HSLA (`palette.rs`). `cell_bounds` / `run_bounds` are origin-relative cell math, not span clippers.

Verdict: ADOPT-EXISTING. Extend `selection.rs` with a thin wrapper. No new public helper file. Colors live next to `selection_color`.

## Blast radius

What it does. Paint extra translucent quads when `search_matches` is nonempty. Empty vec is a no-op.

The one fact it is safe because of. Search fill uses the same clip path as selection, and it never writes into app or history crates. Step 4: `search_match_single_row`, `search_match_multi_row`, `search_match_clips_and_empty` call the real helper. `defaults_are_opaque_and_deterministic` asserts search colors differ from selection and from each other.

Risks. Overlapping current-match search fill plus selection fill can stack alpha on the current hit. Cost is visual, not data. Check: paint order search then selection.

Cleared. No Metal. No `pane.rs` / `search.rs` / `action.rs` edits. `cargo check -p mr-crabs-element --offline` finished.

## Tests

```
CARGO_TARGET_DIR=~/.cargo-target cargo test -p mr-crabs-element --offline --lib -- search_match
  3 passed, 104 filtered

CARGO_TARGET_DIR=~/.cargo-target cargo test -p mr-crabs-element --offline --lib -- search_match defaults_are_opaque
  4 passed, 103 filtered

CARGO_TARGET_DIR=~/.cargo-target cargo check -p mr-crabs-element --offline
  Finished. Pre-existing dead_code on PreparedEffects.focused/now_ms.
```

No full workspace suite. No git mutation.

## Files

- `crates/mr-crabs-element/src/element.rs`
- `crates/mr-crabs-element/src/selection.rs`
- `crates/mr-crabs-element/src/palette.rs`
- `crates/mr-crabs-element/src/lib.rs` (re-export `search_match_rects`)
- `verification/visual/2026-08-31-search-highlight/RESULT.md`

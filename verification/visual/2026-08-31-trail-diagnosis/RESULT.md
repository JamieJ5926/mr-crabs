# Lane A. Cursor trail diagnosis

Status. PASS. Named cause plus the one paint-boundary probe that still cannot run.

Output directory. `verification/visual/2026-08-31-trail-diagnosis/` (untracked). No writes under `crates/`. No git mutation.

Principles that shaped this. Investigation playbook plus how (Explain, simple, no child spawn because this seat cannot spawn). Fix-root-causes. Unslop. Prove-it-works via existing `cargo test` filters, not a new crate test.

Throughput checkpoint. n/a, read-only investigation.

## Overview

The trail state machine activates on a scripted cursor jump. Existing tests prove that without painting. The 400-frame dogfood capture (`DOGFOOD-CRABS.md:49-54`) therefore cannot be "the model never turns on." The remaining fault lives in `paint_trail` (colour, alpha, geometry, mask, or GPUI drop-shadow behavior). Headless `AppModel::pump` never reaches that paint path, so `trail_active` at the paint boundary is not readable today.

## Split

Paint half, not model half.

Evidence.

1. `CursorTrail::frame` sets `active`, `alpha`, `glow_rect`, and `segment` when enabled, focused, visible, and inside the duration (`crates/mr-crabs-effects/src/trail.rs:307-338`). Tests `first_frame_glows_without_segment` and `move_captures_previous_rect_and_resets_fade` passed.
2. `EffectsModel::apply_frame` always feeds `cursor_rect(&frame.cursor, self.cell)` into that machine (`model.rs:319-325`). `trail_fades_and_resumes_with_focus` passed. After `cursor.col = 5` at `now_ms=2016`, `f.trail.glow_rect.x == 50.0`.
3. S9 corpus case `cursor-trail-fade` expects `active: true`, `alpha: 0.35`, segment from `(5,10)` to `(55,10)` on that jump. `s9_effects_corpus_matches_fixtures` passed.
4. Config default is on (`DEFAULT_CURSOR_TRAIL = true`). Dogfood used `--animation cursor-trail --cursor-trail-opacity=1 --cursor-trail-duration=2000ms` with `+show-config --default` confirming `cursor-trail = true`.

If the model were dead, those tests would fail. They did not.

## Root cause (paint)

`paint_trail` (`element.rs:960-986`) is the only drawer.

1. Glow is `window.paint_drop_shadows` of the **current** cursor rectangle, zero offset, blur `radius_px`. The vacated cell is not filled as a leftover quad. A drop-shadow of the live cell can sit under the opaque cursor quad painted immediately after (`paint_cursor` at `element.rs:784-785`). Capture then sees no leftover at the old cell (luma ~13 vs expected ~220).
2. The previous-to-current leftover is a filled path of `trail_segment_quad`. That path is a thin stroke (`radius_px * 0.5`, min 1 px), not a decaying copy of the old cell. A 60-char type-out would leave a 1-to-5 px band, not a full-bright cell. The dogfood analysis looked for full-cell leftovers.
3. Both draws run inside `window.with_content_mask` of `content_bounds` (`element.rs:776-787`). Glow that extends past the cell is clipped to the cell box. Combined with (1), the visible remainder can be zero even when `trail.active` is true.
4. `GradientId` is computed and never consumed by paint. Prefs forbid a second Metal pass. The oracle glow (`exp(-d/radius)` around the rect plus the segment) is not reproduced.

This is a code-level diagnosis of the paint path. It is not a GPU screenshot. Visual confirmation is owned by P1.

## Feasibility of the paint-boundary probe

`install_diagnostic_trace` / `set_diagnostic_trace` (`app_model.rs:761-776`) are tests-only. Existing tests around `app_model.rs:2420-2560` record Pump and Frame from `model.pump`. They never record Paint.

`DiagnosticPaintEvent` is emitted only from `TerminalElement::paint_frame` via `with_paint_diagnostics` (`element.rs:790-796`, wired at `workspace.rs:515`). Paint runs only inside a GPUI window. Headless `AppModel` has no window, so the paint boundary is not reachable.

Smallest product change that would make it reachable (not implemented, Jamie's call).

- Debug-only install of the existing sink behind `MR_CRABS_TRACE_PAINT=1` or `cfg(debug_assertions)`, writing `trail_active` / `trail_alpha` to stderr or the trace. Cost is one env/cfg branch in `workspace.rs` plus a documented headless-vs-window distinction. Still needs a live window (or a GPUI test harness) because the sink lives in `paint`.
- Or an in-crate test in `crates/mr-crabs-element/src/element.rs` that calls `paint_trail` / `prepare_effects` with a stub `Window`. Cost is a GPUI test window fixture this crate does not currently have for effects.

Neither is applied here.

## Named probe that would decide remaining paint sub-causes

Once P1 can capture, or once a later unit lands a paint-sink env flag.

1. At the first paint after a 5-column jump with trail on, duration 2000 ms, opacity 1.0, read `trail_active` and `trail_alpha` from `DiagnosticPaintEvent`. If both are true and alpha ~1.0, the model reached paint and the fault is GPUI draw (drop-shadow clip / path too thin). If false, wiring from `PreparedEffects` into that paint call is broken despite `apply_frame` consuming `focused`/`now_ms` at `element.rs:869` (already refuted as dead storage).
2. Independently, a later unit can land the seam below to lock the jump-frame `active` assertion that `trail_fades_and_resumes_with_focus` currently skips (it only asserts `glow_rect.x`).

## Seam (not applied)

File. `crates/mr-crabs-effects/src/model.rs`, module `tests`, function `trail_fades_and_resumes_with_focus`, immediately after the `cursor.col = 5` `apply_frame`.

Assert.

```rust
assert!(f.trail.active);
assert_eq!(f.trail.alpha, 0.35);
assert!(f.trail.segment.is_some());
```

Quoted patch (do not apply).

```
--- a/crates/mr-crabs-effects/src/model.rs
+++ b/crates/mr-crabs-effects/src/model.rs
@@ -612,6 +612,9 @@
         cursor.col = 5;
         let f = m.apply_frame(&frame_at(size, 2, Vec::new(), cursor), 2016, true);

+        assert!(f.trail.active);
+        assert_eq!(f.trail.alpha, 0.35);
+        assert!(f.trail.segment.is_some());
         assert_eq!(f.trail.glow_rect.x, 50.0);
```

S9 already asserts the same numbers. This seam only tightens the unit test. It does not decide paint.

## Tests actually run

Filter `CARGO_TARGET_DIR=~/.cargo-target cargo test -p mr-crabs-effects -- --nocapture trail`

9 passed, 57 filtered. Names: `block_cursor_rect_is_the_full_cell`, `bar_and_underline_rects_follow_shape_rules`, `degenerate_rect_draws_nothing`, `first_frame_glows_without_segment`, `hidden_or_unfocused_or_disabled_draws_nothing`, `fade_is_linear_and_expires`, `gradient_cache_is_bounded_and_evicts_lru`, `move_captures_previous_rect_and_resets_fade`, `trail_fades_and_resumes_with_focus`.

Filter `CARGO_TARGET_DIR=~/.cargo-target cargo test -p mr-crabs-effects -- --nocapture`

66 passed. Includes `s9_effects_corpus_matches_fixtures` and `defaults_match_mr_crabs` (`cursor_trail` true).

Filter `CARGO_TARGET_DIR=~/.cargo-target cargo test -p mr-crabs-element trail -- --nocapture`

3 passed, 99 filtered. `trail_glow_translates_by_origin`, `trail_glow_reports_degenerate_as_none`, `trail_segment_quad_handles_horizontal_vertical_diagonal_and_zero_length`. Geometry helpers only. They do not call `paint_drop_shadows`.

## How it works (paint chain)

`workspace` builds `TerminalElement` with `EffectsConfig::from(animation)` and a real focus handle (`workspace.rs:496-508`). `prepare_effects` skips only when text is Disabled **and** trail is off (`element.rs:848-851`). Clock is `Instant` elapsed ms. `apply_frame` gets `focus.is_focused(window)`. If `trail.active && trail.alpha > 0.0`, `paint_trail` runs under the content mask, then the cursor quad.

## Gotchas

- Focus wiring and `PreparedEffects` dead-code were not re-run (prefs line 12).
- First frame after enable glows with `segment = None`. A leftover only exists after a rect change.
- Paint diagnostics mapping tests (`paint_diagnostics.rs:90-114`) construct a fake outcome. They do not paint.
- `GradientCache` is unused by GPUI paint.

## Deviations

Investigation how-skill step 2b asks for a child explainer. This seat's brief forbids spawning children except when the official playbook of a Pstack role requires it, and this coordinator-assigned poteto-agent may not spawn. Explained inline.

No GUI capture (forbidden). No crate edits.

## Follow-ups (out of scope)

- Lane B. Replace drop-shadow-of-current-cell with a leftover that survives the live cursor (or drop the content mask for the glow). Consume or delete `GradientId`.
- Env/cfg paint-trace for a live window, Jamie's product call.
- P1 capture once the paint change lands.
- Optional unit-test seam above.

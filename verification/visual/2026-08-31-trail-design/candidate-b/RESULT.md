# Candidate B. Cursor trail as leftover geometry

routing: cli-proxy/dddai.grok-4.6, fallback: none (workstation model)

Status. PASS. Design only. No source edits. No git mutation. No GUI.

Candidate A is the GPUI-paint-only sibling. This candidate prefers a model-shape change because the current `TrailFrame` names the wrong rectangle. Paint cannot invent a vacated cell if the frame only ships `glow_rect = current`.

Principles. Investigation playbook. How (Explain, simple, no child spawn). Model the domain. Boundary discipline. Unslop. Redesign from first principles for leftover vs live cursor. Subtract before add for `GradientCache`.

Throughput checkpoint: n/a, read-only investigation.

## Overview

Lane A proved the state machine turns on. Root cause is geometry, not wiring. `CursorTrail::frame` at `crates/mr-crabs-effects/src/trail.rs:333` sets `frame.glow_rect = current`. `paint_trail` at `crates/mr-crabs-element/src/element.rs:960-973` drop-shadows that rect with zero offset. `paint_cursor` then fills the same cell opaque (`element.rs:784-785`). Capture sees no leftover. The thin `trail_segment_quad` is a 1-to-5 px band, not a decaying copy of the old cell.

The domain is a leftover that decays after the caret leaves a cell, plus one live caret. Encode that in `TrailFrame`. Do not keep a glow field that means "current cursor." GPUI paint stays a thin consumer of leftover bounds. No app-owned Metal. Prefs line 2 and `verification/research/gpu-terminal-architecture.md:8-46` already closed a second renderer.

## Key concepts

**Live caret.** Opaque cursor quad from `paint_cursor`. Not a trail primitive.

**Leftover.** The previous cursor rectangle while the fade window is open. This is the only glow paint target.

**`TrailFrame` after the reshape.** `active`, `elapsed_ms`, `alpha`, `radius_px` stay. Replace `glow_rect` with `leftover_rect: Option<RectPx>`. Keep `segment` as optional connective tissue between leftover center and current center. Drop `gradient` from the product contract now (field may linger until H removal).

**`AccessibilityPolicy`.** App-owned snapshot. `allows_motion()` is the only motion gate B consumes. Detection stays in `crates/mr-crabs-app/src/accessibility_policy.rs`. Effects and element never call NSWorkspace.

**Dead descriptors.** `GradientCache`, `GradientId`, `MAX_GRADIENTS`, `TrailFrame.gradient`, `EffectsModel::change_texture*`. H catalogued them in `verification/visual/2026-08-31-effects-hygiene/RESULT.md`. Paint never reads them. Leave them until H's later delete unit.

## How it works

### Domain reshape

`CursorTrail` already stores `previous` and `current` (`trail.rs:257-258`). `frame()` already captures previous on rect change (`trail.rs:308-312`). The bug is the published frame, not the internal history.

New `frame()` contract:

1. Track rect changes as today. Clock and fade math stay linear `(1 - elapsed / duration) * opacity`.
2. If disabled, unfocused, hidden, degenerate current, or elapsed past duration, return inactive with `leftover_rect = None`.
3. First paint after enable, no prior rect. `active` may be false for leftover paint. The live caret still draws. There is no vacated cell. Today's `first_frame_glows_without_segment` is the wrong product. It glows under the caret.
4. After a move, `leftover_rect = previous`. `segment` joins leftover center to current center. `radius_px` is `0.5 * max(w, h)` of the leftover, not the live caret.
5. Fade belongs to the leftover. Alpha decays on the vacated cell. The live caret never inherits trail alpha.

This makes "glow under the current cursor" unrepresentable. Paint cannot drop-shadow current unless it ignores the type.

Optional bounded history. If a 60-char type-out should leave more than one fading cell, replace `previous: Option<RectPx>` with a small ring of vacated rects, each with its own `change_ms`. Cap at 1 for the first land. Prefs want one recognizable cursor signature, not seven copies. One leftover plus one caret is the signature. Extra ghosts wait for a later product call.

### Paint consumer

`paint_trail` paints only `leftover_rect`. `trail_glow_bounds` takes that option. Drop-shadow stays GPUI `paint_drop_shadows` with blur `radius_px`, still zero offset, now on the vacated cell so the opaque caret does not cover it.

Keep `trail_segment_quad` as a faint connector at leftover alpha. It is not the leftover. If the connector fights the "one signature" rule, omit it in the first land. Candidate B prefers omit on first land. The leftover cell is the signature. The 1 px band was the dogfood miss.

Content mask. Lane A noted glow clipped to `content_bounds` (`element.rs:776-787`). Leftover is inside the grid if the old cell was. Blur may still clip at the viewport edge. That is acceptable. Do not drop the mask. Do not inflate leftover bounds past the cell plus blur.

Layer order stays trail then cursor. Trail no longer shares the current cell, so order is less load-bearing, but keep it.

### Reduce Motion

E owns detection. B consumes.

Wire at the app boundary, not in `CursorTrail::frame` and not via a second OS read.

1. `workspace.rs` already builds `EffectsConfig::from(animation)` (`workspace.rs:508`).
2. Later unit. Workspace (or `AppModel` when it already holds a policy snapshot) receives `AccessibilityPolicy` from E's existing detect path. If `!policy.allows_motion()`, force `EffectsConfig.cursor_trail = false` for that paint, or pass a cloned config with trail off. User config can stay true. Runtime effective config is off.
3. `prepare_effects` already tears down the model when text is Disabled and trail is off (`element.rs:848-851`). Reduce Motion therefore reuses the disabled fast path. No new motion style.
4. Tests in element or app pass `AccessibilityPolicy::from_flags(true, false)` and assert `cursor_trail` effective false. Never call `detect()` from those tests.

Do not put `AccessibilityPolicy` inside `mr-crabs-effects`. That crate stays host-free. Policy is an app boundary type. Mapping policy to `TrailConfig.enabled` is the adapter.

Reduce Transparency is C's gate (`allows_transparency`). B does not consume it. Trail leftover is an opaque-ish cursor-colored shadow, not a theme translucency.

### Dead descriptors until H removal

Ignore in the B land:

- `GradientCache::get` at `trail.rs:337`. Still runs. Still unused by paint.
- `TrailFrame.gradient`.
- `EffectsModel::change_texture*` (reveal packing, not trail paint).

B's reshape may stop writing `frame.gradient` only if that is in the same `trail.rs` function B already edits. Prefer leave the assignment so H's delete unit stays one patch. If the field remains, tests that `assert_eq!(f.gradient, GradientId(0))` keep passing.

Do not invent a GPUI gradient atlas keyed by `GradientId`. That would recreate the shader-resource contract without Metal, and H already marked it dead.

## Rejected alternatives

**Paint-only remap of `glow_rect`.** Candidate A's lane. Draw previous by subtracting `segment` or by reading `CursorTrail` internals from the element. The element must not reach into `CursorTrail`. The published frame would still lie. Illegal state stays representable.

**App-owned Metal / resurrect `cursor-trail.glsl`.** Prefs forbid it. Two critics already rejected a second renderer. `GradientCache` exists for that path. Leave it for H to delete.

**Seven caret copies / particle trail.** Prefs line 8. One leftover plus one live caret.

**Keep first-frame glow under current.** That is the bug. Idle caret must not halo.

**Headless paint-sink as the fix.** Lane A named an env probe. Useful later. It does not move leftover geometry.

**Element reads Reduce Motion from NSWorkspace.** Violates prefs line 7 and E's API.

## Where things live

| Piece | Owner later | Path |
|---|---|---|
| Leftover frame | B (`CursorTrail::frame`) | `crates/mr-crabs-effects/src/trail.rs` |
| Model test jump assert | B | `crates/mr-crabs-effects/src/model.rs` `trail_fades_and_resumes_with_focus` |
| S9 glow fixture | B | `crates/mr-crabs-effects/tests/s9_corpus.rs` plus corpus JSON if glow means leftover |
| `paint_trail` | B, function split | `crates/mr-crabs-element/src/element.rs` `paint_trail`, `trail_glow_bounds` |
| Policy to config | B consumer, E API | `crates/mr-crabs-app/src/ui/workspace.rs` (or whoever already threads `EffectsConfig`) |
| Gradient delete | H, not B | hygiene RESULT |

This design batch writes only this file. Implementation is a later single-writer unit.

## Patch seam (later implementer, not this batch)

Files in one land, in order.

1. `crates/mr-crabs-effects/src/trail.rs`
   - Rename or replace `TrailFrame.glow_rect` with `leftover_rect: Option<RectPx>`.
   - `frame()` sets leftover from `self.previous` after a move. None on first rect.
   - `radius_px` from leftover when present, else 0.
   - Leave `gradient` assignment for H.
   - Rewrite tests `first_frame_glows_without_segment` to `first_frame_has_no_leftover`. `move_captures_previous_rect_and_resets_fade` asserts leftover equals the old rect, not current.
2. `crates/mr-crabs-effects/src/model.rs` test `trail_fades_and_resumes_with_focus`. After `cursor.col = 5` at 2016, assert `f.trail.active`, leftover x is 0 (old cell), not 50. Lane A's proposed `glow_rect.x == 50` is the old lie. Replace it.
3. S9 `check_trail` and `cursor-trail-fade` fixture. Glow array becomes leftover. Current caret is not in the trail payload.
4. `crates/mr-crabs-element/src/element.rs`
   - `trail_glow_bounds` takes leftover, still rejects degenerate.
   - `paint_trail` returns immediately if leftover is None.
   - Drop-shadow leftover only. Do not paint current.
   - Skip `trail_segment_quad` on first land unless a follow-up wants the connector.
   - Tests `trail_glow_translates_by_origin` keep translating leftover.
5. Workspace. Map `!AccessibilityPolicy.allows_motion()` onto effective `cursor_trail = false`. Request no new config key from C.
6. Do not edit `accessibility_policy.rs` (E). Do not edit `palette.rs` (K). Do not add shaders.

Quoted intent for leftover assignment (do not apply now):

```
frame.leftover_rect = self.previous.filter(|r| !r.degenerate());
frame.radius_px = frame.leftover_rect.map(|r| 0.5 * r.w.max(r.h)).unwrap_or(0.0);
frame.segment = match (self.previous, self.current) {
    (Some(prev), Some(cur)) => Some(LinePx::new(prev.center(), cur.center())),
    _ => None,
};
```

`active` is true only when leftover is Some, fade window open, enabled, focused, visible.

## Tests and capture proof for implementation

Headless, this land.

- `CARGO_TARGET_DIR=~/.cargo-target cargo test -p mr-crabs-effects -- trail`
- Include renamed first-frame and move tests.
- `CARGO_TARGET_DIR=~/.cargo-target cargo test -p mr-crabs-effects -- s9_effects_corpus_matches_fixtures`
- `CARGO_TARGET_DIR=~/.cargo-target cargo test -p mr-crabs-element -- trail`
- App test that `from_flags(true, false)` yields no trail prepare. Exact file is whoever threads config after E. Do not detect in the test.

Visual, after paint lands, P1 only. This design seat does not capture.

Script a 5-column jump, trail on, duration 2000 ms, opacity 1.0, Reduce Motion off. Burst with `screencapture -x -o -S -l <CGWindowID>` (Gate 3). Idle stills are not evidence.

Proof table the implementer must fill from frames, not from this file.

| Frame | Expect |
|---|---|
| Before jump | Live caret only. No halo on the caret cell. |
| First frames after jump | Vacated cell brighter than idle bg, decaying. Live caret opaque at new col. |
| After duration | Vacated cell back to bg. One caret. |
| Reduce Motion on | Same as trail off. Instant caret, no leftover. |

Dogfood 400-frame leftover hunt looked for full-cell luma ~220 at the old cell. That remains the pass bar.

## Gotchas

- Focus wiring and `PreparedEffects` storage are settled (prefs 12). Do not re-diagnose.
- `apply_frame` already consumes focused at `element.rs:869`.
- First leftover exists only after a rect change. Typing one character is the smallest visual unit.
- Content mask still clips blur at the pane edge.
- H removal of `GradientCache` must not block this land.
- Candidate A may keep `glow_rect` and remap in paint. If both land, pick one. This candidate says the name `glow_rect` should die.

## Deviations

How skill step 2b wants a child explainer. This seat cannot spawn. Explained inline.

No build. Brief says none required.

Wrote only `verification/visual/2026-08-31-trail-design/candidate-b/RESULT.md`.

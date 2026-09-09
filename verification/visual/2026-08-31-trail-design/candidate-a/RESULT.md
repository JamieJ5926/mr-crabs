# Candidate A. Smallest GPUI paint-only trail rewrite

routing: cli-proxy/dddai.grok-4.6, fallback not used (workstation model line)

Status: PASS. Design only. No `crates/` edits. No git mutation. No GUI.

Lane: B Candidate A. Output is this file only.

Principles that changed a choice. Laziness protocol kept the model and `TrailFrame` fields as they are, and moved the leftover onto paint geometry already implied by `segment`. Model the domain treated the leftover as one vacated cursor rectangle reconstructed from `segment.from` plus `glow_rect` size, not a new particle list. Unslop on this report.

## Design

The model already works. Lane A proved `CursorTrail::frame` sets `active`, `alpha`, `glow_rect = current`, and `segment` from previous-to-current centers (`trail.rs:308-337`). `paint_trail` then glows the **current** cell (`element.rs:960-986` via `trail_glow_bounds(trail.glow_rect)`), then `paint_cursor` covers that same cell (`element.rs:784-785`). The vacated cell never gets a decaying copy. The segment fill is a thin stroke (`trail_stroke_width` = `radius_px * 0.5`, min 1 px), not a cell.

Candidate A does not change `TrailFrame`, fade math, duration, or cursor shape rules. It changes what `paint_trail` draws.

**Leftover, not live glow.** When `trail.segment` is `Some(line)`, reconstruct the vacated rectangle as axis-aligned bounds of size `glow_rect.w` x `glow_rect.h` whose **center** is `line.from`. That is the previous cursor rect the model already stored, recovered without a new field. Paint that leftover with GPUI fill plus a small drop-shadow, alpha = `trail.alpha`, color = cursor palette color. Do not paint a glow on `glow_rect` (the live cell). Keep the existing segment path as a connector only if it does not read as a second caret. Prefer skipping the segment on adjacent-cell moves so one leftover cell is the signature.

**First frame after enable.** `segment` is `None`. Paint nothing for trail. Idle cursor is the opaque cursor only. That matches Lane A: leftover exists only after a rect change.

**Mask.** Trail currently runs inside `window.with_content_mask(content_bounds)` (`element.rs:776-783`). Keep that mask. The leftover is a full cell inside the grid, so it does not need to bleed past `content_bounds`. Do not drop the mask. Do not add a second Metal pass.

**One signature.** One leftover cell in the cursor color, fading over `duration_ms`. Same for Block, Bar, and Underline because leftover size follows `glow_rect` (shape-aware via existing `cursor_rect`). No extra caret styles.

## Reduce Motion

E owns detection. `AccessibilityPolicy` (`crates/mr-crabs-app/src/accessibility_policy.rs`) exposes `allows_motion()`. Paint and effects never call NSWorkspace.

Wire at the app boundary that already builds `EffectsConfig`:

- `workspace.rs` around `with_effects(EffectsConfig::from(animation))` (~508).
- Snapshot `AccessibilityPolicy` on the model or window (E's surface). Pass it in. If `!policy.allows_motion()`, force `cursor_trail = false` on the `EffectsConfig` handed to the element (or skip `paint_trail` after that config is already false).

Do not read `AccessibilityPolicy::detect()` from `element.rs` or `trail.rs`. Tests inject `from_flags(true, _)` and assert `paint_trail` is not called / `TrailFrame.active` stays false because config disabled the machine.

Reduce Transparency is Lane C. Ignore it here.

## Dead descriptors until H removal

Ignore and do not consume:

- `GradientId`, `GradientCache`, `MAX_GRADIENTS`, `TrailFrame.gradient` (H: product paint never reads them; `paint_trail` uses `active`, `alpha`, `glow_rect`, `radius_px`, `segment`).
- `EffectsModel::change_texture*` and `ChangeTracker` packed RGBA. Not on the trail paint path.

Candidate A still does not delete them. H's removal plan runs after this paint unit. Keep writing `frame.gradient` so s9 and trail unit tests stay green.

## Rejected alternatives

1. **Glow the current cell harder.** That is the bug. Opaque cursor covers it.
2. **Widen the segment stroke to cell size.** Diagonal jumps become a smear, not a vacated cell. Adjacent hops look like two carets.
3. **Add `TrailFrame.previous_rect`.** Cleaner model, Candidate B's job. Not needed to reconstruct leftover from `segment.from` + `glow_rect` size for same-shape moves. Shape change mid-fade (Bar to Block) is slightly wrong with this reconstruction. Accept that for the smallest paint patch. If B lands a previous-rect field later, paint can switch to it in one helper.
4. **App-owned Metal / `GradientId` texture.** Prefs line 2. Two critics already rejected it.
5. **Seven caret copies / particle chain.** Prefs line 8. One leftover.
6. **Disable content mask for glow.** Unnecessary if leftover is inside the cell grid.

## Patch seam (later single writer of `element.rs`)

Files the implementer may touch. This candidate does not.

| File | Function | Change |
|---|---|---|
| `crates/mr-crabs-element/src/element.rs` | `paint_trail` | Leftover quad from `segment` + skip current-cell glow. |
| `crates/mr-crabs-element/src/element.rs` | `trail_glow_bounds` | Keep. Reuse for leftover rect after reconstructing `RectPx` from `segment.from`. Optionally add `trail_leftover_rect(segment, glow_rect) -> Option<RectPx>` next to it. |
| `crates/mr-crabs-element/src/element.rs` | `prepare_effects` | No trail math change. Optional: if config already has trail off from Reduce Motion, existing skip at 849-851 is enough. |
| `crates/mr-crabs-app/src/ui/workspace.rs` | pane `TerminalElement` builder | Consume `AccessibilityPolicy.allows_motion()` when building `EffectsConfig`. Never detect here. |
| `crates/mr-crabs-effects/src/trail.rs` | none for Candidate A | Leave `glow_rect = current`. Leave gradient write. |

Do not edit `element.rs` in this batch. Ownership stays the later B implementer for `paint_trail` / trail half of `prepare_effects`.

## Tests

Existing, keep green:

- `mr-crabs-effects` filter `trail` (9 tests). Especially `move_captures_previous_rect_and_resets_fade`, `first_frame_glows_without_segment` (name stays; paint no longer glows first frame).
- `s9_effects_corpus_matches_fixtures` / `cursor-trail-fade`.
- `mr-crabs-element` `trail_glow_translates_by_origin`, `trail_glow_reports_degenerate_as_none`, `trail_segment_quad_handles_horizontal_vertical_diagonal_and_zero_length`.

Add (implementer):

1. Pure helper test. `trail_leftover_rect` from segment `(5,10)->(55,10)` and `glow_rect` 10x20 yields a 10x20 rect centered on `(5,10)`. Degenerate / no segment yields `None`.
2. `workspace` or element config test. `AccessibilityPolicy::from_flags(true, false)` produces `EffectsConfig.cursor_trail == false` even when settings say trail on.
3. Optional Lane A seam in `model.rs` `trail_fades_and_resumes_with_focus` after `cursor.col = 5`: `assert!(f.trail.active && f.trail.segment.is_some())`. Does not prove paint.

Do not add a GPUI window fixture in this unit unless one already exists.

## Capture proof (after code lands, not this lane)

P1 method only. `screencapture -x -o -S -l <CGWindowID>`. Always `-S`. No `get_window_state` in the timed region. Single on-screen window.

Script:

1. Launch one Mr Crabs window, trail on, duration 2000 ms, opacity 1, Reduce Motion off.
2. Place cursor, then jump ~5 columns (same scripted jump as S9).
3. Burst ~12 frames at ~85 ms through the fade window.
4. Crop the vacated cell vs live cell.

Pass:

- Vacated cell luma rises then falls across frames (not stuck ~13).
- Live cell stays the opaque cursor, not a second glow halo.
- After duration, vacated cell matches neighbors.
- Repeat with `AccessibilityPolicy` reduce_motion true (or OS flag via E). Vacated cell never lights.

A still of idle cursor is not evidence.

## Deviations

Read tool ignored `:N-M` selectors on large files after the first full snapshot. Line cites follow the first successful reads plus grep (`trail.rs:333`, `element.rs:960-986`, `element.rs:776-785`, `workspace.rs:508`).

No collaboration with Candidate B.

## Follow-up (out of scope)

H mechanical deletion of gradient cache after this paint unit. Candidate B may add `previous_rect` if shape-changing fades look wrong. Env paint-trace remains Jamie's product call (Lane A).

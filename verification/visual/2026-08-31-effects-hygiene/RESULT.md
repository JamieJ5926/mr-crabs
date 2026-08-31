# Lane H: effects crate consumer tracing

Status: PASS (read-only). No `crates/` edits. Output is untracked.

Tool note: `lsp references` returned "No language server found" for every symbol. Consumers below come from workspace `grep` plus call-site reads. Trait dispatch and re-exports were checked by reading `lib.rs` and every `use mr_crabs_effects` import in `crates/`. No `cfg(feature)` gates exist in `mr-crabs-effects` besides `cfg(test)`.

Preferences 2 and 12 still bind: stay on GPUI paint, do not add a Metal pass. The gradient/texture APIs are the leftover shader-resource contract from the oracle port.

## Consumer table

| Item | Kind | Consumers | Tool | Verdict |
|---|---|---|---|---|
| `GradientId` (`trail.rs:165`) | public newtype | Field `TrailFrame.gradient`; filled by `GradientCache::get`; asserted in `trail.rs` unit tests; re-exported from `lib.rs:47-50`. Product paint never reads it. `s9_corpus.rs` `check_trail` does not mention `gradient`. | grep + `element.rs:960-986` | **hold for Lane A**. Re-exported but unused by product. Tests use it. |
| `MAX_GRADIENTS` (`trail.rs:168`) | public const | `GradientCache::get` overflow; unit test `gradient_cache_is_bounded_and_evicts_lru`; re-export. No product import. | grep | **hold for Lane A** (tied to cache). Re-exported, unused outside crate+tests. |
| `GradientEntry` (`trail.rs:171`) | private struct | `GradientCache` only. | grep | **hold for Lane A**. Internal to cache. |
| `GradientCache` (`trail.rs:183`) | public struct | Field on `CursorTrail`; constructed in `CursorTrail::new`; `get` from `CursorTrail::frame`; `len`/`is_empty`/`retained_capacity` from `CursorTrail` + tests. Re-exported. No other crate constructs it. | grep | **hold for Lane A**. Product never names the type. Cache still runs on every active trail frame. |
| `GradientCache::new` (`trail.rs:190`) | public ctor | `CursorTrail::new`, `Default`. | grep | **hold for Lane A**. |
| `GradientCache::get` (`trail.rs:200`) | public method | Only `CursorTrail::frame` at `trail.rs:337`. | grep | **hold for Lane A**. Live on the trail path even though paint ignores the id. |
| `GradientCache::len` (`trail.rs:233`) | public method | Unit tests via `gradient_cache()`. | grep | used only in tests |
| `GradientCache::is_empty` (`trail.rs:237`) | public method | Unit test `hidden_or_unfocused_or_disabled_draws_nothing`. | grep | used only in tests |
| `GradientCache::retained_capacity` (`trail.rs:242`) | public method | `CursorTrail::retained_capacity` → `EffectsModel::retained_capacity` → bench `workloads.rs` + model tests + s9 corpus disabled-path size. | grep | used in product-adjacent bench + tests, not paint |
| `GradientCache` Default (`trail.rs:247`) | impl | No call sites found besides the impl itself. | grep `GradientCache::default` empty | re-exported/impl unused |
| `CursorTrail::gradient_cache` (`trail.rs:292`) | public method | Unit tests only. Not re-exported as a standalone item beyond `CursorTrail`. | grep | used only in tests |
| `TrailFrame.gradient` (`trail.rs:143`) | public field | Written every active `frame()`; read only by trail unit tests. `paint_trail` uses `active`, `alpha`, `glow_rect`, `radius_px`, `segment`. | grep + `element.rs:960-986` | **hold for Lane A**. Dead to GPUI paint. |
| `EffectsModel::change_texture` (`model.rs:343`) | public method | Wrapper over `ChangeTracker::change_texture`. Callers: `model.rs` tests (`change_texture_flows_through_model`, disabled-path empty), `s9_corpus.rs:298` length assert. No `mr-crabs-element` / app / bench call. | grep | used only in tests. **hold for Lane A/I** if a shader reveal is ever reconsidered (prefs forbid that). |
| `EffectsModel::change_texture_dirty` (`model.rs:351`) | public method | Tests only (fresh tracker dirty, after apply, after clear). | grep | used only in tests |
| `EffectsModel::clear_change_texture_dirty` (`model.rs:358`) | public method | Tests only. | grep | used only in tests |
| `ChangeTracker::change_texture` (`key.rs:506`) | public method | `EffectsModel::change_texture` + key unit tests packing sentinels. | grep | used only in tests (via model) |
| `ChangeTracker::upload_dirty` / `clear_upload_dirty` (`key.rs:511-517`) | public methods | `EffectsModel` texture dirty API + key tests. Flag is set on every stamp/`repack` path. | grep | used only in tests. Flag still mutates in product `apply_frame`. |
| `ChangeTracker::repack` + `packed` (`key.rs:494-501`) | private | Called from every stamp/resize/clear/adopt. Allocates `packed` rgba8 bytes for the unused texture. | read `key.rs` | **hold**, but this is the real cost of the dead API. Product never reads `packed`. |
| `EffectsModel::last_change_ms` (`model.rs:367`) | public method | Tracker method used in product `apply_frame` for `needs_frame`. Model wrapper used by issue_19 tests and model tests. Element does not call the wrapper. | grep | used in product (tracker) / tests (model wrapper) |
| `EffectsModel::retained_capacity` (`model.rs:376`) | public method | Bench + tests + s9 corpus. Includes gradient cache bytes. | grep | used in bench/tests, not paint |
| `collect_reveals` (`model.rs:389`) | crate-private fn | Single call: `apply_frame` at `model.rs:309`, after tracker updates, every frame that has a tracker. Scans `0..tracker.tracked_cells()`, skips `NEVER_BITS` and elapsed >= duration. Feeds `fx.revealing` / `fx.pending`, which `paint_text_reveal` consumes. | grep + read | **used in product**. Not dead. Lane I owns bounding; Lane F measures. |

## Product paint vs shader contract

`mr-crabs-element` imports `CellPx, EffectsConfig, EffectsFrame, EffectsModel, RectPx, RevealMath, TextAnimation, TrailFrame` plus `LinePx`/`PointPx` in helpers. It does not import `GradientCache`, `GradientId`, or `MAX_GRADIENTS`.

`prepare_effects` (`element.rs:859-869`) calls `EffectsModel::apply_frame` and stores `&EffectsFrame`. It never calls `change_texture*`.

`paint_trail` (`element.rs:960-986`) paints GPUI drop shadows and a filled path. It never keys a texture by `GradientId`. That matches metal-gap `REVIEW.md:27`.

No feature-gated GPU path exists in this crate.

## What is held for Lane A

Do not remove:

- `GradientCache` and `GradientId` until A and B confirm the trail fix does not need a renderer-keyed gradient (prefs already forbid a second Metal pass, so the likely fix stays on `paint_drop_shadows` / path).
- `TrailFrame.gradient` assignment in `CursorTrail::frame`. Removing it changes the frame payload A may be comparing.
- `change_texture*` until A/I confirm they will not upload packed times. Prefs say stay on GPUI, so this is probably dead, but the brief says the trail fix may need exactly these descriptors.

Honest negative: **no product consumer of the gradient id or change texture exists today.** The cache still runs and `repack` still copies every tracked cell into `packed` on stamp. That is live cost for unused bytes, not a missing call site.

## Removal plan (later unit, after A+B)

Mechanical order. Each step should compile and keep s9 corpus green only after the matching asserts are dropped.

1. Confirm A and B do not need `GradientId` for paint.
2. Stop writing `frame.gradient` in `CursorTrail::frame`. Drop `TrailFrame.gradient` and its `Default`. Update trail unit tests that `assert_eq!(f.gradient, GradientId(0))`.
3. Delete `CursorTrail.gradient: GradientCache`, `gradient_cache()`, and the `get` call. `retained_capacity` on trail becomes 0. Update `hidden_or_unfocused...` and `gradient_cache_is_bounded_and_evicts_lru`.
4. Delete `GradientCache`, `GradientEntry`, `GradientId`, `MAX_GRADIENTS`. Drop them from `lib.rs` re-exports. Update `s9-contract.json` gradient bullet.
5. Confirm A/I do not need a change texture.
6. Delete `EffectsModel::change_texture`, `change_texture_dirty`, `clear_change_texture_dirty`. Delete model test `change_texture_flows_through_model` and the disabled-path texture asserts. Drop s9 corpus `change_texture().len()` check and the contract bullet.
7. Delete `ChangeTracker::change_texture`, `upload_dirty`, `clear_upload_dirty`, `repack`, field `packed`, field `upload_dirty`, and every `self.upload_dirty = true; self.repack();` (keep stamp logic). This is the allocation win. Key unit tests that assert texel bytes go away.
8. Optionally un-export `CursorTrail` / `TrailConfig` if still only used inside the crate (product uses `EffectsModel`, not `CursorTrail` directly). Out of the named line ranges; separate hygiene.

What would break if done early: trail unit tests, s9 texture length, `s9-contract.json` wording, and any in-flight A patch that starts keying GPUI resources by `GradientId`.

## `collect_reveals` note for I and F

Call site is only `apply_frame` at `model.rs:309`, inside `if let Some(tracker)`. It always walks `tracked_cells()` even when `frame_reveal_eligible` is false, after possible `clear_changes` / `adopt_rows` / `sync_rows_without_stamping`. Product paint depends on the output vectors. Bounding to changed cells is I; measuring the scan is F. This lane did not change it.

## Deviations

- No rust-analyzer. Verdicts are grep plus import and paint-path reads, not LSP. Re-run `lsp references` after a rust-analyzer attach if the coordinator wants the stronger evidence class.
- No builds or tests, per brief.

## Follow-ups

- After A+B, execute the removal plan as a mechanical lane.
- `ChangeTracker.packed` + `repack` is the hygiene item with actual heap cost. Gradient LRU is 16 small entries.
- `EffectsModel::last_change_ms` wrapper is test-only; tracker method is live. Not in the named ranges.

## Output directory

`verification/visual/2026-08-31-effects-hygiene/` (this file only). Untracked. No git mutation.

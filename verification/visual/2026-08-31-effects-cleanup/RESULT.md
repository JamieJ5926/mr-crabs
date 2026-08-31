# Lane H: effects hygiene removal

Status: PASS. Model: cli-proxy/dddai.grok-4.6. Fallback: none.

## What it does

Dead shader-resource contract is gone from `mr-crabs-effects`. Trail frames no longer carry `GradientId`. The change tracker no longer packs rgba8 upload texels. Stamp, clear, adopt, and `collect_reveals` stay.

## Deleted symbols

- `TrailFrame.gradient`
- `GradientId`, `GradientCache`, `GradientEntry`, `MAX_GRADIENTS`
- `CursorTrail.gradient`, `CursorTrail::gradient_cache`
- `EffectsModel::change_texture`, `change_texture_dirty`, `clear_change_texture_dirty`
- `ChangeTracker.packed`, `upload_dirty`, `repack`, `change_texture`, `upload_dirty()`, `clear_upload_dirty`

## Changed files

- `crates/mr-crabs-effects/src/trail.rs`
- `crates/mr-crabs-effects/src/model.rs`
- `crates/mr-crabs-effects/src/key.rs`
- `crates/mr-crabs-effects/src/lib.rs`
- `crates/mr-crabs-effects/tests/s9_corpus.rs`
- `verification/effects-corpus/s9-effects.json`
- `verification/visual/2026-08-31-effects-cleanup/RESULT.md`

## Blast-radius safety fact

Trail leftover fade (`active`, `alpha`, `radius_px`, `leftover_rect`, `segment`) and reveal vectors (`revealing`, `pending`, `needs_frame`) still work after those fields are gone. Proven at blast-radius step 4 by running the crate tests.

## Commands

```
CARGO_TARGET_DIR=~/.cargo-target cargo test -p mr-crabs-effects --offline --lib trail
CARGO_TARGET_DIR=~/.cargo-target cargo test -p mr-crabs-effects --offline --lib model
CARGO_TARGET_DIR=~/.cargo-target cargo test -p mr-crabs-effects --offline --lib key
CARGO_TARGET_DIR=~/.cargo-target cargo test -p mr-crabs-effects --offline --test s9_corpus
CARGO_TARGET_DIR=~/.cargo-target cargo check -p mr-crabs-effects --offline
```

Results: trail 9 passed, model 17 passed, key 14 passed, s9_corpus 1 passed, check finished.

## Risks

None confirmed in this crate. Product paint already ignored the deleted fields. `retained_capacity` no longer counts texel bytes or gradient LRU heap, so benches that assert exact byte counts outside this crate would need an update. Not in writable scope here.

## Cleared

`collect_reveals`, `last_change_ms`, trail leftover math, and s9 reveal/trail fixtures were not rewritten.

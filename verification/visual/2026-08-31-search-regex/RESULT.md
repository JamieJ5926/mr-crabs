# Lane L2 search regex

Status: finished, not rolled back.

Resolved model: cli-proxy/dddai.grok-4.6. Fallback: none (routing: workstation reports this model).

## Data shape

`SearchPattern::{Literal, Regex}` on `SearchRequest.pattern`. Default is `Literal`. Bad regex returns `SearchOutcome.pattern_error` and does not panic.

## Files changed (L-owned)

- `crates/mr-crabs-history/Cargo.toml` (regex 1.13.1)
- `crates/mr-crabs-history/src/lib.rs` (re-export `SearchPattern`)
- `crates/mr-crabs-history/src/search.rs`
- `crates/mr-crabs-history/tests/corpus.rs` (`pattern: SearchPattern::Literal`)
- `crates/mr-crabs-bench/src/workloads.rs` (two explicit `SearchRequest` sites)
- `verification/visual/2026-08-31-search-regex/RESULT.md`

No terminal crate files. No E/J/`element.rs` files. `pane.rs` already had `pattern`.

## Tests

`CARGO_TARGET_DIR=$HOME/.cargo-target cargo test -p mr-crabs-history --lib search --offline`

14 passed (32 filtered). Covers literal, regex match, no-match, bad-regex, case policy, reverse regex.

`CARGO_TARGET_DIR=$HOME/.cargo-target cargo test -p mr-crabs-history --test corpus --offline`

1 passed (`s8_history_corpus_passes`).

`CARGO_TARGET_DIR=$HOME/.cargo-target cargo check -p mr-crabs-bench --benches --offline`

Finished. Compiles `workloads.rs`. Pre-existing `mr-crabs-element` dead_code warning on `PreparedEffects.focused`/`now_ms`. Future-incompat note on `block v0.1.6`.

## Cargo.lock

This lane did not rewrite `Cargo.lock`. `regex 1.13.1` is already present in the lockfile.

## Gaps for highlight UI

`FrameDelta.search_matches` still unused by `element.rs`. Remaining explicit `SearchRequest` initializers in L-owned targets now set `pattern: SearchPattern::Literal`. The `..SearchRequest::default()` worker cancel site already inherits `pattern`.

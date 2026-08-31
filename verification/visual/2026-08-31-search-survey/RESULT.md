# Lane L survey: scrollback search today

Status: PASS. Read-only. No crates/ edits. Output untracked.

Playbook: Investigation (how, Explain). Throughput checkpoint: n/a, read-only investigation.

Principles used: Prove It Works (named tests, not GUI). Guard the Context Window (no child spawn; how-skill spawn skipped because this seat cannot spawn children). Unslop on this file.

## Overview

Scrollback search is a Ghostty-shaped literal substring scan over encoded history plus a captured visible grid. The matcher lives in `mr-crabs-history`. The app pane slices that scan, cycles matches, and projects them onto `FrameDelta`. The renderer does not paint `search_matches` yet. There is no regex. There is no as-you-type search field.

## Answers

### 1. What matching does search perform today?

Literal UTF-8 substring over the search stream. Not regex. Not Unicode case fold.

`find_needle` walks `hay[from..].windows(needle.len())` and compares bytes (`crates/mr-crabs-history/src/search.rs:547-564`). Case-insensitive mode uses ASCII-only `eq_ignore_ascii_case` (`:566-568`). The module comment states this and pins Ghostty `indexOfIgnoreCase` (`:11-12`). Test `find_needle_is_ascii_case_insensitive` proves `"ÄBC"` does not match `"äbc"` (`:1031-1042`).

`SearchRequest.case_sensitive` exists and defaults to `false` (`:171-194`). Pane search always constructs the request with `case_sensitive: false` (`crates/mr-crabs-app/src/model/pane.rs:1428-1434`). No UI or action toggles case sensitivity.

Needle cap is 255 bytes (`MAX_NEEDLE_BYTES`, `search.rs:45-46`). Soft-wrapped rows join; non-wrapped rows emit `\n`; wide spacers and trailing blanks are stripped (`:13-16`, `encode_row_with_map` at `:82-121`).

### 2. Incremental as the user types, or on submit?

On command, not as-you-type.

`AppAction::SearchNext` / `SearchPrevious` (`crates/mr-crabs-app/src/action.rs:71-74`) are bound to `cmd+shift+g` and `cmd+shift+h` (`keymap.rs:142-143`). Dispatch clones `AppModel.search_query` and calls `pane.search` (`app_model.rs:1525-1539`). `set_search_query` only stores the string (`:1144-1146`). Grep shows no production caller of `set_search_query` outside tests.

Empty query is ignored (`NoNeedle`, `pane.rs:1396-1403`, `app_model.rs:1554-1555`). Same needle on a completed search cycles the match list without rescanning (`pane.rs:1406-1416`). A new needle starts a sliced scan (`:1419-1439`).

Pump continues an in-flight slice (`pane.rs:1617-1620`, `1649-1653`). That is bounded-scan progress, not keystroke highlighting.

There is no search box that re-runs on each character.

### 3. How are matches projected, and how many at once?

Engine type: `SearchMatch` with `spans: Vec<SearchSpan>`, `start_line`, `start_col` (`search.rs:159-166`). Outcome holds `Vec<SearchMatch>` (`:203`). Default limit 1000, hard cap 100_000 (`:47-50`). Truncation sets `truncated: true`.

Pane stores that vec, plus `index` for the current hit (`finish_search_selection` at `pane.rs:1476-1488`). Frame projection `search_frame_matches` (`:1091-1155`) writes `Vec<FrameSearchMatch>` on `FrameDelta.search_matches` (`crates/mr-crabs-terminal/src/delta.rs:120-125, :171`).

`FrameSearchMatch` is not a single-match type. It is `{ range: FrameRange, current: bool }`. One logical `SearchMatch` becomes one half-open row-major range from first span to last span, clipped to the viewport. Empty or inverted ranges drop. `current` is `index == self.search.index`. Ranges sort by row then col. Visible-only: matches wholly above or below the viewport are omitted.

The current match is also projected as `SelectionState` via `search_selection` (`pane.rs:1223-1255`) when there is no user selection (`:1057-1058`).

Cap on the frame vec is "all stored matches that intersect the viewport", and stored matches are capped at `DEFAULT_SEARCH_LIMIT` (1000) at the pane (`:1444-1446`). Not a single-match assumption.

`element.rs` paints selection quads only (`crates/mr-crabs-element/src/element.rs:726-728`). Grep of that crate for `search_matches` returned no hits. Highlight quads for search are not implemented.

### 4. What is tested, and what is untested?

**`mr-crabs-history` lib (`search.rs` tests, 11 ran, all ok)**

- `row_encoding_trim_skip_and_join`
- `find_needle_is_ascii_case_insensitive`
- `search_forward_across_wrapped_rows`
- `search_reverse_and_limit_and_start`
- `search_crosses_compressed_and_uncompressed_pages_identically`
- `worker_delivers_results_and_generation_invalidates`
- `worker_cancellation_stops_mid_search`
- `worker_replaces_pending_request`
- `search_spans_visible_rows`
- `needle_truncation_is_bounded`
- `bounded_slices_resume_and_preserve_wrapped_matches`

**Corpus:** `crates/mr-crabs-history/tests/corpus.rs` `run_search_case` (`:48-108`). Kind `"search"` (`:401`). JSON cases cover needle, direction, start, limit, `case_sensitive`, visible rows. Not run this unit (history `--lib search` only).

**`mr-crabs-app` pane/app (9 named tests, all ok)**

- `search_selects_visible_match_and_sets_selection` (`pane.rs:2876`)
- `search_next_wraps_and_previous_goes_back` (`:2892`)
- `search_no_match_and_empty_needle_clear_selection` (`:2915`)
- `search_match_in_history_updates_viewport` (`:2927`)
- `large_search_advances_in_bounded_pump_slices` (`:2950`)
- `frame_search_matches_emits_all_visible_sorted_half_open` (`:3128`)
- `frame_search_matches_clips_and_omits_when_scrolled` (`:3164`)
- `search_commands_dispatch_through_the_keymap` (`app_model.rs:2003`)
- `search_commands_require_a_query_and_are_registered_in_the_palette` (`:2044`)

**`mr-crabs-terminal`:** 109 tests, 4 suites, all ok. Search-related: `clear_for_reuse_clears_search_matches_and_hyperlinks_retaining_capacity` (`delta.rs:551`). No matcher tests here.

**Gaps**

- Regex: no type, no tests.
- As-you-type incremental highlight: no UI, no tests.
- Pane never sets `case_sensitive: true`.
- No production `set_search_query` path.
- `element.rs` does not paint `frame.search_matches`.
- Wrapped match projected as one `FrameRange` (start of first span to end of last), not per-span quads. Untested visually; unit tests cover the DTO only.

### 5. Regex change shape (not an implementation)

Engine side (`mr-crabs-history`, can land without `element.rs`):

- `SearchRequest`: add a mode (`literal` vs `regex`) next to `needle` and `case_sensitive` (`search.rs:171-181`). Regex compile errors need an outcome path.
- `find_needle` (`:549`) cannot stay a fixed-width window walk. Regex match length is not `needle.len()`. Span mapping (`spans_for`, `:570`) already takes `[start, end)` byte offsets, so variable-length matches can reuse it if the stream encoding stays the same.
- Reverse search reverses the stream and the needle (`:369-374`). Regex reverse is a different design (scan forward and pick last, or unsupported).
- `MAX_NEEDLE_BYTES` 255 may be wrong for patterns.
- `search_slice` / `search_core_range` / `SearchWorker` keep the same request type.
- Pane `PaneModel::search` (`pane.rs:1395`) always builds a literal request. Regex would enter here, plus an action or query flag in `AppModel`.
- Corpus `run_search_case` and `find_needle_is_ascii_case_insensitive` need regex cases.

Renderer side (`element.rs`, later wave):

- `FrameSearchMatch` already carries many ranges. Regex does not need a new frame type unless you want per-span quads instead of one collapsed range (`search_frame_matches` first-to-last span).
- Paint loop next to selection at `element.rs:726-728` is the missing highlight. Independent of regex.

## Crate split

| Capability | Owner | Wave |
|---|---|---|
| Literal match, wrap join, limits, worker, slices | `mr-crabs-history` (`search.rs`) | engine |
| `FrameSearchMatch` DTO | `mr-crabs-terminal` (`delta.rs`) | already there |
| Query store, next/prev, slice pump, viewport jump, frame fill | `mr-crabs-app` (`pane.rs`, `app_model.rs`) | engine + app |
| Regex / case toggle / search field | history + app | engine, not `element.rs` |
| Highlight quads | `mr-crabs-element` `element.rs` | later, contended |
| Current-match selection overlay | already via `selection` | incidental, not search quads |

## How it works

1. Someone sets `AppModel.search_query` (tests only today).
2. Search Next/Previous dispatches `pane.search(needle, forward)`.
3. Pane scans history with `search_slice` in 4096-line budgets (`SEARCH_SLICE_BUDGET`, `pane.rs:463`).
4. Matches accumulate up to 1000. Viewport jumps to the chosen line. Frame rebuild fills `search_matches` and selection.
5. Element paints selection, not search quads.

## Where things live

- Matcher: `crates/mr-crabs-history/src/search.rs`
- Frame DTO: `crates/mr-crabs-terminal/src/delta.rs:120-171`
- Pane: `crates/mr-crabs-app/src/model/pane.rs` (`search`, `search_frame_matches`)
- Actions: `crates/mr-crabs-app/src/action.rs:71-74`
- Paint gap: `crates/mr-crabs-element/src/element.rs:726-728`

## Gotchas

- Module docs say case-insensitive; pane hard-codes that even though `SearchRequest` can be sensitive.
- `SearchNext` on a completed needle walks the list backward (`pane.rs:1411-1414`). Naming vs wrap direction is easy to get wrong.
- Reverse `search_slice` does a full `search_core`, not a true reverse slice (`search.rs:289-292`).
- History mutation clears search (`invalidate_stale_history_views`, `pane.rs:1386-1389`).
- Collapsed multi-span `FrameRange` can cover cells that are not part of the match (wrapped gap).

## Tests run

```
CARGO_TARGET_DIR=~/.cargo-target cargo test -p mr-crabs-terminal --offline
  109 passed, 4 suites

CARGO_TARGET_DIR=~/.cargo-target cargo test -p mr-crabs-history --offline --lib search
  11 passed, 32 filtered

CARGO_TARGET_DIR=~/.cargo-target cargo test -p mr-crabs-app --offline --lib -- \
  search_selects_visible_match_and_sets_selection \
  search_next_wraps_and_previous_goes_back \
  search_no_match_and_empty_needle_clear_selection \
  search_match_in_history_updates_viewport \
  large_search_advances_in_bounded_pump_slices \
  frame_search_matches_emits_all_visible_sorted_half_open \
  frame_search_matches_clips_and_omits_when_scrolled \
  search_commands_dispatch_through_the_keymap \
  search_commands_require_a_query_and_are_registered_in_the_palette
  9 passed, 375 filtered
```

No full workspace suite. No GUI. No git mutation.

## Deviations

how-skill explorer/explainer spawn skipped: this seat must not spawn children. Direct explain.

App search tests ran in addition to `mr-crabs-terminal` because they are the search-specific tests the brief allows by name.

## Follow-ups

- Search field + `set_search_query` production path if incremental highlight is desired.
- Regex in `SearchRequest` + reverse-search policy.
- `element.rs` search quads (later wave, contended file).
- Expose `case_sensitive` from the pane if the request flag is meant to be user-facing.

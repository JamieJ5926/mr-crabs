---
name: mr-crabs-style-compaction-safety
description: "Plan, implement, or review safe u16 style-table compaction in Mr Crabs, including compression quiescence and colored-history viewport/persistence contracts."
---

## Purpose
Plan or review the Mr Crabs `u16` style-table capacity repair without corrupting active rows, scrollback, saved pens, viewport frames, or persistence.

## Preconditions
- Work only in the canonical Mr Crabs checkout.
- Re-read current `side_tables.rs`, `compact/{engine,state,row}.rs`, `storage.rs`, terminal `lib.rs`, and history `viewport.rs`, `replay.rs`, `persist.rs`.
- Preserve `Cell` as `#[repr(C)] { content: u32, style: u16, flags: u16 }` and keep style ID 0 as default.
- Treat prior plans as evidence, not current implementation proof.

## Required owner census
Count/remap exactly:
- current engine pen;
- `primary.saved.pen`;
- `alternate.saved.pen`;
- primary and alternate active rows;
- primary and alternate engine history;
- storage hot Flat/Segmented pages;
- storage cold compressed pages.

Do not treat frame deltas, normalized snapshots, renderer caches, or recycled row pools as semantic owners.

## Quiescence gate
Do not use `drain_compression()` as the compaction barrier if it still calls `maybe_enqueue_full_pages` at the end. Add/use a private transaction barrier that:
1. applies completions while waiting for queued jobs;
2. performs a final apply;
3. does not re-enqueue;
4. proves `queued_jobs == 0`, `pending_completions == 0`, and every page has `pending == false`.

Run cardinality census only after this barrier under exclusive `&mut Terminal`.

## History-style gate
Before compaction, verify/fix these contracts:
- `CompactEngine::snapshot()` is active-grid-local and frame-local.
- `viewport::project_frame` must remap engine-global history IDs into the frame-local style table before `batch_runs`.
- Every projected `Cell.style` and `Run.style` must be `< frame.styles.len()` and resolve the original color for hot and cold history.
- `TerminalSnapshot` and `HistoryFile` must carry a complete style table for their raw history cells, validate all style bounds, install the table before restoring history, and bump encoded format versions when layouts change.
- Do not broaden `NormalizedSnapshot` to include history.

## Compaction transaction
After a full-cold-inclusive live-set probe proves sufficient headroom:
1. quiesce compression without re-enqueue;
2. build combined live census;
3. build deterministic dense old→new map with `0→0`;
4. preflight/decode/validate every cold page and stage all fallible allocations;
5. remap storage hot/cold pages and bump page generations;
6. remap all engine rows and all three pens while preserving `RowExtras`;
7. replace the style table and increment one authoritative epoch;
8. clear recycle pools;
9. mark full damage and bump row generations;
10. resume normal compression scheduling.

Never use `CompactRow::from_parts` during remap if it drops extras. Never skip a corrupt cold page and continue; abort before mutation/table replacement.

## Trigger safety
Trigger before interning can overflow, not after feed. Bound new styles per feed chunk against remaining `u16` headroom. If the full live set approaches capacity, stop and return to specification; direct-RGB sidecars are a separate design, not an automatic fallback.

## Acceptance
- Three saved/current pens restore the same colors after compaction.
- Both screens, engine history, hot/cold storage, and stale compression completions are correct.
- Held pre-compaction frame remains valid; next frame is full damage.
- Colored hot/cold viewport rows resolve correctly.
- Snapshot and history-file colored round trips preserve styles.
- Termbench FGPerChar/FGBGPerChar Small and Normal complete without panic or recoloring.
- Full serial terminal/history tests, workspace checks, clippy, and headless runtime gates pass before claiming completion.

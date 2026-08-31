# Lane N: frame/cache cluster payload

routing: cli-proxy/dddai.grok-4.6, fallback unverified (workstation model line)

Status: PASS for this unit. Occupancy unchanged. GPUI paint only. No Metal. Scrolled hot/cold history, cold partial front eviction, and same-width resize restoration preserve combining extras.

## What is fixed

`GraphemeTable` extras already attached in the engine now reach paint text, and paint consumes one width per cluster.

1. `RowDelta.combining` is `Vec<(u16, Vec<u32>)>`: column in that row, extra scalars. Not a copy of the whole side table.
2. `Terminal::build_frame_delta` copies matching `NormalizedSnapshot.combining_marks` into each damaged row. Snapshot still strips `Cell::COMBINING` from cell flags; extras live on the row payload.
3. `RowDelta::extras_at(col)` is the cache lookup.
4. `RenderCache::fill_batch` appends those extras after the base scalar. `glyph_widths` still gets one entry per cell (1 or 2). Wide spacers still emit no text.
5. `RunBatch.cluster_starts` records the byte offset of each base cell. Combining extras share that start. `paint_run_cell_aligned` and `cell_col_for_glyph_byte` advance columns from those starts, never from every Unicode scalar.

Proven: `e` + U+0301 is one cell. Frame extras are `[0x0301]`. Cache run text is `é` with `glyph_widths == [1]` and `cluster_starts == [0]`. `e\u{0301}x` maps the `x` cluster to column 1, not 2. Scrolled `e\u{0301}x!` history projects the same extras before and after forced compression, same-width height growth restores the combining mark into the visible snapshot, and forced-cold multi-row partial front eviction keeps each retained row paired with its own distinct combining mark.

## What still remains

Occupancy is still unicode-width per scalar. No grapheme iterator.

- ZWJ family stays several wide cells. ZWJ (width 0) attaches to the previous person cell, so extras can now reach that cell's run text, but CoreText never sees the full family string as one cluster.
- Flags stay two regional-indicator cells.
- Skin tone attaches only if unicode-width reports 0 for the modifier. Not measured here. No extra occupancy test added.

Paint already shapes whatever `RunBatch.text` is. Colour emoji still depends on GPUI `is_emoji`. No second renderer.

## Blast radius

**What it does.** Extra scalars on changed rows only. Cache concatenates them into existing run text. One width per occupied cell. Paint indexes widths by cluster start, not by scalar.

**The one fact it's safe because of (step 4).** Spacer cells still skip text, and a combining cell still contributes one width. A later base scalar does not inherit a combining mark's phantom width. Proof: `spaces_and_wide_spacers_produce_no_text_runs` still passes (`glyph_widths == [2, 1]`, spacer silent). `combining_cluster_shapes_full_text_with_one_width` passes (`"e\u{0301}"`, one width). `combining_then_next_cell_maps_to_immediate_next_column` and `combining_cluster_then_x_maps_second_glyph_to_next_column` pass (`x` at column 1). Occupancy test `combining_mark_attaches_to_previous_cell` still passes.

**Risks.** If a future occupancy change stores a second visible scalar as extras, glyph columns would still share the first cell. Unchanged occupancy keeps that case out.

**Cleared.** Presentation-flag run split still ignores `COMBINING` (`0x8000`). Dock synthetic frames map dock combining into the new field. History `project_frame` now forwards scrolled-row combining sidecars from both hot segmented rows and forced-compressed cold reads. Cold/restored-hot flat `trim_front_page` drains the dropped sidecar rows with the retained cells so partial front eviction stays aligned.

## Commands

```
CARGO_TARGET_DIR=~/.cargo-target cargo test -p mr-crabs-terminal --offline extras_at_returns_attached_scalars
CARGO_TARGET_DIR=~/.cargo-target cargo test -p mr-crabs-terminal --offline frame_delta_carries_combining_extras_for_acute_e
CARGO_TARGET_DIR=~/.cargo-target cargo test -p mr-crabs-terminal --offline combining_mark_attaches_to_previous_cell
CARGO_TARGET_DIR=~/.cargo-target cargo test -p mr-crabs-element --offline combining_cluster_shapes_full_text_with_one_width
CARGO_TARGET_DIR=~/.cargo-target cargo test -p mr-crabs-element --offline combining_then_next_cell_maps_to_immediate_next_column
CARGO_TARGET_DIR=~/.cargo-target cargo test -p mr-crabs-element --offline combining_cluster_then_x_maps_second_glyph_to_next_column
CARGO_TARGET_DIR=~/.cargo-target cargo test -p mr-crabs-element --offline spaces_and_wide_spacers_produce_no_text_runs
CARGO_TARGET_DIR=~/.cargo-target cargo test -p mr-crabs-terminal --offline combining
CARGO_TARGET_DIR=~/.cargo-target cargo test -p mr-crabs-history --offline scrolled_combining_projection_survives_hot_and_cold_history
CARGO_TARGET_DIR=~/.cargo-target cargo test -p mr-crabs-element --offline combining_
CARGO_TARGET_DIR=~/.cargo-target cargo check -p mr-crabs-terminal --offline
CARGO_TARGET_DIR=~/.cargo-target cargo check -p mr-crabs-history --offline
CARGO_TARGET_DIR=~/.cargo-target cargo test -p mr-crabs-terminal --offline cold_partial_front_eviction_shifts_combining_sidecar
CARGO_TARGET_DIR=~/.cargo-target cargo check -p mr-crabs-element --offline
```

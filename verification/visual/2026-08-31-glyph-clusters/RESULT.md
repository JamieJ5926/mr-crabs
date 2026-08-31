# Lane N: glyph clusters and colour glyphs (source-only)

routing: cli-proxy/dddai.grok-4.6, fallback unverified (workstation model line; no parent routing receipt)

Status: PASS for this unit. Source-backed answers only. No `crates/` edits. No custom compile of the leftover probe under this directory.

## What the CoreText/GPUI path does today

Cells are 8 bytes (`content: u32`, `style: u16`, `flags: u16`). Extra codepoints live in `GraphemeTable` (`crates/mr-crabs-terminal/src/side_tables.rs`), keyed from `RowExtras.combining`. That matches the Metalterm “lazy side table, cell stays small” claim.

`CompactEngine::input` (`engine.rs` `input`, `attach_combining`) classifies each scalar with `unicode-width` 0.2.2 (`compact/width.rs`):

- width `None`: C0/C1/DEL dropped
- width 0: attach to previous cell (skip spacer), intern into `GraphemeTable`, set `Cell::COMBINING`
- width 1: one cell
- width ≥ 2: `WIDE` plus `WIDE_SPACER`

There is no grapheme-cluster iterator. Family ZWJ, flags, and skin-tone sequences are not one cell unless every trailing scalar has width 0.

Paint never sees the side table. `FrameDelta` / `RowDelta` (`delta.rs`) carry cells only. `RenderCache::fill_batch` (`cache.rs`) pushes `char::from_u32(cell.content)` and skips spacers. `PRESENTATION_FLAGS = 0x7b8f` does not include `COMBINING` (`0x8000`), so the combining bit does not even split runs. Combining marks, ZWJ, VS-16, and tag characters that attached in the engine are dropped before shaping.

Shaping is GPUI `window.text_system().shape_line` on that truncated `run.text` (`element.rs` around the `shape_line` call). Paint is `paint_run_cell_aligned`: if `glyph.is_emoji` then `window.paint_emoji(...)` with no terminal foreground; else `paint_glyph` with run colour. Colour emoji therefore exist only when GPUI/CoreText mark a shaped glyph as emoji. There is no Mr Crabs font-family override for Apple Color Emoji. A baseline still of “no colour glyphs” is consistent with this pipeline if the run text never contains a colour-font cluster, or if the resolved face has no colour glyphs.

## Family / flags / skin tones (source prediction)

| Sequence | Engine occupancy | Paint text | Colour glyph |
|---|---|---|---|
| Simple emoji `🎉` (U+1F389) | 2 cells (`WIDE` + spacer). Proven. | `"🎉"` | Possible if GPUI sets `is_emoji` |
| Combining `e` + U+0301 | 1 cell + side table. Proven. | `"e"` only | n/a |
| Skin `👋🏻` (base + U+1F3FB) | Base width 2, then Fitzpatrick is `Sk` / EAW W. If `UnicodeWidthChar` returns 2, a second wide cell is written instead of attaching. If 0, it attaches. No existing test. | Base scalar only unless attached and later reconstructed | Broken if occupancy splits; still uncoloured if cache omits the modifier |
| ZWJ family `👨‍👩‍👧‍👦` | Each person is wide (2). ZWJ is `Cf` width 0 and attaches to the previous person. Occupancy is several wide cells, not one. | First scalar of each person cell | CoreText never sees the ZWJ string |
| Flag `🇬🇧` (two regional indicators, EAW N) | Two width-1 cells unless unicode-width treats RI as 2 | Two letters | Two glyphs, not a flag ligature |
| England `🏴󠁧󠁢󠁥󠁮󠁧󠁿` (flag + tag sequence) | Base wide; tags are `Cf` width 0 so they attach if they follow the same cell | Base flag only | Tags dropped at cache |

Metalterm “one cell, font colour glyph” is not what this tree does.

## Existing tests (ran)

All with `CARGO_TARGET_DIR=~/.cargo-target`. No workspace suite.

- `mr-crabs-terminal` `compact::tests::combining_mark_attaches_to_previous_cell` PASS
- `mr-crabs-terminal` `compact::width::tests::ascii_and_emoji_match_unicode_width` PASS (`🎉`-class purple circle U+1F7E3 is width 2)
- `mr-crabs-terminal` `tests::wide_combining_emoji_style_hyperlink_semantic_roundtrips` PASS (feeds single-scalar `🎉` only; no ZWJ/flag/skin)
- `mr-crabs-terminal` `tests/omp_like_headless.rs` `sgr_red_and_wide_emoji_widths` PASS (`Hi` + SGR + `界` + `🎉`, cursor col 7, wide+spacer)
- `mr-crabs-element` `spaces_and_wide_spacers_produce_no_text_runs` PASS (wide CJK into `glyph_widths` `[2, 1]`)

No GPUI shaping test asserts `is_emoji` or `paint_emoji`. Geometry tests only re-anchor cell origins.

## Gaps

- No test for ZWJ family, RI flags, Fitzpatrick, VS-16, or tag flags.
- No test that `FrameDelta` preserves combining text into paint runs (it currently cannot).
- No test of `glyph.is_emoji` / `paint_emoji`.
- No measurement of `UnicodeWidthChar` for U+1F3FB / RI / ZWJ (inferred from Unicode categories plus the width wrapper).
- Leftover cancelled probe `verification/visual/2026-08-31-glyph-clusters/{Cargo.toml,src/}` was not compiled.

## Later `element.rs` fix seam (do not land here)

Ownership: N owns glyph/cluster paint in `element.rs`. Engine occupancy is `mr-crabs-terminal` (coordinate; not this batch). Stay on GPUI paint. No app Metal.

1. **Occupancy (engine, if product wants one cell per grapheme).** `CompactEngine::input` / `input_run`: consume an extended grapheme (or emoji ZWJ sequence) as one write. Keep `GraphemeTable` for extra scalars. Keep `unicode-width` for the cluster’s display width so TUI apps still match, or document a deliberate unicode-vs-legacy split (Ghostty `grapheme-width-method`).
2. **Frame contract.** Add combining/cluster payload on `RowDelta` (cell index → extra codepoints, or interned ids plus a table on `FrameDelta`). Today the paint crate cannot reconstruct clusters.
3. **Cache.** `cache.rs` `fill_batch`: `RunBatch.text` is the full cluster string; `glyph_widths` is one entry per cluster (1 or 2), not per scalar. That is the shape `shape_line` needs for CoreText ligature/colour fonts.
4. **Paint.** `element.rs` `shape_line` already shapes whatever text the cache gives it. `paint_run_cell_aligned` already branches `paint_emoji` vs `paint_glyph`. After (2)+(3), keep cell-origin anchoring via `glyph.index` vs cluster widths. Do not add a second renderer.
5. **Font.** Optional later: request a colour-emoji fallback family from Lane C. Not required to prove the cluster string reaches CoreText.

### Test shape (later)

Headless, no GUI:

- Engine: feed `🎉`, `👋🏻`, `🇬🇧`, `👨‍👩‍👧‍👦`; assert cell count, `WIDE`/`COMBINING`, and `combining_marks` contents.
- Cache: `RowDelta` with combining extras → `RunBatch.text` equals the cluster, `glyph_widths` length 1.
- Element: keep geometry tests; add a unit that maps a multi-scalar cluster to one `glyph_cell_cols` slot. `paint_emoji` remains a GUI/P1 claim.

## Constraints honoured

No git mutation. No `crates/` writes. No `element.rs` edit. No probe compile. No GUI.

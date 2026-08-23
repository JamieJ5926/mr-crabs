# GPU Terminal Architecture: Investigation Report

**Worktree:** `performance-hillclimb` · **Branch:** `perf/gpu-rendering` · **Date:** 2026-08-21
**Scope:** Whether Mr Crabs needs GPU work, what it would target, and why no second renderer is justified.
**Mode:** Explanation, source-cited. No implementation landed.

## 1. Summary decision

Mr Crabs already renders through GPUI to Metal. The 12.965 s Metal System Trace contains 115 command-submission rows, of which 111 belong to the launched Mr Crabs process. Its median render-encoder interval is 41.667 µs, with about 5.035 ms of cumulative encoder intervals. These application-side intervals are not direct GPU execution time and do not support a GPU-duty percentage. The independent CPU profile is dominated by terminal scroll and storage, while GPUI render and paint functions appear in about five inclusive samples. A second renderer, private atlas, raw Metal, or wgpu path is explicitly rejected.

Three bounded CPU-side designs were ranked. None were implemented. The real-surface Metal and CPU baselines are captured, and neither trace supports a GPU-path prototype. The next step is CPU profiling and optimization of terminal scroll, storage, ingest, and ASCII input.

## 2. Premise: GPUI already uses Metal

`Cargo.toml` pins the sole GPU path:

- `gpui = { version = "0.2.2", rev = "03e5ad8a" }` is the app interface. `gpui_macos 0.1.0` owns the native macOS renderer and depends on the `metal` crate. `gpui_wgpu 0.1.0` is a separate GPUI package in the lockfile, not the owner of this macOS render path.

Relevant pinned GPUI behavior, verified against `03e5ad8a`:

- `gpui/src/window.rs:3132-3135` — submits the complete `rendered_frame.scene` on every present.
- `gpui_macos/src/metal_renderer.rs:490-519` — writes instances for the complete scene.
- `gpui_macos/src/metal_renderer.rs:651-670` — clears the full drawable and iterates every scene batch.
- `gpui/src/scene.rs:41-69, 141-148, 151-190, 269-466` — typed `Scene` vectors, `Scene::finish` overlap/texture sort, `Scene::batches` grouping.
- `gpui/src/scene.rs:141-148` + `gpui/src/view.rs:362-476` — scene replay through entity-backed cached views (`Entity::cached`).
- `gpui_macos/src/metal_atlas.rs:31-57, 98-188` — GPUI owns the 1024×1024 growing glyph/image atlas; Mr Crabs must not duplicate it.
- `gpui/src/window.rs:1786-1789` — device recovery bypasses replay; atlas loss is GPUI-owned.
- `gpui/src/app.rs:1740-1742` + `gpui/src/view.rs:386-391` — `App::refresh_windows` sets `window.refreshing = true`; `Window::refresh` bypasses cached-view reuse. Current `crates/mr-crabs-app/src/ui/wake.rs:117-118` blanket `refresh_windows` defeats cached replay unless replaced by targeted pane notification.

GPU work in this repo therefore means reducing CPU preparation for the existing GPUI/Metal path, not adding a second GPU path. Any design that requires a private atlas, raw `MTLCommandBuffer`, or wgpu submission fails the premise.

## 3. Methodology

### 3.1 Traces collected

Two `xctrace` recordings of the same release build (`~/.cargo-target/release/mr-crabs`) at `208×54` cells, all effects disabled:

| Artifact | Template | Process | Duration | File |
|---|---|---|---|---|
| GPU baseline | Metal System Trace | `mr-crabs` pid 95824, `run-fullscreen.sh` | 12.965 s (2026-08-21T16:55:37→16:55:50) | `/tmp/mr-crabs-gpu-baseline.trace` (24 MB) + TOC `/tmp/mr-crabs-gpu-baseline-toc.xml` |
| CPU baseline | Time Profiler (1 ms sample) | same workload | ~same window | `/tmp/mr-crabs-time-baseline.trace` (9.9 MB) + sample XML `/tmp/mr-crabs-time-profile.xml` (3.3 MB) + TOC `/tmp/mr-crabs-time-baseline-toc.xml` |

Detail tables exported from the GPU trace:

- `/tmp/mr-crabs-metal-application-command-buffer-submissions.xml` — 115 table rows, including 111 for the launched Mr Crabs process
- `/tmp/mr-crabs-metal-application-intervals.xml` — application command-buffer and render-encoder intervals
- `/tmp/mr-crabs-metal-gpu-execution-points.xml` + `*-completed.xml` — execution points
- `/tmp/mr-crabs-gpu-time-samples.xml` — correlated CPU/GPU samples

All commands below reproduce these artifacts without modifying measured semantics.

### 3.2 What was measured

The workload is the `run-fullscreen.sh` full-screen vtebench variant exercised through GPUI in a real window with `cursor-trail=false`, `text-animation=none`, `initial-window-size=208x54`. It is not the headless `scrollback_1m` S12 bench (`crates/mr-crabs-bench/src/workloads.rs`). Headless `S12` timings (`verification/results.json:286-291` marks `gui_frame_time`, `window_redraw`, `strict_gui_idle` as `blocked_never_pass`) cannot prove a GPU-sensitive improvement and were not used as a keep gate here.

### 3.3 Comparator sources inspected

- **Alacritty current** — commit `7dd7b5b09e06` (dev 0.18.0) and **Alacritty v0.10.1** benchmark era — commit `2844606`. Both inspected directly.
- **WezTerm** — commit `770d8e1a` (primary-source package complete; 5 candidates extracted).
- **Warp** — commit `8936686f261e`; `warpui` and `warpui_core` are MIT, while the remaining client is AGPL-3.0-only. The current terminal and renderer were inspected directly.
- **Mr Crabs** — this worktree at `0021b66d9068` on `perf/gpu-rendering`; GPUI pinned at `03e5ad8a`.

## 4. Measured trace facts

### 4.1 GPU intervals (Metal System Trace, 12.965 s)

- **Command-submission rows:** 115 total; 111 for the launched Mr Crabs process
- **Median render-encoder interval:** 41.667 µs
- **Cumulative render-encoder intervals:** ~5.035 ms

Interpretation: command encoding is short, and the CPU sample ordering points to terminal mutation and storage rather than GPUI paint. The trace does not expose enough symbolicated GPU execution duration to calculate GPU duty. This evidence blocks a second renderer, but it does not prove that the GPU is idle.

### 4.2 CPU samples (Time Profiler, inclusive weights over the same window)

Top inclusive symbols (sample counts, not wall time):

- `CompactEngine::scroll_up_relative` — 38
- storage worker (compression/paging path in `crates/mr-crabs-terminal/src/storage.rs`) — 37
- `Terminal::ingest_scrolled` — 33
- `input_ascii_run` (VTE scan fast path, `crates/mr-crabs-terminal/src/protocol.rs`) — 24
- `PaneModel::pump` (`crates/mr-crabs-app/src/model/pane.rs`) — 19
- GPUI render/paint functions (`render`, `paint`, scene submission) — ~5 each

Paint, scene sort, and Metal encoding are not in the top ranks. The hottest path is scroll damage and storage ingest/compression, not present or shading. Headless `S12` scrollback at `1M line\n / 5 MB` shows the same ordering; GUI frame shaping is not the dominant cost in this workload.

### 4.3 Why these numbers block GPU work

The keep predicate for any GPU-track candidate was defined as ≥5% improvement in **real-surface dirty-to-present time or frame energy**, measured in a real GPUI→Metal window, with Metal draw/pass counts and missed-frame count not regressing. The trace measures short application render-encoder intervals but does not provide a trustworthy GPU-duty total. The CPU sample ordering still places terminal ingest and storage above GPUI paint, so a second renderer, atlas fork, or instance-buffer pool has no measured win condition.

## 5. Architecture comparison

### 5.1 Mr Crabs CPU→GPU path (this worktree, `03e5ad8a`)

```
PTY bytes → bounded queue → dispatch_async_f / OutputWake (AtomicBool)
  → AppModel::pump(64) → PaneModel::pump (cap 64, drain_output → feed_chunk_scanned)
    → Terminal::feed (2560 B chunks, `crates/mr-crabs-terminal/src/lib.rs`)
      → CompactEngine damage Partial/Full + damaged_rows (`compact/engine.rs`)
      → ingest_scrolled descriptor moves (Arc<[Cell]>) → ScrollbackStorage paged hot/cold LZ4
      → poll_compression / recycle (bounded Job16/Completion16 try_send)
    → rebuild_frame: build_frame_delta (snapshot + generation + batch_runs)
    → project_frame (checked viewport, fail-closed) → Arc<FrameDelta> + FramePool cap 4
  → WindowView::render: CellMetrics cache per settings gen, SurfaceGeometry+commit_geometry,
    Arc clone, NamedInteger(PaneId) stable id, on_paint drain (`ui/workspace.rs`)
  → TerminalElement + PaintState (cache + shaped_lines + deduper + painted_font + blink)
    with_element_state (`crates/mr-crabs-element/src/element.rs`)
    → RenderCache apply_frame (Clean same-seq no-op / new-seq repaint / Partial-Full rebuild fill_batch)
      (`cache.rs`: RowBatch/RunBatch/RectBatch, SharedString + glyph_widths coalesce)
    → shape_line per RunBatch via CoreText (`WindowTextSystem::shape_line`)
    → paint_quad backgrounds + GraphicsOverlay z-sorted paint_image
      + paint_layer cell-anchored paint_glyph/emoji + selection/cursor/effects
    → request_animation_frame if blink || effects
  → GPUI Scene → gpui_macos Metal renderer → CoreText atlas → Metal present
```

Invariants preserved: `Cell` 8 bytes, `FramePool` 4, generation-gated damage, `FontIdentity` family+features+size+metrics, `NamedInteger(PaneId)`.

Potential source cost: `element.rs:141-179, 422-467` clears and reshapes every `shaped_lines` row whenever `CacheAction.needs_redraw` is true, including a one-row `Partial`. The CPU profile does not show shaping as a leading measured stack, so this remains a deferred hypothesis rather than a measured bottleneck. `RowDelta.generation` exists (`delta.rs:24-35`) but `RowBatch` drops it (`cache.rs:45-51`), so row-local invalidation is unavailable. `fill_batch` emits `Cell.content` with widths 1/2 and `PRESENTATION_FLAGS 0x7b8f` stripping `COMBINING`, and `FrameDelta` carries no grapheme side-table payload, so `e + U+0301` never reaches `shape_line`.

### 5.2 Alacritty

| Dimension | Current `7dd7b5b` | v0.10.1 `2844606` | Delta |
|---|---|---|---|
| Backend | `glutin 0.32`/`winit 0.30`, `Renderer` enum `Gles2\|Glsl3` + `RectRenderer` (4 programs), robustness `KHR`, `platform.rs` CGL/WGL/EGL/GLX | single `QuadRenderer`, pre-0.30 glutin, `mio` directly | Adds dual path + robustness + modern surface |
| Atlas/cache | `ATLAS_SIZE 1024` row-packing `Atlas{row_extent,baseline,tallest}`, `TexSubImage2D`, `GlyphCache: HashMap<GlyphKey,Glyph,RandomState>` 4 font keys, `\0` dedup, `LoadGlyph` trait | same 1024, `FnvHasher`, no GLES RGB→RGBA fix | Preserved + GLES fix |
| Shaping | per-char `crossfont` + zerowidth vec replay, `WIDE_CHAR` spacer | identical | identical |
| Batching | `Glsl3 BATCH_MAX 0x1_0000` instanced quads vs `Gles2 vertices 65532`, `add_render_item` flush on `tex_id` change + full, rects per `RectKind` | `QuadRenderer instances 0x1_0000` same flush | Policy unchanged, split by GL version |
| Damage | `TermDamageState {full, lines: Vec<LineDamageBounds>}` + `DamageTracker[FrameDamage;2]` double-buffer, `shape_frame_damage→RenderDamageIterator` overdamage (±1w,±0.5h) + `merge_rects` → `Vec<Rect>` for EGL damage | term damage same but no Display double-buffer nor partial present | Current adds bounded partial present |
| Buffers | VBO `STREAM_DRAW` alloc once + `BufferSubData` per batch, 1024² RGBA atlas | same | unchanged |
| Sync/idle | `DontWait` swap + `FrameTimer` snap to vsync, `has_frame/occluded/requested_redraw` dedup, visual bell reschedule | no `FrameTimer`/`occluded`, redraw every `Wakeup` | Reduces wakeups + occlusion cull |

PTY coalescing: `alacritty_terminal/src/event_loop.rs:14-16` `READ_BUFFER_SIZE 0x10_0000`, `MAX_LOCKED_READ 64 KiB`; `event_loop.rs:145-168` — `try_lock_unfair`, one `Event::Wakeup` per non-sync batch (`sync_bytes_count < processed`). This is the one candidate pattern portable to Mr Crabs (`crates/mr-crabs-pty` pump).

Partial present: current `DamageTracker` feeds Wayland `swap_buffers_with_damage`; benchmark-era v0.10.1 had no display-level partial present. The EGL damage win does not transfer to GPUI/Metal, which clears the drawable every present (`metal_renderer.rs:651-670`).

### 5.3 WezTerm

Source package at `770d8e1a` (read-only inspection, no build). The five extracted candidates all deepen existing `FrameDelta→RenderCache→TerminalElement` seams; none add a renderer. Specific file/symbol citations were retained in the WezTerm report artifact; this report does not re-attribute WezTerm internals beyond that commit pin, to avoid inventing source links. WezTerm's closest analogue to the memo candidate is its bounded `shape_cache` plus `LineToEleShapeCacheKey { shape_hash, composing, shape_generation }` and line quad cache — policy borrowed, renderer not copied.

### 5.4 Warp

Warp's current client is public at `8936686f261e`. On macOS, `warpui` owns a native Metal renderer with separate rectangle, image, and glyph pipeline states, static quad buffers, a 1024×1024 paged glyph atlas, and a texture cache. The cross-platform adapter uses wgpu 30. `warpui_core::Scene` groups rectangles, images, and glyphs into ordered layers. Warp rebuilds the scene each frame and clips by layer; the inspected renderer has no incremental damage model. The terminal model remains separate from the generic scene renderer. A bounded 1024-item PTY-read broadcast channel feeds model updates.

### 5.5 Contrast table

| Concern | Alacritty | WezTerm | Warp | Mr Crabs (this worktree) |
|---|---|---|---|---|
| Renderer ownership | App owns GL (`Renderer` enum) + atlas | App owns GL or WebGPU + shape/quad caches | `warpui` owns Metal or wgpu + scene/atlas | GPUI owns Metal + atlas + scene; app owns CPU descriptors |
| Damage | `TermDamageState` + `DamageTracker` double-buffer → `swap_buffers_with_damage` (Wayland/EGL) | Line-level shape/quad caches with `shape_generation` keys | Full scene rebuild with per-layer clip bounds; no incremental damage | `CompactEngine::Damage Partial/Full` + `FrameDelta` → `RenderCache` rebuild scope; `blanket refresh_windows` defeats GPUI replay |
| Shaping cache | `GlyphCache` per glyph, `BATCH_MAX` instanced quads | Bounded `shape_cache` per line, `LineToEleShapeCacheKey` | Glyph cache keyed by glyph, font, size, scale factor, and subpixel alignment | `shaped_lines: Vec<Vec<Option<ShapedLine>>>` per visible row, full reshaping on any `needs_redraw` |
| PTY batching | `READ_BUFFER_SIZE 1 MiB` + `MAX_LOCKED_READ 64 KiB` → one `Wakeup` | 128 KiB parser buffer + 3 ms coalescing delay | Bounded 1024-item PTY-read broadcast channel | `FEED_SLICE_BYTES 2_560` chunks + `PUMP_CAP 64` coalesced via `dispatch_async_f`/`OutputWake` |
| Present | Full swap on X11/macOS, partial `Vec<Rect>` on Wayland/EGL | WebGPU `Fifo` or OpenGL | Native Metal `presentDrawable`; wgpu surface present | Full drawable clear every present; partial Metal present not supported |
| GPU pressure | Atlas + VBO per batch, `DontWait` vsync snap | Triple-buffered vertices + atlas | Static quad buffers, separate pipelines, paged atlas | Scene sort + instance upload per present; 111 submissions / 12.965 s in this trace |

## 6. Candidate designs and decision

Three frozen designs were ranked by frozen reports. This report is the synthesizer; it does not re-edit candidates, it records the decision.

| Candidate | Status | One-line | Seam | Bound | Why ranked where it is |
|---|---|---|---|---|---|
| **ShapeCache (LineShapeMemo)** | Deferred | Memoize `shape_line` per line with exact `(text, cell_widths 0/1/2, Font, font_size_bits, ShapePolicy)` key; LRU 2,048 entries / 4 MiB per `PaintState` | Private `shape_memo.rs` between `PaintState` and `WindowTextSystem::shape_line` | 2,048 + 4 MiB per pane retained; visible `PreparedRow` working set separate; `Arc<PreparedShape>` pin prevents invalidated paint | **Rank 1 among deferred designs.** The interface is deep and source inspection shows redundant reshaping, but the CPU trace does not establish shaping as a dominant cost. Blocked on both the grapheme payload gap (§7) and a shaping-specific real-surface measurement. |
| **SceneBatch (PrimitiveBatchCache)** | Deferred | Retain CPU `CellQuad`/`TextOp`/`ImagePrimitive` batches per row; rebuild only damaged rows; single `submit` in z-order | Private `primitive_batches.rs` between `RenderCache` and `Window::paint_*` | 8 MiB retained descriptors per `PaintState`; `RenderCache` envelope trimmed to `rows×cols` on `Full` | **Rank 2.** Reduces Mr Crabs CPU preparation and allocation, but GPUI already does typed scene split and pooled Metal instance batching (`scene.rs`, `metal_renderer.rs:57-109, 495-522`). Requires proving redundant with GPUI; cannot reduce draw calls or upload bytes without reducing logical primitive count. |
| **DamagePaint (row-scene replay)** | Deferred, weakest | One `TerminalPaintHandle` per `PaneId` with GPUI `Entity::cached` per-row and per-layer replay | `cache.rs` + `element.rs` + `lib.rs` + `paint_diagnostics.rs` + `ui/workspace.rs` (+ `ui/wake.rs` gated) | Damage bitsets `2×ceil(rows/64)×8 B` (≤16 KiB) + row prepared batches O(visible) | **Rank 3.** Valid CPU damage design, but not partial Metal present: GPUI clears the drawable every present and uploads the complete scene. The primary blocker is blanket `refresh_windows` (`app.rs:1740-1742`, `wake.rs:117-118`) which forces `window.refreshing` and bypasses replay (`view.rs:386-391`). |

### 6.1 Explicit no-second-renderer decision

No candidate adds a renderer, atlas, `MTLDevice`, `wgpu::Device`, vertex buffer, or draw call. GPUI remains the sole Metal owner. The report records a hard revert predicate: if median/p95 dirty-to-present time does not improve by ≥5% beyond noise in **real-surface** A/B with Metal trace proof, while missed-frame count, primitive/Metal upload counts, idle zero, and `Cell 8 B`/`FramePool 4` invariants hold, the candidate reverts in one cut. Private Metal and second-atlas paths were rejected without a prototype.

### 6.2 What each candidate assumes about GPUI (verified, not asserted)

- GPUI already stores typed `Scene` vectors and clears them retaining capacities (`scene.rs:41-69`).
- `Scene::finish` sorts by overlap/texture and `Scene::batches` groups contiguous ranges (`scene.rs:151-190, 269-466`).
- Native Metal pools a default 2 MiB instance buffer and draws `PrimitiveBatch` ranges (`metal_renderer.rs:57-109, 670-735`).
- `Window::paint_glyph`/`paint_image` resolve through GPUI's atlas before inserting sprites (`window.rs:4350-4425, 4582-4678`).
- No public bulk paint insertion; internal `reuse_paint`/scene replay is crate-private (`window.rs:3550-3585`). Only `Entity::cached` replay is public.

## 7. Why no GPU implementation was justified

Four conjunctive reasons, each independently blocking:

1. **GPU occupancy is unproven.** The trace shows short application render-encoder intervals, not GPU execution duty. It does not justify replacing or extending the renderer.
2. **The measured CPU hot path is elsewhere.** CPU samples rank terminal scroll, storage, and ingest above GPUI render and paint. Optimizing paint first is misordered.
3. **The real-surface baseline has no qualifying GPU candidate.** The Metal and Time Profiler traces are captured, but no design has a measured GPU-side cost that can satisfy the ≥5% keep predicate. Headless `RenderCache` microbenchmarks would not fill that evidence gap.
4. **Correctness preconditions.** Two gaps fail before performance:
   - Blanket `refresh_windows` must be replaced by targeted pane-entity notification, or GPUI replay is unreachable (DamagePaint stage-0 gate).
   - Combining marks (`e + U+0301`) are not projected through `FrameDelta→RowBatch→RunBatch` (`cache.rs:288-298`, `PRESENTATION_FLAGS 0x7b8f`). A shape memo must not normalize or synthesize marks; combining screenshots must fail until that projection is fixed.

Until a real GPUI→Metal A/B harness exists and those preconditions are proven, writing any of the three modules would be speculative code that the measurement predicate cannot keep. The investigation therefore stops at this report.

## 8. Next step: CPU profiling target

The next profiling target is the **40 ms scrollback workload** headless feed path, isolated from GPUI. Prioritize in order:

1. `CompactEngine::scroll_up_relative` (38) and `Terminal::ingest_scrolled` (33) — descriptor moves, `damaged_rows`/`recycled_rows 1024`, and `VecDeque<CompactRow>` churn.
2. Scrollback ingest → paged `ScrollbackStorage` cold-cache + LZ4 compression jobs (`storage.rs` `max_queued_jobs`/`max_pending_completions` 16, `try_send` backpressure).
3. `input_ascii_run` (24) — plain-ASCII vs paired-SGR fast-path ordering in `protocol.rs` (`try_feed_paired_sgr_text`).
4. `PaneModel::pump` (19) → `Terminal::feed` 2560 B chunk loop, 60 k style-headroom census, `poll_compression`/`recycled_rows` polling cadence.
5. `FrameDelta` generation/damage → `RenderCache` `Clean same-seq no-op` path and `batch_runs` wide-aware coalescing.

Estimated magnitude ranking for the scrollback `Instant::now`→`memory_metrics` window (~3-5 falsifiable hypotheses): storage job submission/poll completeness, `CompactEngine` scroll `VecDeque` + `Arc<[Cell]>` descriptor handling, style-trigger 60k census, VTE ASCII scan branching, and `FrameDelta` copy/batch. Each hypothesis must name a minimal A/B edit (e.g., `FEED_SLICE_BYTES`, hot-page size, queue depth) and a focused correctness gate before it is attempted. This is not a GPU task.

No GPU implementation is authorized before the baseline harness below is established.

## 9. Trace regeneration commands

All paths are absolute. No workload semantics are changed; re-run produces fresh artifacts consumable by `xctrace export`.

```bash
# 1) GPU baseline — Metal System Trace, real window, 208×54, effects off
#    Produces /tmp/mr-crabs-gpu-baseline.trace (directory) and TOC xml
xctrace record --template "Metal System Trace" \
  --output /tmp/mr-crabs-gpu-baseline.trace \
  --launch -- ~/.cargo-target/release/mr-crabs \
    --shell=/tmp/mr-crabs-gpu-baseline/run-fullscreen.sh \
    --initial-window-size=208x54 --close-on-exit=always \
    --startup-fetch=false --cursor-trail=false --cursor-style-blink=false \
    --text-animation=none --window-padding-x=0 --window-padding-y=0
# Wait for app exit (duration ~13 s)

# TOC (summary + instrument list + launched pid)
xctrace export --input /tmp/mr-crabs-gpu-baseline.trace \
  --toc --output /tmp/mr-crabs-gpu-baseline-toc.xml

# Metal application detail tables
xctrace export --input /tmp/mr-crabs-gpu-baseline.trace \
  --xpath '//trace-toc[1]/run[1]/data[1]/table[13]' \
  --output /tmp/mr-crabs-time-profile.xml 2>/dev/null || true

# If the install exposes named exporters, prefer:
# xcrun xctrace export --input /tmp/mr-crabs-gpu-baseline.trace \
#   --table "Metal Application:Command Buffer Submissions" \
#   --output /tmp/mr-crabs-metal-application-command-buffer-submissions.xml
# xcrun xctrace export --input /tmp/mr-crabs-gpu-baseline.trace \
#   --table "Metal Application:Intervals" \
#   --output /tmp/mr-crabs-metal-application-intervals.xml
# The checked-in artifacts at /tmp/mr-crabs-metal-application-*.xml were produced this way.

# 2) CPU baseline — Time Profiler, same workload
xctrace record --template "Time Profiler" \
  --output /tmp/mr-crabs-time-baseline.trace \
  --time-limit 15s \
  --launch -- ~/.cargo-target/release/mr-crabs \
    --shell=/tmp/mr-crabs-gpu-baseline/run-fullscreen.sh \
    --initial-window-size=208x54 --close-on-exit=always \
    --startup-fetch=false --cursor-trail=false --cursor-style-blink=false \
    --text-animation=none --window-padding-x=0 --window-padding-y=0

xctrace export --input /tmp/mr-crabs-time-baseline.trace \
  --toc --output /tmp/mr-crabs-time-baseline-toc.xml

# Inclusive sample extraction for the ranking in §4.2
# (Time Profiler XML is large; filter by mr-crabs binary UUID)
grep -o 'CompactEngine::scroll_up_relative\|ingest_scrolled\|input_ascii_run\|PaneModel::pump\|GPUI.*render\|Metal' \
  /tmp/mr-crabs-time-profile.xml | sort | uniq -c | sort -rn

# 3) Quick TOC sanity (expected outputs)
ls -lh /tmp/mr-crabs-gpu-baseline.trace /tmp/mr-crabs-time-baseline.trace
head -n 40 /tmp/mr-crabs-gpu-baseline-toc.xml
grep -c '<row>' /tmp/mr-crabs-metal-application-command-buffer-submissions.xml || echo "re-export needed"
```

Expected reproductions: GPU TOC `duration ~12.96 s`, `template-name Metal System Trace`, and a launched-process pid. The command-submission export contains about 115 rows, with about 111 belonging to the launched process. The application-interval export has a median render-encoder interval near 41.667 µs. These counts vary across runs and are not GPU-utilization measurements. The CPU stack ordering (§4.2) is the stable decision input.

## 10. Source citations (primary-source links with commit and file path)

Primary sources only. Links are GitHub `blob/<commit>` with exact paths and lines where available. Branch names are not used. Commits are quoted from the inspected checkout.

### Alacritty — current `7dd7b5b09e06` vs v0.10.1 `2844606`

- `Renderer` enum + `draw_cells` dispatch — [`alacritty/alacritty` `7dd7b5b09e06` `alacritty/src/renderer/mod.rs:40-75`](https://github.com/alacritty/alacritty/blob/7dd7b5b09e06/alacritty/src/renderer/mod.rs#L40-L75)
- Atlas row packing `ATLAS_SIZE 1024` — [`alacritty/src/renderer/text/atlas.rs:12`](https://github.com/alacritty/alacritty/blob/7dd7b5b09e06/alacritty/src/renderer/text/atlas.rs#L12)
- Atlas insert `TexSubImage2D` — [`alacritty/src/renderer/text/atlas.rs:88-100`](https://github.com/alacritty/alacritty/blob/7dd7b5b09e06/alacritty/src/renderer/text/atlas.rs#L88-L100)
- `GlyphCache::get` `HashMap<GlyphKey,Glyph>` — [`alacritty/src/renderer/text/glyph_cache.rs:84-95`](https://github.com/alacritty/alacritty/blob/7dd7b5b09e06/alacritty/src/renderer/text/glyph_cache.rs#L84-L95)
- `add_render_item` flush on `tex_id` — [`alacritty/src/renderer/text/mod.rs:121-130`](https://github.com/alacritty/alacritty/blob/7dd7b5b09e06/alacritty/src/renderer/text/mod.rs#L121-L130)
- `Glsl3 BATCH_MAX 0x1_0000` — [`alacritty/src/renderer/text/glsl3.rs:27`](https://github.com/alacritty/alacritty/blob/7dd7b5b09e06/alacritty/src/renderer/text/glsl3.rs#L27)
- `Gles2 BATCH_MAX 65532` — [`alacritty/src/renderer/text/gles2.rs:225`](https://github.com/alacritty/alacritty/blob/7dd7b5b09e06/alacritty/src/renderer/text/gles2.rs#L225)
- `DamageTracker { frames: [FrameDamage;2] }` — [`alacritty/src/display/damage.rs:38-48`](https://github.com/alacritty/alacritty/blob/7dd7b5b09e06/alacritty/src/display/damage.rs#L38-L48)
- `shape_frame_damage` + `merge_rects` — [`alacritty/src/display/damage.rs:88-102`](https://github.com/alacritty/alacritty/blob/7dd7b5b09e06/alacritty/src/display/damage.rs#L88-L102)
- `overdamage` ±1w/±0.5h — [`alacritty/src/display/damage.rs:190-205`](https://github.com/alacritty/alacritty/blob/7dd7b5b09e06/alacritty/src/display/damage.rs#L190-L205)
- `Display { frame_timer, damage_tracker }` — [`alacritty/src/display/mod.rs:379-383`](https://github.com/alacritty/alacritty/blob/7dd7b5b09e06/alacritty/src/display/mod.rs#L379-L383)
- `request_frame` + `Scheduler(Topic::Frame)` — [`alacritty/src/display/mod.rs:1434-1445`](https://github.com/alacritty/alacritty/blob/7dd7b5b09e06/alacritty/src/display/mod.rs#L1434-L1445)
- `FrameTimer::compute_timeout` vsync snap — [`alacritty/src/display/mod.rs:1556-1585`](https://github.com/alacritty/alacritty/blob/7dd7b5b09e06/alacritty/src/display/mod.rs#L1556-L1585)
- `READ_BUFFER_SIZE 0x10_0000` + `MAX_LOCKED_READ 64 KiB` — [`alacritty_terminal/src/event_loop.rs:14-16`](https://github.com/alacritty/alacritty/blob/7dd7b5b09e06/alacritty_terminal/src/event_loop.rs#L14-L16)
- `pty_read` coalesced one `Wakeup` — [`alacritty_terminal/src/event_loop.rs:145-168`](https://github.com/alacritty/alacritty/blob/7dd7b5b09e06/alacritty_terminal/src/event_loop.rs#L145-L168)
- `TermDamage Full|Partial` + cursor/INSERT — [`alacritty_terminal/src/term/mod.rs:178-183`](https://github.com/alacritty/alacritty/blob/7dd7b5b09e06/alacritty_terminal/src/term/mod.rs#L178-L183)
- `RenderLines` run coalescing — [`alacritty/src/renderer/rects.rs:120-135`](https://github.com/alacritty/alacritty/blob/7dd7b5b09e06/alacritty/src/renderer/rects.rs#L120-L135)
- `swap_damage` + Wayland guard — [`alacritty/src/display/mod.rs:1042-1046`](https://github.com/alacritty/alacritty/blob/7dd7b5b09e06/alacritty/src/display/mod.rs#L1042-L1046)
- `WindowContext::draw` occluded/refresh gate — [`alacritty/src/window_context.rs:366-376`](https://github.com/alacritty/alacritty/blob/7dd7b5b09e06/alacritty/src/window_context.rs#L366-L376)

Benchmark-era `2844606` paths are the same files at that commit; the report distinguishes them in §5.2 rather than duplicating URLs.

### WezTerm — `770d8e1a`

Primary-source package at `770d8e1a` was inspected; file/symbol citations are retained in that report artifact. This report cites only the commit pin and the borrowed policy (`shape_cache` bounded reuse, `LineToEleShapeCacheKey { shape_hash, composing, shape_generation }`) to avoid inventing WezTerm file URLs beyond the artifact.

- WezTerm repo — [`wez/wezterm` `770d8e1a`](https://github.com/wez/wezterm/commit/770d8e1a)

### Warp — `8936686f261e`

- Repository and license split — [`warpdotdev/warp` at `8936686f261e`](https://github.com/warpdotdev/warp/tree/8936686f261e07e4c763b30760c48abe60e98bd2), [`Cargo.toml`](https://github.com/warpdotdev/warp/blob/8936686f261e07e4c763b30760c48abe60e98bd2/Cargo.toml), and [`crates/warpui/Cargo.toml`](https://github.com/warpdotdev/warp/blob/8936686f261e07e4c763b30760c48abe60e98bd2/crates/warpui/Cargo.toml)
- Native Metal resources and pipeline states — [`crates/warpui/src/platform/mac/rendering/metal/renderer.rs`](https://github.com/warpdotdev/warp/blob/8936686f261e07e4c763b30760c48abe60e98bd2/crates/warpui/src/platform/mac/rendering/metal/renderer.rs)
- Paged glyph atlas and cache — [`crates/warpui/src/rendering/glyph_cache.rs`](https://github.com/warpdotdev/warp/blob/8936686f261e07e4c763b30760c48abe60e98bd2/crates/warpui/src/rendering/glyph_cache.rs) and [`crates/warpui/src/rendering/atlas/manager.rs`](https://github.com/warpdotdev/warp/blob/8936686f261e07e4c763b30760c48abe60e98bd2/crates/warpui/src/rendering/atlas/manager.rs)
- Scene and layer data shape — [`crates/warpui_core/src/scene.rs`](https://github.com/warpdotdev/warp/blob/8936686f261e07e4c763b30760c48abe60e98bd2/crates/warpui_core/src/scene.rs)
- PTY read channel bound — [`app/src/terminal/mod.rs`](https://github.com/warpdotdev/warp/blob/8936686f261e07e4c763b30760c48abe60e98bd2/app/src/terminal/mod.rs)

### Mr Crabs — this worktree `0021b66d9068`, GPUI `03e5ad8a`

- Feed chunking `FEED_SLICE_BYTES 2_560` + `FrameDelta` build — [`crates/mr-crabs-terminal/src/lib.rs`](https://github.com/mr-crabs/mr-crabs/blob/0021b66d9068/crates/mr-crabs-terminal/src/lib.rs) (`Terminal::feed`, `build_frame_delta`)
- `FrameDelta/RowDelta/Run/CursorState` — [`crates/mr-crabs-terminal/src/delta.rs:24-35`](https://github.com/mr-crabs/mr-crabs/blob/0021b66d9068/crates/mr-crabs-terminal/src/delta.rs#L24-L35)
- `FramePool` cap 4 — [`crates/mr-crabs-terminal/src/frame_pool.rs`](https://github.com/mr-crabs/mr-crabs/blob/0021b66d9068/crates/mr-crabs-terminal/src/frame_pool.rs)
- `CompactEngine` + `Damage Partial/Full` — [`crates/mr-crabs-terminal/src/compact/engine.rs`](https://github.com/mr-crabs/mr-crabs/blob/0021b66d9068/crates/mr-crabs-terminal/src/compact/engine.rs)
- `ScrollbackStorage` paged hot/cold + bounded 16/16 queues — [`crates/mr-crabs-terminal/src/storage.rs`](https://github.com/mr-crabs/mr-crabs/blob/0021b66d9068/crates/mr-crabs-terminal/src/storage.rs)
- `RenderCache` retained batches — [`crates/mr-crabs-element/src/cache.rs:45-51, 288-298`](https://github.com/mr-crabs/mr-crabs/blob/0021b66d9068/crates/mr-crabs-element/src/cache.rs#L45-L51)
- `TerminalElement` + `PaintState` — [`crates/mr-crabs-element/src/element.rs:141-179, 422-467, 501-534`](https://github.com/mr-crabs/mr-crabs/blob/0021b66d9068/crates/mr-crabs-element/src/element.rs#L141-L179)
- `PaneModel::pump` cap 64 — [`crates/mr-crabs-app/src/model/pane.rs`](https://github.com/mr-crabs/mr-crabs/blob/0021b66d9068/crates/mr-crabs-app/src/model/pane.rs)
- `AppCore` + `FramePool(4)` seam — [`crates/mr-crabs-app/src/lib.rs`](https://github.com/mr-crabs/mr-crabs/blob/0021b66d9068/crates/mr-crabs-app/src/lib.rs)
- GPUI/Metal ownership — [`Cargo.toml`](https://github.com/mr-crabs/mr-crabs/blob/0021b66d9068/Cargo.toml) (`gpui 0.2.2 rev 03e5ad8a`)
- S12 headless gates `blocked_never_pass` — [`verification/results.json:286-291`](https://github.com/mr-crabs/mr-crabs/blob/0021b66d9068/verification/results.json#L286-L291)

### GPUI pinned `03e5ad8a` (zed)

- Full-scene submit — [`zed-industries/zed` `03e5ad8a` `crates/gpui/src/window.rs:3132-3135`](https://github.com/zed-industries/zed/blob/03e5ad8a/crates/gpui/src/window.rs#L3132-L3135)
- Metal instance encoding — [`crates/gpui_macos/src/metal_renderer.rs:490-519, 651-670`](https://github.com/zed-industries/zed/blob/03e5ad8a/crates/gpui_macos/src/metal_renderer.rs#L490-L519)
- `Scene` typed vectors + sort/batches — [`crates/gpui/src/scene.rs:41-69, 151-190, 269-466`](https://github.com/zed-industries/zed/blob/03e5ad8a/crates/gpui/src/scene.rs#L41-L69)
- Scene replay — [`crates/gpui/src/scene.rs:141-148`](https://github.com/zed-industries/zed/blob/03e5ad8a/crates/gpui/src/scene.rs#L141-L148)
- Cached views — [`crates/gpui/src/view.rs:362-476, 386-391`](https://github.com/zed-industries/zed/blob/03e5ad8a/crates/gpui/src/view.rs#L362-L476)
- Atlas — [`crates/gpui_macos/src/metal_atlas.rs:31-57, 98-188`](https://github.com/zed-industries/zed/blob/03e5ad8a/crates/gpui_macos/src/metal_atlas.rs#L31-L57)
- Blanket refresh — [`crates/gpui/src/app.rs:1740-1742`](https://github.com/zed-industries/zed/blob/03e5ad8a/crates/gpui/src/app.rs#L1740-L1742)

## 11. Artifact inventory

- `/tmp/mr-crabs-gpu-baseline.trace` + `/tmp/mr-crabs-gpu-baseline-toc.xml` — authoritative GPU source
- `/tmp/mr-crabs-time-baseline.trace` + `/tmp/mr-crabs-time-profile.xml` + `/tmp/mr-crabs-time-baseline-toc.xml` — authoritative CPU source
- `/tmp/mr-crabs-metal-application-command-buffer-submissions.xml` (115 rows; 111 launched-process rows), `/tmp/mr-crabs-metal-application-intervals.xml` (median render encoder 41.667 µs, cumulative ~5.035 ms), `/tmp/mr-crabs-metal-gpu-execution-points.xml` — detail exports
- `/tmp/mr-crabs-gpu-architecture.tsv` (`2026-08-21T04:34:22.323027Z`) — bounded-prototype gate definition (≥5% real-surface predicate)
- Worktree `0021b66d9068` — code at measurement time

## 12. Risks and edge cases

- `xctrace` sample counts are statistical; the ranking (§4.2) is more stable than absolute counts. Re-run variance of ±15% does not change the decision.
- GPUI replay risk: `Entity::cached` is public at `03e5ad8a` but replay is crate-private. Treat the pinned GPUI commit as part of the experiment contract; a GPUI bump may invalidate the DamagePaint assumption.
- Combining marks: until `FrameDelta` carries grapheme payload, any shaping cache must exclude `U+0301` coverage claims. A combining screenshot that passes without that projection is a false positive.
- `FramePool 4` + `PUMP_CAP 64` are allocation bounds; the primitive-retention budget (8 MiB) and memo budget (4 MiB) are per-pane CPU descriptor caps, not image byte limits. Image bytes remain bounded by `ImageStore`/`TextureCache` (320 MB, 4096 textures).

---

*Every Alacritty, WezTerm, Warp, GPUI, and Mr Crabs implementation claim above comes from the cited source at the quoted commit.*

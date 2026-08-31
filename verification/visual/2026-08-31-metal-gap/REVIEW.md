# Metal feature gap review, 2026-08-31

Read-only investigation. No source changes, no git mutations.

## Question

Did Mr Crabs actually integrate Metal visual features, and what is visually missing against metalterm.dev?

Two separate findings, kept apart deliberately.

App-owned Metal. None. Zero `.metal`, `.wgsl`, or `.msl` files exist outside `vendor/`. No shader source, no shader compilation step, no app-authored GPU pass.

GPUI's transitive Metal backend. Real, and not ours. `Cargo.toml:32-33` pins `gpui` 0.2.2 and `gpui_platform` to zed rev `03e5ad8a630c84c3990055905d0444ea0a519b7f`. That checkout carries the Metal backend at `~/.cargo/git/checkouts/zed-a70e2ad075855582/03e5ad8/crates/gpui_macos/src/metal_renderer.rs`, `metal_atlas.rs`, and `shaders.metal`.

So rendering is hardware-backed through Metal, and Mr Crabs implemented none of it. GPUI usage alone is not evidence of Metal feature work. Everything visual we wrote is Rust calling GPUI paint primitives.

## Evidence

Repository state at HEAD `537d5bed16438f2370e8ef64850a6a21972ab34a`, branch `main`, tracking `origin/main`.

- Zero `.metal`, `.wgsl`, or `.msl` files outside `vendor/`. Verified by find across the tree.
- `crates/mr-crabs-element/src/element.rs:1-7` and `lib.rs:10-14` state the renderer is GPUI with CoreText shaping, no wgpu, no Zig.
- `crates/mr-crabs-effects/` is nine Rust files. `lib.rs:1-24` describes a renderer-independent port of an oracle GLSL contract, not a shader.
- `ANIMATION_PRESETS` is live at `crates/mr-crabs-app/src/settings.rs:361-415`, consumed by `crates/mr-crabs-app/src/bin/mr-crabs.rs:27-30, 67-70, 173-177`.
- Effects wiring is complete at the Rust level. `TerminalElement::with_effects` to `prepare_effects` to `EffectsModel::apply_frame` to `paint_text_reveal` and `paint_trail`, in `element.rs:~530-575, 760-985`.
- `paint_trail` at `element.rs:960-986` calls GPUI `paint_drop_shadows` and `paint_path`. CPU-side geometry, no shader dispatch.
- `CursorTrail` builds a `GradientCache` at `crates/mr-crabs-effects/src/trail.rs:198-220, 320-345`, but the element paint path never consumes the `GradientId`. The gradient work is currently dead weight.
- `verification/research/gpu-terminal-architecture.md:8-30` already decided against a second renderer. `:159-222` records ShapeCache, SceneBatch, and DamagePaint as deferred.

## Prior animation work

- `db58a632b811a92fbfd6c844885ed98b99954b08` is an ancestor of HEAD. 28 files, 5376 insertions. This is the landed animation base.
- `9f548eb7` "polish terminal text reveal rendering" is NOT an ancestor of HEAD. 457 insertions in `element.rs` alone, unmerged.
- `6bc20c0d` "animate startup Rustfetch logo" is NOT an ancestor of HEAD. 592-line `animated_fetch.rs`, unmerged.
- Fifteen `archive/2026-08-23/*` branches exist on origin, including six prototype lines named baseline-tide, ink-feather, kinetic-echo, startup-combined, startup-hard-veil, startup-soft-band. Contents not diffed in this pass.
- `DOGFOOD-CRABS.md:49-74` records cursor trail as NOT VISIBLE under exaggerated settings, 400 frames at 50fps, 70 cursor jumps, zero decaying leftovers observed. Root cause was open at that session.

## Visual capture

`01-current-shell.png` is the live app, PID 83139, window 2994, 1536x1128.

Observed. Flat opaque black background. White text with cyan, green, yellow accents. Crisp monospace, no ligatures, no colour glyphs. No transparency, no blur, no gradient, no bloom, no scanlines. No cursor glow, trail, or halo visible.

`02-apple-menu-open-hotkey-refused.png` is a failed palette attempt. `cua-driver hotkey` refused with `same_pid_keyboard_ambiguity` because PID 83139 owns two on-screen windows, and the fallback opened the Apple menu instead. The command palette was not captured.

Not verified this pass. Transient animation rendering. Typewriter reveal and cursor trail are time-windowed effects; a single still frame of an idle TUI cannot show them. A raw-binary launch with exaggerated settings (`cursor_trail_opacity` 1.0, `cursor_trail_duration_ms` 2000) started a live process but produced no AX-visible window, so it proved nothing and was killed.

## Metalterm feature set, verbatim

Recovered by gzip-decompressing the `__bundler/manifest` blobs embedded in `index.html`. Full text in `metalterm-site-copy.txt`.

Headline. "Every frame goes through Metal 3. A 200 MB log streams past at 120 fps on a 120 Hz display, every command is its own block, and search and the palette are one keystroke away."

Visual and on-screen features.

1. "A caret with physics. Sub-cell interpolation, three shapes and seven named motion styles — Snap, Ease, Spring, Smear, Squash, Phosphor, Arc — or switch it all off in one toggle."
2. "Command blocks. Each command and its output is one tracked region, read from the shell's own OSC 133 markers — with how long it ran and a red gutter mark when it failed."
3. "Command palette. Every setting, theme, tab and recent command behind one fuzzy field. It opens in under a frame and remembers what you actually use."
4. "Find in scrollback. Literal or regex, across your whole scrollback, highlighted on the GPU as you type."
5. "Clusters as one cell. Family emoji, flags and skin tones occupy a single cell and are drawn as the font's own colour glyph."
6. Themes. "Eight colour roles. Eighteen ways to spend them." 18 shipped themes, live-switching.
7. Release v0.1.8 adds "adjustable macOS background blur that follows Reduce Transparency" and "right-aligned execution times and a smoothly animated live command status with a soft fade-in".
8. Release v0.1.7 adds "configurable status-bar transitions: sliding path changes, odometer or drop motion for Git counters, and shared horizontal reflow" and "adjustable 1.0–1.4× line height".
9. Motion toggle. "One toggle in Settings → Motion, and every animation falls back to a 90 ms fade. It also follows the system Reduce Motion setting."

Non-visual features.

10. "Tabs, splits, sessions. Every tab and split keeps its own directory, scrollback and shell — 10 000 lines each."
11. "One plain-text config" at `~/.config/metalterm`.
12. "A cell that stayed 20 bytes." Lazily-allocated side tables for colour emoji, grapheme clusters, per-cell underline colours.
13. "Notifications in band." OSC 9 and OSC 777 posted as system notifications.
14. Shell support. zsh, bash, fish, no rc file edits.

## Speed claims, verbatim

"Two passes. 0.22 ms. Every frame."

- GPU time 0.22 ms.
- CPU time 0.13 ms.
- Frame budget at 120 Hz, 8.33 ms.
- "0.22 ms of GPU time per frame — 2.6% of the 8.33 ms budget"
- "120 fps steady on a 120 Hz display, zero dropped frames, scrolling a filled buffer"
- Methodology stated. "two 30 s runs, M3 Max · 118×34 · continuous scroll of a filled scrollback · 119.7 Hz steady, 0 dropped frames"
- "6 MB installed — one app, zero dependencies"
- Release v0.1.4. "High-volume command output now parses and drains about 11× faster than the previous release's recorded baseline (16–17 → 183–186 MB/s), with 2.5× fewer parser allocations (22,505 → 9,034) and 25% smaller screen and scrollback cells."
- "When nothing changes, the renderer sends no frames at all — not fewer, none — and gives the battery back."

Metalterm's app source is private. Only the site repo `github.com/pioner92/metalterm-site` is public.

## Gap matrix

| Capability | Metalterm | Mr Crabs at HEAD | Evidence |
|---|---|---|---|
| Cursor motion styles | 7 named, sub-cell interpolation, 3 shapes | 1 trail effect, not visually confirmed | `trail.rs`, `DOGFOOD-CRABS.md:49-74` |
| Command blocks via OSC 133 | Yes, with duration and fail gutter | OSC 133 semantic state parsed, no block UI found | `crates/mr-crabs-protocols/src/shell.rs:128-148` |
| Command palette | Fuzzy, opens under a frame | Present. Fuzzy `CommandRegistry::search`, open-state gating | `crates/mr-crabs-app/src/palette.rs:70-130`, `ui/workspace.rs:142,307` |
| Find in scrollback | Literal and regex, GPU highlight | Present at frame level. `SearchNext`/`SearchPrevious` actions, `FrameSearchMatch` projection. Regex and GPU highlight unconfirmed | `crates/mr-crabs-app/src/action.rs:71-72`, `crates/mr-crabs-terminal/src/delta.rs:120,171` |
| Themes | 18 shipped, live switch | Three built-in modes (`auto`, `dark`, `light`); no broader theme registry or picker found | `crates/mr-crabs-config/src/lib.rs:836-841` |
| Background blur / transparency | Adjustable, respects Reduce Transparency | `background-opacity = 1`, no blur | `+show-config`, `01-current-shell.png` |
| Colour emoji and grapheme clusters | One cell, font colour glyph | Not confirmed | `01-current-shell.png` shows none |
| Ligatures | Not claimed | Not observed | `01-current-shell.png` |
| Status bar transitions | Sliding, odometer, drop | Not found | no grep hit |
| Reduce Motion respect | Yes, 90 ms fade fallback | Not found | no grep hit |
| Idle zero-frame rendering | Explicit, "not fewer, none" | Unknown | not measured |
| Published frame timings | 0.22 ms GPU, 0.13 ms CPU, methodology given | None | `gpu-terminal-architecture.md:200-222` |
| Line height control | 1.0–1.4× | `adjust-cell-height = 5%` | `+show-config` |

## Judgment from two independent critics

Both agreed. Stay on GPUI's paint API. Do not add a Mr Crabs-owned Metal pass.

Reasons. GPUI already owns full-scene submission, instance encoding, the atlas, and present. A second render path is a permanent ownership burden. `verification/research/gpu-terminal-architecture.md:8-46, 95-113, 203-250` already measured this and found no GPU-path win. Of the nine competitor capabilities, eight are achievable with GPUI quads, paths, shadows, masks, and glyph runs. Only background blur needs something else, and that something is the platform window material GPUI already exposes, not a terminal post-process.

The 0.22 ms comparison is not directly meaningful. Metalterm owns its renderer end to end; Mr Crabs has GPUI in between. Chasing that number means measuring GPUI, not Mr Crabs.

Architectural objection, both critics independently. The effects crate keeps a shader-resource contract nothing consumes. `GradientId`, `GradientCache`, and the change-texture upload API at `crates/mr-crabs-effects/src/trail.rs:159-255` and `crates/mr-crabs-effects/src/model.rs:361-383` describe resources the GPUI paint path ignores. Meanwhile `collect_reveals` at `model.rs:397-425` scans every tracked cell per animation frame. The crate promises two renderer models and pays for both. Pick the GPUI model and delete the other.

Recommended build order, from the strategy critic.

1. One signature cursor-motion system, not seven copies. The shipped trail is invisible; repair the headline effect before adding more. Proof is a frame-difference heatmap over a deterministic cursor-jump capture.
2. A small art-directed theme and window-material system. Three or four coherent palettes plus adjustable translucency respecting Reduce Transparency. Static composition is read before motion. Proof is a four-image matrix across light and dark desktops with Reduce Transparency on and off.
3. Command-aware visual feedback built on the OSC 133 state that already exists.

Position on parity. Do not chase pixel parity with metalterm. Eighteen themes and seven caret styles are a feature-count race Mr Crabs cannot win from behind. Three excellent themes and one recognizable cursor signature beat an undifferentiated list.

## Cursor trail root cause, unknown

Root cause is not identified. Two hypotheses were tested against source and refuted.

Refuted, absent focus handle. `crates/mr-crabs-app/src/ui/workspace.rs:499` passes a real handle via `.with_focus(self.focus.clone())` on the live app path. `element.rs:865-869` evaluates it and `model.rs:319-324` forwards it to the trail. Test `model.rs:605-629` already covers focus loss and resume.

Refuted, the dead-field warning. `PreparedEffects.focused` and `.now_ms` are consumed by `apply_frame` before the struct is constructed at `element.rs:869-876`. The stored copies are redundant, not a broken chain.

Next decisive test. Use the paint diagnostics sink already wired at `crates/mr-crabs-app/src/ui/workspace.rs:515` and capture a scripted cursor jump. Read `trail_active` at the paint boundary. False means the model never activates, so the fault is upstream in state or config. True means the model activates and the paint draws invisibly, so the fault is in colour, alpha, geometry, or mask at `element.rs:960-986`. That single split decides which half of the chain to open.

## Open items for the next session

1. Cursor trail visibility. Capture paint diagnostics during a scripted cursor jump. Do not guess further from source.
2. Two unmerged animation branches carry roughly 1000 lines. Decide merge or discard before writing new animation code.
3. The unconsumed gradient and change-texture contract in `mr-crabs-effects`. Trace its consumers, then decide whether to remove it or wire it. Deletion is not yet verified safe.
4. No frame-timing measurement exists for Mr Crabs.
5. Command palette capture is still owed. Run it against a single-window instance so `cua-driver hotkey` does not refuse.

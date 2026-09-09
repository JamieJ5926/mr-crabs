## Superseded

Current state is `STATE.md`. This file is history only.

# Handoff: animation and visual parity program

Written 2026-08-31 by the Metal gap investigation session. Read this first, then `verification/visual/2026-08-31-metal-gap/REVIEW.md`.

## Start here

Fresh session, first message:

```
/skill:poteto-mode

Playbook: Orchestrate. Program: Mr Crabs animation and visual parity.
Read HANDOFF-ANIMATION-PROGRAM.md and verification/visual/2026-08-31-metal-gap/REVIEW.md before planning.
```

The `/skill:poteto-mode` prefix is required. Naming the playbook in prose does not load the skill.

## Ground state

Repo `/Users/jamie/Documents/Projects/active/mr-crabs`, HEAD `537d5bed16438f2370e8ef64850a6a21972ab34a` on `main`, tracking `origin/main`. One worktree, no stashes, clean except untracked `.DS_Store`, `.audit/*.tsv`, `DOGFOOD-CRABS.md`, `RESUME.md`, and `verification/visual/`.

Rust plus GPUI, not Zig. `Cargo.toml:32-33` pins gpui 0.2.2 at zed rev `03e5ad8a630c84c3990055905d0444ea0a519b7f`.

Build works. `CARGO_TARGET_DIR=~/.cargo-target cargo build --release --bin mr-crabs` succeeds with one dead-code warning on `PreparedEffects.focused` and `.now_ms`.

Bundle refresh needs rm-then-cp, never cp over a live bundle, or macOS SIGKILLs on stale signature.

## The two findings that shape the program

App-owned Metal does not exist. Zero shader files. Nothing to build on, nothing to fix.

GPUI owns the Metal backend at `crates/gpui_macos/src/metal_renderer.rs` and `shaders.metal` in the pinned checkout. Rendering is hardware-backed. We wrote none of it.

Verdict from two independent critics, both agreeing: stay on GPUI's paint API. Do not add an app-owned Metal pass. `verification/research/gpu-terminal-architecture.md:8-46` already rejected a second renderer after measurement, and that finding still holds. Eight of nine competitor visual capabilities are reachable with GPUI quads, paths, shadows, masks, and glyph runs. Background blur is the exception and belongs to the platform window material GPUI already exposes.

## What is gated and what is not

One thing is gated. Effects integration, meaning any new work that paints through `prepare_effects` to `paint_trail` or `paint_text_reveal`, waits on the trail diagnosis in Lane A. Every other lane is independent and starts immediately in the first wave.

The cursor trail is invisible. `DOGFOOD-CRABS.md:49-74` records zero decaying leftovers across 400 frames at opacity 1.0 and 2000 ms duration. Root cause is unknown.

Two hypotheses are already refuted, do not re-run them. Focus wiring is correct, `crates/mr-crabs-app/src/ui/workspace.rs:499` passes a real handle and `crates/mr-crabs-effects/src/model.rs:605-629` tests focus loss and resume. The dead-code warning is redundant storage, not a broken chain, because `apply_frame` consumes both values at `element.rs:869` before the struct is built.

The decisive test. Use the paint diagnostics sink wired at `crates/mr-crabs-app/src/ui/workspace.rs:515`, capture a scripted cursor jump, read `trail_active` at the paint boundary. False means the model never activates and the fault is upstream in state or config. True means it activates and paints invisibly, so the fault is colour, alpha, geometry, or mask at `crates/mr-crabs-element/src/element.rs:960-986`.

## Lanes

Fourteen lanes with disjoint ownership. Every lane's investigation and design starts in Wave 1. What differs is when each may land code, set by the waves below. Two lanes wait on Lane A's verdict before landing paint work: B and I, both of which write the effects chain A is diagnosing. Lane N is not gated on A; it owns glyph and cluster rendering, which the trail does not touch, and coordinates with I only on `element.rs` write ordering. Lane M waits on Lane D's block state. Lane H's removal waits on B and I. Lane J waits on Lane G's branch report. Everything else lands as soon as it is ready.

Lane A, trail root cause. Run the diagnostics probe above. Deliverable is a named root cause with the diagnostic output attached, not a fix. This is the one lane the effects work depends on.

Lane B, cursor motion system. Design and land one signature motion style, not seven metalterm copies. Sub-cell interpolation already exists, `crates/mr-crabs-effects/src/trail.rs:31-66` uses f64 pixel coordinates. Add a Reduce Motion short-fade fallback. Design work proceeds in the first wave; the paint landing waits for Lane A. Proof is a frame-difference heatmap over a deterministic cursor jump showing vacated cells decay rather than vanish.

Lane C, themes and window material. Three or four art-directed palettes plus adjustable translucency respecting Reduce Transparency. Current state is three modes only, `auto`, `dark`, `light`, at `crates/mr-crabs-config/src/lib.rs:836-841`. Owns `mr-crabs-config` and settings, touches no effects code. Proof is a four-image matrix, light and dark desktop, Reduce Transparency on and off.

Lane D, OSC 133 command block UI. Semantic state already parses at `crates/mr-crabs-protocols/src/shell.rs:128-148`. No block UI exists. Command blocks with duration and a failure gutter mark are layout over existing state, independent of the effects chain.

Lane E, accessibility pass. Reduce Motion and Reduce Transparency handling across the app, plus whatever the system settings require. Nothing found by grep today. Independent surface.

Lane F, frame-timing benchmark. No timing measurement exists for Mr Crabs at all. Build the harness so later visual work has a baseline. Note that measuring against metalterm's 0.22 ms measures GPUI, not us; the number is for our own regression tracking.

Lane G, historical diff and branch decision. Two unmerged animation branches carry roughly 1000 lines. `9f548eb7` is 457 lines in `element.rs`, `6bc20c0d` is a 592-line `animated_fetch.rs`. Neither is an ancestor of HEAD. Fifteen `archive/2026-08-23/*` branches include six prototype lines never diffed. Read-only lane, deliverable is a merge-or-discard recommendation per branch.

Lane H, effects crate hygiene. Trace consumers of the unconsumed `GradientCache` and change-texture API at `trail.rs:159-255` and `model.rs:361-383`. Read-only tracing runs in the first wave. Any removal waits on Lane A and Lane B, since the trail fix may need those descriptors.

Lane I, text reveal and typewriter. The streaming and typewriter presets ship today, `ANIMATION_PRESETS` at `crates/mr-crabs-app/src/settings.rs:361-415`, and `paint_text_reveal` at `element.rs:837-958` draws CPU overlay quads. Nobody has ever proved they render. Verify the shipped presets visually, then judge whether reveal quality holds up. Also read `collect_reveals` at `crates/mr-crabs-effects/src/model.rs:397-425`, which scans every tracked cell per animation frame; bound it to changed cells. Effects lane, so the paint landing waits on Lane A.

Lane J, animated Rustfetch and startup. `startup-animation = rustfetch` and `startup-fetch-command` are live config, confirmed via `+show-config`. Branch `6bc20c0d` carries a 592-line `animated_fetch.rs` that never merged, and six `archive/2026-08-23/prototype-startup-*` branches explored startup presentation. Coordinate with Lane G on the branch verdict, then decide the startup sequence: adopt, rebuild, or drop. Startup is the first thing a user sees, so this is identity work.

Lane K, command palette visual polish. The palette exists and works, `CommandRegistry::search` at `crates/mr-crabs-app/src/palette.rs:70-130`. Metalterm claims theirs opens in under a frame and remembers usage. Ours was never captured on screen. Measure open latency, then polish appearance and add recency ranking if the measurement justifies it. Owns `palette.rs` and its UI, no effects dependency.

Lane L, scrollback search highlighting. `SearchNext` and `SearchPrevious` exist at `crates/mr-crabs-app/src/action.rs:71-72` and `FrameSearchMatch` projects at `crates/mr-crabs-terminal/src/delta.rs:120,171`. Regex support and as-you-type incremental highlighting are unconfirmed. Establish what works today, then add regex and live highlighting. Highlights are cell-aligned quads, the same class as the existing selection overlay at `element.rs:727-728`.

Lane M, command and status transitions. Metalterm ships sliding path changes, odometer motion for Git counters, animated live command status with a soft fade-in, and right-aligned execution times. Nothing equivalent was found by grep. Depends on Lane D's block state for command timing, so brief them as a pair with D owning state and M owning motion.

Lane N, grapheme clusters and colour emoji. Metalterm claims family emoji, flags, and skin tones occupy one cell drawn as the font's own colour glyph, with lazily-allocated side tables keeping the cell at 20 bytes. Our baseline screenshot shows no colour glyphs at all, so current behaviour is unknown. Establish what the CoreText path does with clusters today, then fix cell occupancy and colour glyph rendering. Coordinate with Lane I on `element.rs` glyph code.

## Shared-file contract, set before spawning

`crates/mr-crabs-element/src/element.rs` is the contended file. Five lanes want it. Split by function, one writer per branch, and coordinate through `hub` before touching it. Lane B owns `paint_trail` and the trail half of `prepare_effects`. Lane D owns block layout and gutter painting. Lane I owns `paint_text_reveal`. Lane L owns search-highlight quads. Lane N owns glyph and cluster rendering. Nobody else edits it.

`crates/mr-crabs-effects/` is owned by Lane A while diagnosing, then by Lane I and Lane B jointly for model changes. Lane H must not land a removal until both confirm. `mr-crabs-config` and settings belong to Lane C, and any lane needing a new config key requests it from C rather than adding one. `palette.rs` belongs to Lane K alone. Lane M writes status and chrome UI, never the terminal element.

## Waves

Wave 1, investigation and benchmarks. All fourteen lanes open here. A runs the diagnosis, F builds the timing harness and baseline, G does the branch archaeology, H traces consumers read-only. C, D, E, J, K, L, M, and N establish current behaviour in their own surfaces, since most of them are answering an open question rather than starting from a known state: what the CoreText path does with clusters, what search actually supports, what the palette's open latency is, what accessibility handling exists. B, I, M, and N also produce their designs. No paint code lands in this wave. It ends when the trail root cause is named and a benchmark baseline exists.

Wave 2, isolated implementations. Lanes whose files nobody else touches: C on config and themes, E on accessibility, K on the palette. Lane L's search-index and regex work also lands here, since it lives in `mr-crabs-terminal`; only its highlight quads in `element.rs` defer to Wave 3. These merge as they finish, Gate 1 per merge.

Wave 3, integration on shared files. The `element.rs` lanes land in sequence, never in parallel: B, then I, then N, then D, then L's highlight quads. The order is write-conflict avoidance, not dependency; N could land first if it is ready before B, so long as one writer holds the file at a time. Each rebases on the previous and re-runs Gate 1 and Gate 2. Lane M lands on top of D's block state. Lane H's removal lands once B and I both confirm they do not need the descriptors. Lane J's startup decision lands once G has reported.

Wave 4, visual parity. Every visual lane produces its cua-driver capture set against the integrated head, not against its own branch. This is where parity is actually judged, side by side with `metalterm-site-copy.txt` and the baseline screenshot.

## Convergence gates

Completion is not every lane reporting done. All five of these pass on one final head SHA, or the program is not finished.

Gate 1, locked tests and check. `cargo check --workspace` and the full test suite green on the integrated head, not per branch. `Cargo.lock` unchanged unless a lane's brief authorized a dependency change. The `crates/mr-crabs-effects/tests/` corpus and the S9 oracle hash are the most likely breakage under animation work. A lane that passed alone and fails integrated is not done.

Gate 2, release build. `CARGO_TARGET_DIR=~/.cargo-target cargo build --release` clean. Warnings introduced by the program get fixed, not inherited.

Gate 3, deterministic animation captures. Every animation has a scripted, repeatable capture that shows the effect across frames. Cursor trail needs a frame-difference heatmap proving vacated cells decay. Typewriter and streaming reveal need frame sequences. A still frame of an idle terminal is not evidence, which is the exact failure this session hit. Run against a single-window instance or `cua-driver hotkey` refuses with `same_pid_keyboard_ambiguity`.

Gate 4, accessibility matrices. Reduce Motion on and off for every animation, showing the fade fallback actually replaces motion. Reduce Transparency on and off for every theme and translucency setting. Four images minimum per surface, light and dark desktop.

Gate 5, benchmark non-regression. Lane F's baseline from Wave 1 re-run on the final head. Frame cost that grew without a stated and accepted budget gets reverted, not merged and noted. `collect_reveals` scanning every tracked cell per frame at `crates/mr-crabs-effects/src/model.rs:397-425` is a known cost centre and its fix must show a measured delta.

## Position on the goal

Do not chase pixel parity. Eighteen themes and seven caret styles is a feature-count race from behind. Three excellent themes and one recognizable cursor signature beats an undifferentiated list.

## Orchestration runtime

Verify the runtime first. The playbook references `scripts/orch/orch.ts` for store bookkeeping and that path does not exist in this repo, but the invoked skill or playbook may supply orchestration externally. Check before planning around it. If no orchestration runtime is available, emulate the same lane ownership with the available task tooling: one owner per lane, one writer per file, the shared-file contract above stated in every brief, and lane state tracked in the todo list plus `orchestrate/mr-crabs-visual/` in the agent store.

## Artifacts from this session

`verification/visual/2026-08-31-metal-gap/REVIEW.md` is the full investigation with the gap matrix and both critic verdicts.

`01-current-shell.png` is the live app baseline, PID 83139, 1536x1128.

`02-apple-menu-open-hotkey-refused.png` records a failed palette capture. `cua-driver hotkey` refused with `same_pid_keyboard_ambiguity` because the app owned two on-screen windows.

`metalterm-site-copy.txt` is the competitor's full marketing copy, recovered by gzip-decompressing the bundler manifest in its `index.html`.

## Owed captures

The command palette was never captured. Run it against a single-window instance so `cua-driver hotkey` does not refuse.

No animation was ever captured on video. Typewriter reveal and cursor trail are time-windowed and a still frame of an idle TUI proves nothing. This needs Jamie's hands for a moment, or a single-window instance driven frame by frame.

# Lane G: historical diff and branch decision

Status: PASS (read-only). HEAD `537d5bed16438f2370e8ef64850a6a21972ab34a` (`main`).
Output: `verification/visual/2026-08-31-branch-archaeology/` (untracked).
No git mutation. No `crates/` writes.

Principles that shaped calls: **prove-it-works** (verdicts from `git diff` / blob IDs, not names), **laziness-protocol** (do not merge work already on `main`), **subtract-before-you-add** (identical trees collapse), **never-block-on-the-human** (Lane J gets a startup verdict without a merge).

throughput checkpoint: n/a, read-only investigation

## Flag for Lane J (read this first)

Do **not** merge `feature/animated-rustfetch` (`6bc20c0d`) or any `archive/2026-08-23/prototype-startup-*` branch.

Startup animation on current `main` already includes the `6bc20c0d` idea, then more:

- `crates/mr-crabs-app/src/animated_fetch.rs` exists on HEAD (822 lines). `6bc20c0d` is 592 lines of the same ANSI-to-stdout reel, without PTY capture, centering, dimming, or `sleep 0.5`.
- HEAD default is `sleep 0.5; "$MR_CRABS_BIN" +animated-fetch` (`crates/mr-crabs-config/src/lib.rs:32`). `6bc20c0d` only set `"$MR_CRABS_BIN" +animated-fetch`.
- `should_run_animated_fetch` / `run_animated_fetch_and_exit` already wire in `crates/mr-crabs-app/src/bin/mr-crabs.rs:189`.
- `MR_CRABS_BIN` is already injected in `app_model.rs:394`.
- Landed later: `4c8c89d` restore native animated Rustfetch, `6444710` delay reel until visible. Both are ancestors of HEAD.

The three `prototype-startup-*` branches are **the same tree** as each other (`c1050a5d…`). They are cursor-trail / s9-corpus checkpoints, **not** a window-open startup reel. They do not implement a veil or a band despite the names. Lane J should treat them as discarded trail experiments, not as a startup design menu.

If Lane J needs a visual language for *first paint of rustfetch*, start from HEAD `animated_fetch.rs` plus `fetch_animation.rs` (GIF cell driver already on main). Rebuild from those files. Do not cherry-pick `6bc20c0d`.

GPUI paint API: none of the startup branches add an app-owned Metal pass. `6bc20c0d` writes ANSI to stdout. `fetch_animation.rs` maps GIF frames onto terminal cells. Prototypes use `window.paint_quad` / `fill` / `outline`.

## Verdict table

Lines = insertions+deletions on the unique commit vs its merge-base with HEAD, unless noted. Ancestry is vs HEAD `537d5bed`.

| Branch | SHA | Lines (vs merge-base) | Ancestry | Verdict | Reason |
|---|---|---|---|---|---|
| `origin/feature/animated-rustfetch` | `6bc20c0d` | 620 / 6 (8 files; 592-line `animated_fetch.rs`) | Not ancestor; HEAD not ancestor. MB `57226cc5`. +1 / −40 | **DISCARD (idea already on main)** | Blob `dda00c2b` ≠ HEAD `88506ec1`. HEAD file is a strict superset (PTY capture, ANSI width, centered chunks). Cherry-pick would fight `sleep 0.5` and capture path. Salvage: none required. |
| `origin/archive/2026-08-23/animated-rustfetch` | `238b6eb6` | 580 / 7 (9 files; 550-line `animated_fetch.rs`) | Not ancestor. MB `3d4b76ed`. +1 / −44 | **DISCARD** | Older sibling of `6bc20c0d`. Diff vs `6bc20c0d` on that file: 105 / 147. Same `DEFAULT_STARTUP_FETCH_COMMAND` rewrite, weaker capture. |
| `origin/archive/2026-08-23/recovery-root-animated-fetch` | `9198e02a` | 1622 / 1 (31 files; 463-line `fetch_animation.rs` plus skills) | Not ancestor. MB `959434bf`. +1 / −62 | **DISCARD (GIF driver already on main)** | HEAD `fetch_animation.rs` is the same GIF→cells driver (LoopPolicy, FetchDriver). Diff vs HEAD: 79 / 108. Skills dump is not product. |
| `origin/archive/2026-08-23/prototype-startup-combined` | `b94a34f8` | 1246 / 286 (6 files) | Not ancestor. MB `1a71531f`. +1 / −75 | **DISCARD** | Tree `c1050a5d`. Trail rewrite + s9 corpus, not startup. HEAD `trail.rs` / `element.rs` already diverged (453 / 932 vs this tip). Names lie: no veil/band/startup reel in the hunk. GPUI quads only. |
| `origin/archive/2026-08-23/prototype-startup-hard-veil` | `460497da` | same 1246 / 286 | Not ancestor. MB `1a71531f`. +1 / −75 | **DISCARD** | **Identical tree** to combined (`git diff` empty; `^{tree}` equal). Name is a checkpoint label only. |
| `origin/archive/2026-08-23/prototype-startup-soft-band` | `5e7ec05c` | same 1246 / 286 | Not ancestor. MB `1a71531f`. +1 / −75 | **DISCARD** | **Identical tree** to combined. |
| `origin/archive/2026-08-23/terminal-visual-quality` | `12d73b2e` | 1262 / 286 (7 files) | Not ancestor. MB `1a71531f`. +1 / −75 | **DISCARD** | Same six files as combined plus `.audit/terminal-visual-quality.tsv` (16 lines). Tree `7ca769d3`. |
| `origin/archive/2026-08-23/prototype-baseline-tide` | `1e3e02e4` | 93 / 1 (2 files) | Not ancestor. MB `1a71531f`. +1 / −75 | **DISCARD, salvage idea** | Adds `ScrollArrival` (ease-out cover on live-bottom translate) in `model.rs` + element paint. Not on HEAD (`rg ScrollArrival` empty in current effects). Rebuild against current `EffectsFrame` if Lane B wants a scroll-settle. Do not merge the 75-behind `element.rs`. |
| `origin/archive/2026-08-23/prototype-ink-feather` | `b4750745` | 79 / 20 (`element.rs`) | Not ancestor. MB `1a71531f`. +1 / −75 | **DISCARD, salvage idea** | Extra alpha slices on reveal overlay (`color.a *= slice.alpha`). HEAD already has `reveal_overlay_rects` 2px feather (`element.rs` tests at 2502+). Likely redundant. |
| `origin/archive/2026-08-23/prototype-kinetic-echo` | `2804152b` | 67 / 67 (`trail.rs` + `element.rs`) | Not ancestor. MB `1a71531f`. +1 / −75 | **DISCARD, salvage idea** | `TrailEcho` ×3 lerp from previous cursor rect; paint fill vs outline by `CursorShape`. HEAD trail has no `echoes` field. Rebuild on current `TrailFrame` if a signature trail is wanted. GPUI `paint_quad` only. |
| `origin/feature/terminal-animation-polish` | `9f548eb7` | 457 / 117 (`element.rs` only) | Not ancestor. MB `3d4b76ed`. +1 / −44 | **DISCARD (already landed)** | Blob of `element.rs` at `9f548eb7` **equals** `5f98826` (`57ad7ea2…`), and `5f98826` **is an ancestor of HEAD**. Later HEAD edits the same file (285-line two-way diff). Merging this branch would rewind polish. |
| `origin/fix/issue-19-text-animation-render-dedup` | `21219c32` | 580 / 83 (`key.rs`, `model.rs`, `issue_19_repeated_output.rs`) | Not ancestor. MB `3d4b76ed`. +27 / −44 | **DISCARD (superseded trap)** | Unique blobs equal `62e3e09` (`key.rs` `6cc2b2ba`, `model.rs` `cec0e57c`), and `62e3e09` **is an ancestor of HEAD**. HEAD still has `issue_19_repeated_output.rs` (evolved, 122-line two-way). **Do not merge.** |
| `origin/fix/issue-22-typewriter-latency` | `e78ce80d` | +204 / −47 on top of issue-19 (`schedule.rs` + `issue_22_typewriter_latency.rs`) | Not ancestor. Contains issue-19. MB `3d4b76ed`. +29 / −44 | **DISCARD (superseded trap)** | `e78ce80` is **not** an ancestor of HEAD. Replacement on main: `ac8b62f` / `f0122d5` (`f0122d5` **is** ancestor). Schedule blob `08b385cb` ≠ HEAD `94ea93ff`. No `issue_22_*.rs` on HEAD. Re-port only if a live typewriter backlog is measured; do not merge this stack. |
| `origin/archive/2026-08-23/perf-cpu-scroll-pipeline` | `0021b66d` | 6661 / 559 (47 files) | Not ancestor. MB `e5a4b26b`. +11 / −78 | **DISCARD (out of visual scope; stale perf)** | Storage/compact/phase_runner heap. 78 commits behind. Not an animation branch. |
| `origin/archive/2026-08-23/perf-gpu-rendering` | `c646f73c` | cpu tip + 347-line `verification/research/gpu-terminal-architecture.md` | Not ancestor. MB `e5a4b26b`. +12 / −78 | **DISCARD** | Doc-only delta on the cpu branch. Preferences already cite that research against a second renderer. |
| `origin/archive/2026-08-23/perf-vtebench-scrolling` | `b2f6352d` | gpu minus that doc, plus a reverted 64KiB PTY batch | Not ancestor. MB `e5a4b26b`. +13 / −78 | **DISCARD** | Tip reverts its own PTY coalesce. |
| `origin/archive/2026-08-23/recovery-performance-heap` | `029d2df8` | 6344 / 748 (54 files) | Not ancestor. MB `e5a4b26b`. +1 / −78 | **DISCARD** | Checkpoint of the same perf heap plus PNGs. Not visual-program work. |
| `origin/archive/2026-08-23/structured-chat-cleanup` | `73c643ac` | 272 / 523 (13 files) | Not ancestor. MB `3d4b76ed`. +5 / −44 | **DISCARD (not animation)** | Moves structured chat to host. Out of Lane G visual merge set. |
| `origin/feature/chat-agent-session` | `70a3ac37` | 4 commits; ancestor of structured-chat | Not ancestor. MB `3d4b76ed`. +4 / −44 | **DISCARD (not animation)** | Parent of structured-chat. Same reason. |
| `origin/pr/animation-followup` | `1fd40867` | 0 unique vs HEAD | **SHA is ancestor of HEAD** (−61) | **ALREADY MERGED** | Fast-forward empty. |
| `origin/pr/performance-consolidation` | `fe683b53` | 0 unique | **SHA is ancestor of HEAD** (−64) | **ALREADY MERGED** | |
| `origin/repair/terminal-input-rendering` | `4f62c383` | 0 unique | **SHA is ancestor of HEAD** (−41) | **ALREADY MERGED** | Merge-base of `6bc20c0d`. |

## Startup branches in detail (Lane J)

### `6bc20c0d` `feat: animate startup Rustfetch logo`

Read: `git show 6bc20c0d` on `animated_fetch.rs`, `mr-crabs.rs`, `app_model.rs`, `settings.rs`, `mr-crabs-config`.

What it does: spawn `rustfetch` via `std::process::Command`, parse logo/info columns at `---`, emit 8 HSV-cycled ANSI frames to **this process stdout**, then `_exit`. Gated on `+animated-fetch`. Default PTY startup command becomes `"$MR_CRABS_BIN" +animated-fetch`. Sets `MR_CRABS_BIN` in pane env.

Does it still apply: the **behavior** applies and **already ships**. HEAD `animated_fetch.rs` keeps `FRAME_COUNT=8`, `FRAME_DELAY=80ms`, `hsv_to_rgb`, `frame_bytes`, plus:

- ANSI-aware `visible_text` / `split_at_visible_column`
- `capture_rustfetch_from` via `mr_crabs_pty` (not raw `Command`)
- `centered_animation_chunks` / `dimmed_frame`
- `+rustfetch` alias
- `RUSTFETCH_CAPTURE_MAX_BYTES`

Metal: none. Stdout ANSI only. Compatible with the GPUI-paint rule.

Call: **discard**. Rebuild if a new look is wanted; do not merge.

### `prototype-startup-*`

Read: `git show` of `b94a34f` / `460497d` / `5e7ec05`; `git rev-parse ^{tree}`; empty `git diff` across the three.

What they actually change vs `1a71531f`:

- `trail.rs` 269 lines (echo-style trail tests vs `segment`)
- `element.rs` 108 lines (`TrailEchoPaintStyle` Fill/Outline via `paint_quad`)
- `model.rs` 9 lines of test updates
- `s9-effects.json` +1079 corpus rows

No startup fetch, no window-open veil, no soft band overlay. Three branch names, one tree.

Call: **discard**. If a named “veil/band” look is still desired, it was never snapshotted here.

## Issue-19 / issue-22 trap (verified)

- `git merge-base --is-ancestor 21219c32 HEAD` → no
- `git merge-base --is-ancestor 62e3e09 HEAD` → **yes**
- `21219c32` unique files blob-equal `62e3e09`
- `git merge-base --is-ancestor e78ce80 HEAD` → no
- `git merge-base --is-ancestor f0122d5 HEAD` → **yes**
- `git merge-base --is-ancestor 21219c32 e78ce80` → yes (issue-22 stacks issue-19)

Memory warning confirmed: merging either fix branch would replay a 44-commit-behind fork whose unique code already entered `main` under different SHAs.

## Commands run

```
git rev-parse HEAD
git branch -a
git merge-base --is-ancestor <sha> HEAD   # every listed tip
git merge-base HEAD <sha>
git rev-list --count HEAD..<sha>  and  <sha>..HEAD
git log --oneline -8 <sha>
git log --oneline HEAD -- <paths>
git diff --stat <merge-base> <sha>
git diff --stat HEAD...<sha>
git diff --stat <shaA> <shaB>
git diff <shaA> <shaB>                    # empty for the three startup prototypes
git rev-parse <sha>^{tree}
git rev-parse <sha>:<path>                # blob equality
git show <sha> -- <paths>
git show <sha>:<path> | head / rg
git ls-tree -r --name-only HEAD | rg fetch|issue_
git grep -l -i -E 'metal|shader' <sha> -- '*.rs' '*.metal'
rg on working tree for HEAD defaults
```

No `checkout`, `switch`, `stash`, `merge`, `rebase`, `cherry-pick`, `branch`, `reset`, `clean`.

## Could not evaluate

Nothing required by ACCEPTANCE. Perf/chat branches were scored as out-of-scope discards from diffstat + log only, not line-by-line review of 6k-line storage diffs. That is enough to refuse a visual merge.

## Deviations

Investigation playbook **how** / **why** skills were not opened as separate files. The brief is a merge-or-discard table, not a subsystem walkthrough. Output is the recommendation table the playbook allows for a decision.

## Follow-ups (not this lane)

- Rebuild kinetic-echo or baseline-tide on current `TrailFrame` / `EffectsFrame` if those looks are wanted (ideas only).
- Lane J starts from HEAD fetch files, not archived SHAs.
- Do not open issue-19/22 branches for “the remaining typewriter fix” without a live repro against HEAD `schedule.rs`.

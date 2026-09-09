# Full terminal plan

Plan for implementers. Do not start until Jamie says to execute.

## Context

Mr Crabs is already a real terminal engine with GPUI paint. Local `main` is `f2d5fb2` on top of `d534b5f`. The last stretch stacked animation lanes. The shipping window shows themes, molt, rustfetch, typewriter, streaming, and the command palette. It does not show a cursor trail, a find field, or command-block chrome.

Metalterm is a taste target, not a feature checklist. Their 18 themes and seven carets are a count race. Beauty here means four coherent themes, one visible cursor signature, and a terminal content box the user can restyle. Speed means the CPU scroll and storage path already measured as hot, not a second renderer.

Bugs remain because verification recorded the wrong display. `cua-driver start_recording` captures about 1.2 s of the main screen. Window-local `screencapture -l <CGWindowID>` is the path that actually sees Mr Crabs.

## Scope

Included:

- A live-window verification loop that agents actually follow
- A shipping-surface baseline of what is already visible
- A visible cursor trail
- User control of the terminal content box (cursor shape, plus the font, padding, and theme flags that already exist)
- A production find overlay on the search engine we already have
- Reduce Motion for the trail
- One CPU scroll hot-path pass against the existing baseline

Excluded:

- Tachyonfx, Ratatui in-process, app-owned Metal or shaders
- G-Spot workspace chrome, tabs, panes, product shell
- Eighteen themes, seven caret styles, ligature hunting, kitty or iTerm image protocols
- Command-block gutter UI
- A new verification skill
- Push to origin

## Constraints

- GPUI pin `03e5ad8a630c84c3990055905d0444ea0a519b7f` stays
- G-Spot consumes crates at pinned git revs, not the app
- macOS bundle refresh is rm-then-copy, never in-place
- `element.rs` is shared. One writer per phase
- Git commit, merge, and push still need Jamie's word each time after this plan
- Existing visual skill lives at `.agents/skills/mr-crabs-visual-verification/SKILL.md`. Extend it. Do not create a sibling skill

## Alternatives

1. Keep adding animation presets until it feels like Metalterm. Rejected. That is how the trail stayed invisible and the MP4s recorded Ghostty.
2. Port Metalterm feature-for-feature. Rejected by the 2026-08-31 metal-gap review.
3. Stop stacking effects. Prove the window. Fix the trail. Give the user a content-box control. Add find. Then spend the speed budget on scroll CPU. Chosen.

## Applicable skills

- `skill://mr-crabs-visual-verification` after the phase 1 patch
- `skill://cua-driver` for window drive
- `skill://find-not-invent` before any new script or config key
- `skill://how` before touching an unfamiliar crate
- `skill://unslop` on every prose surface
- `skill://principle-prove-it-works`
- `skill://principle-experience-first`
- `skill://principle-laziness-protocol`

## Phases

1. [Verification harness](phase-1-verification-harness.md)
2. [Shipping baseline](phase-2-shipping-baseline.md)
3. [Cursor trail paint](phase-3-cursor-trail.md)
4. [Cursor shape setting](phase-4-cursor-shape.md)
5. [Content-box TUI](phase-5-content-box-tui.md)
6. [Find overlay](phase-6-find-overlay.md)
7. [Reduce Motion trail](phase-7-reduce-motion.md)
8. [CPU scroll pass](phase-8-cpu-scroll.md)

See [testing.md](testing.md) for the shared gates.

## Verification

Project-level, after each phase that touches crates:

```bash
CARGO_TARGET_DIR=~/.cargo-target cargo test --workspace --offline
CARGO_TARGET_DIR=~/.cargo-target cargo build --release --bin mr-crabs --offline
```

Live window. One process. `screencapture -x -o -S -l <CGWindowID>`. Never `cua-driver start_recording`.

Host `control-ui` and `control-cli` skills do not exist here. Use `cua-driver` plus window-local capture.

## Implementation guidance

- Run the **how** skill on a crate before changing it.
- Run **interrogate** before shipping the trail paint design. Two critics already split model vs paint. Do not reopen the model half without a paint-boundary trace.
- Run **unslop** on every prose file. No `/deslop` on this host. Use unslop.
- Keep a **show-me-your-work** trail in `verification/visual/` for this program.
- After a PR is open, babysit with the Pstack Babysit playbook, not a host babysit skill.
- Skill edits in phase 1 belong to the coordinator seat, not a bake-off worker.

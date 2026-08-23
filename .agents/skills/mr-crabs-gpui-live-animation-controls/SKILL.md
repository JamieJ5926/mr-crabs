---
name: mr-crabs-gpui-live-animation-controls
description: "Continue, debug, or verify the GPUI Mr Crabs process-local palette animation controls and generated CLI help without touching renderer/config contracts or launching the GUI autonomously."
---

# Continue or verify GPUI live animation controls

Repository: `/Users/jamie/Documents/Projects/active/mr-crabs-metal-effects`.

## Invariants

- Preserve `SettingsStore` as the only settings owner: `defaults < file < cli < runtime`.
- Live actions remain payload-free `AppAction` variants and enter the palette only through `CommandRegistry::install_shell_commands`.
- Runtime updates must call `SettingsStore::apply_runtime_value` first, then propagate `settings.current().animation_defaults()` to every current pane and increment model generation once.
- Future panes inherit through the existing pane-construction path; file reload must not clear runtime overrides.
- Do not add production menus, default shortcuts, persistence writes, renderer mutation, another settings cache, or config schema changes.
- Do not touch the pre-existing dirty `crates/mr-crabs-element/src/element.rs`; expected SHA-256 is `4fe8fadee64d51dd879ee143cb9d2d3e1cd781bef4211d1fe27ef90e5beac01a` unless current repo evidence shows later user changes.
- Never launch the GUI autonomously. Jamie owns visual acceptance.

## Current actions

- `SetTextAnimationNone` → `shell.set_text_animation_none` → `Text Animation: None` → `text animation set to none`
- `SetTextAnimationStreaming` → `shell.set_text_animation_streaming` → `Text Animation: Streaming` → `text animation set to streaming`
- `SetTextAnimationTypewriter` → `shell.set_text_animation_typewriter` → `Text Animation: Typewriter` → `text animation set to typewriter`
- `ToggleCursorTrail` → `shell.toggle_cursor_trail` → `Toggle Cursor Trail` → `cursor trail enabled|disabled`

## Hidden-palette keyboard regression

If terminal text still appears but Enter, Backspace, arrows, or Command shortcuts appear dead after `Cmd+Shift+P`, inspect palette state before changing input encoders. The proven failure was:

1. `palette_overlay` had no explicit background/text color, so it was visually transparent.
2. The palette-open branch in `handle_key_event` mutated palette state but did not stop GPUI propagation.
3. Named keys were consumed by the hidden palette while printable AppKit text commits continued into the terminal input sink.

Repair in `crates/mr-crabs-app/src/ui/workspace.rs`:

- Give the overlay explicit opaque, theme-aware background/text/border and selected-row chrome.
- Call `model_cx.stop_propagation()` on every palette-open key-down return, including resolved modified shortcuts.
- Do not use `window.prevent_default()`; GPUI simulated input and real macOS `NSTextInputClient` delivery are controlled by event propagation, not the default-prevented flag.

Regression: `ui::workspace::tests::palette_printable_keys_do_not_leak_to_pty_writer`. It opens a rendered `WindowView`, installs a fake `PaneSession` writer, drives `cmd-shift-p`, printable and named palette keys, Escape, then terminal input. The writer must stay empty while the palette is open and receive printable input after close.

## Verification ladder

Run from the repository root:

```bash
cargo test --locked -p mr-crabs-app palette_printable_keys_do_not_leak_to_pty_writer -- --nocapture
cargo test --locked -p mr-crabs-app ui::workspace::tests
cargo test --locked -p mr-crabs-app action::tests
cargo test --locked -p mr-crabs-app settings::tests
cargo test --locked -p mr-crabs-app palette::tests
cargo test --locked -p mr-crabs-app model::app_model::tests
cargo test --locked -p mr-crabs-app ui::actions::tests
cargo test --locked -p mr-crabs-effects --test s9_corpus
cargo test --locked -p mr-crabs-app
cargo test --locked -p mr-crabs-element
cargo build --locked --release -p mr-crabs-app --bin mr-crabs
cargo run --locked --release -p mr-crabs-app --bin mr-crabs -- --help
```

Verify unknown flags separately and expect exit code 2:

```bash
cargo run --locked -p mr-crabs-app --bin mr-crabs -- --definitely-unknown
```

Expected stderr: `Mr Crabs: invalid config: unknown flag --definitely-unknown`.

Verify help bypasses file I/O:

```bash
cargo run --locked --release -p mr-crabs-app --bin mr-crabs -- --help --config-file /definitely/missing
```

Use targeted `rustfmt --edition 2024 --check` on changed app files. Workspace-wide `cargo fmt --all -- --check` may report pre-existing formatting differences in untouched files.

## Worker worktree guard

A delegated worker may follow the session CWD instead of an absolute target path. After every worker handoff, verify the authoritative file line count/diff directly. If the worker edited `/mr-crabs` instead of `/mr-crabs-metal-effects`, recover the exact added block from its transcript, remove only that worker-authored block from the wrong worktree, then apply it to the approved worktree. Never trust the worker’s claimed file path without repository evidence.

## Manual acceptance handoff

Do not run this unless Jamie explicitly authorizes GUI launch. Give Jamie:

```bash
cd /Users/jamie/Documents/Projects/active/mr-crabs-metal-effects
cargo run --locked --release -p mr-crabs-app --bin mr-crabs
```

Then: open `Cmd+Shift+P`; verify the palette is visibly opaque; exercise None, Streaming, Typewriter using `printf 'alpha\nbeta\ngamma\n'`; toggle cursor trail twice; select Typewriter and open a new tab to verify inheritance. Also recheck Enter, Backspace, arrows, Command-Backspace, and Option-Backspace. Report automated gates as PASS and visual behavior as blocked on user until observed.

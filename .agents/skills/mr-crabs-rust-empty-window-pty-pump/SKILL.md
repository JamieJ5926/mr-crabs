---
name: mr-crabs-rust-empty-window-pty-pump
description: "Fix a live mr-crabs-rust window that shows title plus blinking cursor but no zsh prompt, or prompt without typing/Enter: pump AppModel on render/paint, never park on cx.spawn wake_rx, drain IME on the main thread, send CR for Return."
---

# mr-crabs-rust empty window / no prompt / no typing / no Enter

Repo: `/Users/jamie/Documents/Projects/active/mr-crabs-rust`. Binary: `target/release/mr-crabs`.

## Symptoms and first divergence

| Symptom | First failed boundary | Do not rewrite |
|---|---|---|
| Title + blinking cursor, no prompt | Parked `cx.spawn` + `wake_rx.next().await` never resumes on live macOS GPUI | PTY spawn/reader (`diag_spawn` already reads ~132 bytes) |
| Prompt visible, letters vanish | Printable keys owned by GPUI text input, then queued to another `cx.spawn` IME future | Second key encoder |
| Letters echo, Return does nothing (Cmd+Enter works) | GPUI tags Enter as `key=enter` + `key_char="\n"`; IME-owns-printable drops the key path; IME would write LF | Encoder already emits CR for `Key::Enter` |

Headless proof: `cargo run -p mr-crabs-pty --example diag_spawn` — live zsh, ~132 bytes including `ESC[?2004h`.

## Required live path

1. `wake::pump_output` / `drain_scheduled` on `WindowView::render` and `TerminalElement` `on_paint`. Pump **`AppModel` only**. Nested `AppShell::update` during `sync_windows` panics (`cannot update AppShell while it is already being updated`).
2. Do not park output or IME on `cx.spawn(...).detach()` + `StreamExt::next`. Drain IME with `std::sync::mpsc` `try_recv` on render.
3. Share the window `FocusHandle` with the element (`with_focus`). If the element borrowed that handle, do **not** `set_focus_handle` again (steals named keys off the window handler).
4. `text_input_owns_keystroke` must exclude `enter`/`return`/`tab`/`backspace`. `encode_ime` must ignore lone `\n`/`\r`. `encode_key(Key::Enter)` writes `0x0d`.

Oracle: Kitty/Alacritty/xterm send CR for Return. GPUI macOS maps Return to `key_char = "\n"` (`gpui_macos` events.rs).

## Launch

Do not launch a GUI unless Jamie authorizes it. Then use `skill://mr-crabs-rust-manual-launch`. Bring `Mr Crabs — shell` forward; do not stack duplicate bundles.

## Verify

Prompt visible. Type `echo hello` + Return (no Cmd) → `hello` and a new prompt. Close window → no leftover `mr-crabs`/zsh.

Known-good Enter-fix commit on rust-rewrite: `9c17fc902`.

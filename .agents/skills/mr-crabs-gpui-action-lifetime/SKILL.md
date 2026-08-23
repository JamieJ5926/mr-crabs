---
name: mr-crabs-gpui-action-lifetime
description: Diagnose and fix Mr Crabs GPUI actions that pass tests but no-op live because the AppShell entity loses its last strong owner.
---

# Diagnose live-only GPUI action no-ops

Use when a registered GPUI action or shortcut works in tests but silently does nothing in the running Mr Crabs app.

## 1. Prove dispatch registration first

- Confirm the action struct is present in `crates/mr-crabs-app/src/ui/actions.rs`.
- Confirm `AppShell::register_actions` registers it.
- Confirm the keybinding resolves to the same shell action.
- Do not change modifier normalization if a focused GPUI test already resolves the shortcut.

## 2. Inspect entity ownership

Trace every strong and weak handle after the startup closure returns:

- `Entity<AppShell>` keeps the shell alive.
- `WeakEntity<AppShell>` does not.
- GPUI callbacks that capture only `WeakEntity<AppShell>` silently fail after the last strong owner drops.
- A window view retaining only a weak shell reference does not establish application-lifetime ownership.

## 3. Apply the boring ownership fix

Inside `AppShell::register_actions`, clone the strong `Entity<AppShell>` once per registered callback and capture that clone in `cx.on_action`. Dispatch through `shell.update(...)`, then synchronize windows and refresh exactly as before. Do not add a second action path, alias, global singleton, or leaked allocation.

## 4. Add a production-shaped regression

The test must reproduce startup ownership:

1. Create the model and shell entities.
2. Register actions and bind keys.
3. Drop the caller's strong shell handle.
4. Simulate the real shortcut, such as `cmd-shift-p`.
5. Assert the model's observable state changed, such as `palette.is_open()`.

A test that keeps `shell` in scope cannot catch this bug.

## 5. Verify

Run the focused regression, the full `mr-crabs-app` suite, the relevant element suite, and a release build. For live proof, launch only when authorized, capture before and after the shortcut, and confirm the command palette rather than relying only on changed pixels.

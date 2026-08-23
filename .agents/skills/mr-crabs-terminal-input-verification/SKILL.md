---
name: mr-crabs-terminal-input-verification
description: Build and launch an isolated current Mr Crabs Rust bundle for manual terminal input/cursor verification without colliding with installed apps.
---

# Mr Crabs terminal input verification

Use in `/Users/jamie/Documents/Projects/active/mr-crabs-rust` after changing keyboard, IME, PTY input, or cursor blink behavior.

## Static and behavioral gates

1. Run focused release tests:
   ```sh
   cargo test -p mr-crabs-input release --locked
   ```
2. Run app input tests:
   ```sh
   cargo test -p mr-crabs-app ui::workspace::tests --locked
   ```
3. Run cursor tests:
   ```sh
   cargo test -p mr-crabs-element cursor::tests --locked
   ```
4. Run workspace gates:
   ```sh
   cargo check --workspace --all-targets --locked
   cargo test --workspace --locked
   cargo fmt -p mr-crabs-input -p mr-crabs-element -p mr-crabs-app -- --check
   ```

## Isolated bundle launch

The installed Ghostty-derived Mr Crabs may share `dev.jamie.mr-crabs`, so launching that identifier can select stale `/Applications/Mr Crabs.app`. Build a unique verification bundle instead.

1. Package to a unique directory under `~/Applications`:
   ```sh
   bash package/macos/package.sh "$HOME/Applications/MrCrabsManualVerify-build"
   ```
2. In the generated `Mr Crabs.app/Contents/Info.plist`, change only generated-artifact identity fields:
   - `CFBundleIdentifier` → `dev.jamie.mr-crabs.manual-verify`
   - `CFBundleName` and `CFBundleDisplayName` → `Mr Crabs Manual Verify`
3. Re-sign:
   ```sh
   codesign --force --deep --sign - "$HOME/Applications/MrCrabsManualVerify-build/Mr Crabs.app"
   ```
4. Register without activation:
   ```sh
   /System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$HOME/Applications/MrCrabsManualVerify-build/Mr Crabs.app"
   ```
5. Start signed cua-driver on its normal socket with the Hub, start a window-scoped session, then:
   ```sh
   cua-driver launch_app '{"bundle_id":"dev.jamie.mr-crabs.manual-verify","creates_new_application_instance":true}'
   ```
6. Confirm the returned window title is `Mr Crabs — shell` with `get_window_state`.

## Manual acceptance

Ask the user to test physical keyboard input:
- ordinary text appears once;
- held-key repeat is normal;
- Backspace deletes once;
- Cmd-Backspace deletes to line start;
- Option-Backspace and Ctrl-W retain word-delete behavior;
- typing/cursor movement resets the cursor to visible at the correct endpoint.

Cua Driver synthetic text may fail to reach this GPUI terminal even with foreground delivery. Do not claim live input success from an `effect: unverifiable` response; physical typing is the final live gate.

Leave a manually requested verification app running until the user reports results. Clean the unique bundle only after they are done.

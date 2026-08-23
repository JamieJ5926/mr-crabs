---
name: mr-crabs-rust-manual-launch
description: Launch the exact current Mr Crabs Rust release for visible manual testing without LaunchServices bundle conflicts
---

# Manual launch procedure

Use for `/Users/jamie/Documents/Projects/active/mr-crabs-rust` when the user asks to open the current build for hands-on testing.

1. Read `skill://cua-driver` and its macOS companion. Foregrounding is allowed only when the user explicitly wants a visible/manual-test window.
2. Build or package the current release as requested. Do not create a duplicate bundle ID merely to launch it: LaunchServices may resolve another installed terminal, and duplicate instances can remain alive after their main window disappears.
3. Start the exact binary through Hub so lifecycle is owned and observable:
   - application: `<repo>/target/release/mr-crabs`
   - cwd: repository root
   - PTY enabled
   - non-persistent unless the user explicitly requests persistence
4. Start the signed Cua Driver daemon on its standard socket through Hub if unavailable:
   - `/Applications/CuaDriver.app/Contents/MacOS/cua-driver serve --socket ~/Library/Caches/cua-driver/cua-driver.sock`
5. Run `cua-driver list_windows '{"pid":PID}'`. Select the layer-0 window titled `Mr Crabs — shell`; do not guess a stale window ID.
6. If the user asked to see or manually test it, call `cua-driver bring_to_front '{"pid":PID,"window_id":WINDOW_ID}'`. This is an explicit focus steal; never use it for ordinary background automation.
7. Verify the exact foreground PID using read-only System Events and confirm the terminal window still exists. If the main window vanished, stop that verified PID and relaunch the direct binary once; do not stack duplicate app-bundle instances.
8. Stop only the Cua Driver service. Leave the Hub-owned Mr Crabs process running for the user's manual test.

Report the exact process/window identity. Never claim typing was verified unless physical or successfully delivered live input was observed.

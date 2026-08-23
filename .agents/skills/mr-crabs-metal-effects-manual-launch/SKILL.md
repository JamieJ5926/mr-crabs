---
name: mr-crabs-metal-effects-manual-launch
description: Launch and foreground the exact mr-crabs-metal-effects release for user-authorized manual animation testing without LaunchServices ambiguity.
---

# Manual launch

1. Confirm the user explicitly asked to launch or see the app. Prior authorization does not carry over.
2. Use the current release binary at `/Users/jamie/.cargo-target/release/mr-crabs`; cwd `/Users/jamie/Documents/Projects/active/mr-crabs-metal-effects`.
3. Start it through Hub with PTY enabled and no restart policy. Do not use LaunchServices or `open`.
4. Start the signed Cua Driver service only if needed.
5. Run `cua-driver list_windows '{"pid":<hub-pid>}'`. Select the on-screen layer-0 window titled `Mr Crabs — shell`.
6. Snapshot that exact window. If the user requested visible/manual testing, call `bring_to_front` for that PID/window and snapshot again.
7. Stop only the temporary Cua Driver service. Leave the Hub-owned Mr Crabs process running.
8. Report exact PID, window title, binary path, and worktree. Do not claim keyboard behavior was verified unless the user reports it or real input was observed.

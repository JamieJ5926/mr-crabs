---
name: mr-crabs-live-animation-controls
description: Launch and manually verify Mr Crabs live text-animation modes and cursor-trail controls in the metal-effects worktree.
---

# Manual live-controls test

Repository: `/Users/jamie/Documents/Projects/active/mr-crabs-metal-effects`.

1. Build the exact release when needed:
   ```bash
   cargo build --locked --release -p mr-crabs-app --bin mr-crabs
   ```
2. Launch `/Users/jamie/.cargo-target/release/mr-crabs` through Hub with the repository as cwd, PTY enabled, and non-persistent lifecycle.
3. Enumerate the new PID's windows with Cua Driver; select the layer-0 `Mr Crabs — shell` window.
4. Bring it forward only when Jamie explicitly requests visible manual testing.
5. Controls:
   - Open palette: `Cmd+Shift+P`
   - Search: type command title
   - Navigate: Up/Down
   - Activate: Enter
   - Close: Escape
   - Edit: Backspace
6. Animation commands:
   - `Text Animation: None`
   - `Text Animation: Streaming`
   - `Text Animation: Typewriter`
   - `Toggle Cursor Trail`
7. Test text modes with:
   ```sh
   printf 'alpha\nbeta\ngamma\n'
   ```
8. Test cursor trail by typing text without Enter and moving the cursor with Left/Right before and after toggling.
9. Test inheritance by selecting Typewriter, opening a new tab with `Cmd+T`, and producing a burst there.

Runtime overrides affect existing panes and newly created panes, survive config reloads, and reset when the process restarts. Stop only isolated agent-owned test processes; leave a user-requested manual-test process running until the user is finished.

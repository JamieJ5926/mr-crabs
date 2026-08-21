---
name: mr-crabs-visual-verification
description: "Record Mr Crabs visual changes for sign-off. Use after animation, rendering, palette, input, focus, resize, or other visible behavior changes, and whenever the user asks for visual proof or a screen recording."
---

# Mr Crabs visual verification

Produce evidence the user can review without watching the live app.

## Workflow

1. Build and package the current animations worktree with `package/macos/package.sh /Users/jamie/Applications` and the configured shared Cargo target directory.
2. Launch a new `dev.jamie.mr-crabs` instance through `cua-driver launch_app`. Pass exaggerated CLI settings when subtle behavior needs to be legible.
3. Bring the exact returned PID and window ID forward only when the user has authorized visible verification.
4. Snapshot the window. Pixel-click the terminal canvas. Snapshot again.
5. Type a harmless sentinel command. Snapshot the window and inspect the image to prove the complete command is waiting at the prompt. Press Enter. Snapshot again and inspect the output to prove terminal input works before recording the real scenario.
6. Create a fresh output directory. Start recording with the raw tool and explicit video capture:

```text
cua-driver start_recording '{"output_dir":"<absolute-directory>","record_video":true}'
```

7. Type the real scenario. Snapshot and inspect the command before pressing Enter. Record long enough for the scenario to finish. Snapshot and inspect the final output.
8. Stop recording with `cua-driver stop_recording '{}'`. Require a non-null `last_video_path`, `last_error: null`, and an existing non-empty `recording.mp4`.
9. Review the MP4 with a configured video-capable model. Prefer Gemini 3.7 Flash when that role is available. Otherwise inspect timed window frames. State only behavior visible in the artifact.
10. Return the absolute MP4 path, settings, command, observed result, build revision, and any unverified behavior.

## Scenarios

### Text animation

Record separate Typewriter and Streaming instances. Use a long duration and full intensity. Print at least 40 paced lines so output crosses the bottom edge and exercises primary-screen scrolling. The final frame must show the highest numbered line and a returned prompt.

### Cursor trail

Use full opacity and a long fade. Run visible horizontal and vertical cursor movement. Record enabled and disabled states when the change affects toggling.

### Command palette

Record opening, searching, selecting, closing, and post-close terminal input. Confirm palette input never appears at the shell prompt.

## Acceptance

A visual check passes only when the recording visibly exercises the changed behavior. A video that only shows an idle window fails. Tool success, tests, diagnostics, and final screenshots do not substitute for the recorded transition.

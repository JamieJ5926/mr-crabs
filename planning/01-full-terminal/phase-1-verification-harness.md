# Phase 1. Verification harness

Back to [overview](overview.md).

## Goal

Agents stop recording the wrong display. The existing Mr Crabs visual skill matches the capture path that actually sees the window.

## Changes

`.agents/skills/mr-crabs-visual-verification/SKILL.md` currently requires `cua-driver start_recording`. That path records about 1.2 s of the main display. Patch the skill so it forbids that call and requires window-local `screencapture -x -o -S -l <CGWindowID>` bursts.

Add one rerunnable helper, `verification/visual/harness/window_burst.sh`, that launches one release binary, waits for a single window, and writes numbered PNGs. Lift the working pieces from `verification/visual/2026-08-31-reveal/capture.sh`. Do not add a second skill.

## Data structures

None. The helper takes a CGWindowID and an output directory.

## Verification

Static. Skill text no longer mentions `start_recording` as acceptance. Helper `--help` prints the flags.

Runtime. Run the helper against one `mr-crabs --startup-animation none --startup-fetch=false` window. Confirm PNG bounds match the Mr Crabs window, not 1080x1920 Ghostty. Confirm process liveness after launch.

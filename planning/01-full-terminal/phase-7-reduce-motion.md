# Phase 7. Reduce Motion trail

Back to [overview](overview.md).

## Goal

System Reduce Motion, or `MR_CRABS_REDUCE_MOTION=1`, shows no vacated-cell leftover. The live caret still draws.

## Changes

Depends on phase 3. `AccessibilityPolicy` already exists. Wire it so trail leftovers are skipped when motion is off. Keep user config `cursor-trail` unchanged, same as the existing test name `reduce_motion_disables_cursor_trail_without_changing_user_config`.

Touch the policy consumer and the paint skip. Not a new config key.

## Data structures

Reuse `AccessibilityPolicy.allows_motion()`.

## Verification

Static. That named test stays green.

Runtime. Same vacated-cell geometry as phase 3. Motion off. Vacated box matches neighbor luma on every frame. Live box stays the caret.

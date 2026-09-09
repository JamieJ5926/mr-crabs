# Phase 4. Cursor shape setting

Back to [overview](overview.md).

## Goal

The user can pick block, bar, or underline without sending OSC. That is the “normal to the other” content-box switch.

## Changes

OSC already maps Block, Bar, Underline, HollowBlock in `crates/mr-crabs-terminal/src/protocol.rs`. Rendering already exists in `crates/mr-crabs-element/src/cursor.rs`. Add one config key through `mr-crabs-config` and `AppSettings`. Default stays block.

Do not add seven Metalterm caret styles. Three shapes plus the blink flag we already have.

Request the key from the config owner. Do not invent a parallel settings bag.

## Data structures

`CursorShape = Block | Bar | Underline`. OSC can still override for the session, same as other terminals.

## Verification

Static. Config parse tests for the three names and one unknown-name failure.

Runtime. Three windows, `--cursor-shape block|bar|underline`, `--cursor-style-blink=false`. PNG of the caret geometry differs. Phase 1 helper.

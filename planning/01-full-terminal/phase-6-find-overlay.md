# Phase 6. Find overlay

Back to [overview](overview.md).

## Goal

The user can type a find query in the live window. Matches paint with the highlight quads that already exist.

## Changes

`SearchNext` and `SearchPrevious` exist. History already runs regex. `FrameDelta.search_matches` exists. There is no field.

Add a small overlay in `crates/mr-crabs-app/src/ui/workspace.rs` and a thin query owner next to `AppModel.search_query`. Do not put the field in `element.rs`. Element only paints matches it is given.

Literal first. Regex can stay in the history crate behind an explicit toggle if it fits in these files. If not, literal only in this phase.

## Data structures

`SearchQuery { text: String, pattern: SearchPattern }`. Default `Literal`.

## Verification

Static. Existing `search_match` element tests plus one app test that a non-empty query fills `search_matches`.

Runtime. Open find. Type a token that exists on screen. Highlight quads appear. Escape clears them. Typed characters must not reach the PTY while find is focused.

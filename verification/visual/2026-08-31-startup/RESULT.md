# Lane J: startup sequence

routing: unverified (cli-proxy/dddai.grok-4.6 from workstation; fallback not observed)

## Status

PASS. Decision is **keep** HEAD startup rustfetch. No rebuild, no drop, no historical merge.

## Evidence

HEAD already owns the identity:

- `startup-animation` default `rustfetch`
- `startup-fetch-command` default `sleep 0.5; "$MR_CRABS_BIN" +animated-fetch`
- `+animated-fetch` / `+rustfetch` in `mr-crabs.rs`
- 822-line `animated_fetch.rs` with PTY capture, ANSI split, 8-phase HSV reel, centered fade, prompt restore
- GIF overlay is a separate optional path in `fetch_animation.rs`

Lane G already discarded `6bc20c0d` and the prototype-startup trees. Those files were not read as a merge source.

## Code change

One test in `animated_fetch.rs`: consecutive phases must recolor the same logo glyph with eight distinct SGR sequences. Config knobs unchanged.

## Tests

`CARGO_TARGET_DIR=~/.cargo-target cargo test -p mr-crabs-app --lib animated_fetch`

18 passed, 370 filtered, 1.06s.

## Capture artifacts

Non-GUI proof (P1 GUI not required because the reel is ANSI on the PTY, not a GPUI paint):

- `dump_frames.py` (rerunnable)
- `dump.json` with eight unique SHA-256 hashes
- `frame_00.txt` … `frame_07.txt` and matching `.hex`

GUI window burst was not run. A still of an idle window would not prove the reel, and launching a second Mr Crabs instance risks colliding with the live two-window pid.

## Follow-ups

None required for identity. Optional later: P1 burst of a dedicated single-window launch with default rustfetch, after the live two-window instance is gone.

## Principles applied

Laziness Protocol. Keep HEAD instead of rewriting 822 lines.
Experience First. The existing rustfetch-until-Enter sequence is the first-paint identity.
Prove It Works. Unique hashes plus the new unit test, not a compile-only claim.

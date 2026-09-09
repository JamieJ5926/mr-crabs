# Testing

Back to [overview](overview.md).

## Static every crate phase

```bash
CARGO_TARGET_DIR=~/.cargo-target cargo test --workspace --offline
```

`Cargo.lock` stays still unless a phase brief allows a crate.

## Live window

One Mr Crabs process. Discover `window_id` from `cua-driver list_windows`. Capture with:

```bash
screencapture -x -o -S -l "$WIN_ID" "$OUT/frame.png"
```

Forbidden as acceptance:

- `cua-driver start_recording`
- A 1080x1920 main-display MP4
- A still of an idle window for molt, typewriter, streaming, or trail
- `package.sh` success without a live pid

## Package

After a phase that should ship in the dock app:

```bash
CARGO_TARGET_DIR=~/.cargo-target sh package/macos/package.sh
rm -rf ~/Applications/"Mr Crabs.app" && cp -R dist/"Mr Crabs.app" ~/Applications/
rm -f target/release/mr-crabs && cp ~/.cargo-target/release/mr-crabs target/release/mr-crabs
```

Then launch and confirm the new pid is `.../Applications/Mr Crabs.app/Contents/MacOS/mr-crabs`.

## Host gap

No `control-ui` or `control-cli` skill on this host. `cua-driver` plus window-local capture is the surface skill.

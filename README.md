# Mr Crabs

A native macOS terminal emulator written in Rust.

This is not a Ghostty fork and does not link `libghostty`. It owns its PTY,
VT/grid, input encoding, and GPUI window.

## Status

Early public alpha. What works today:

- login-shell PTY (`zsh`)
- visible prompt
- typed input and Return (`CR`)
- macOS window titled `Mr Crabs — shell`

macOS only. Tabs, splits, and much of a finished product's surface exist in
the model but are not the supported 0.1.0 path.

## Build

Needs Rust 1.85+ on Apple Silicon or Intel Mac.

```bash
cargo build --release --locked -p mr-crabs-app --bin mr-crabs
./target/release/mr-crabs
```

Optional app bundle (does not install into `~/Applications` unless you copy it):

```bash
sh package/macos/package.sh
```

That writes `dist/Mr Crabs.app` (or `$1/Mr Crabs.app`).

## Config

```bash
./target/release/mr-crabs --config-file /path/to/config.json
```

The file is JSON. A minimal example:

```json
{"font_size": 16.0, "theme": "dark"}
```

Reload with `Cmd+Shift+R` or View → Reload Configuration. CLI flags beat the
file. A missing `--config-file` path is a startup error.

## License

MIT for original Mr Crabs code. Vendored Alacritty terminal sources, VTE, GPUI,
fonts, and Ghostty-origin resources keep their own licenses — see
`resources/THIRD_PARTY_NOTICES.txt`.

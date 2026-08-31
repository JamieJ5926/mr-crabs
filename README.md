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

## Animations

In a running Mr Crabs window, `mr-crabs +animation <name>` switches that
window immediately. Other windows keep their current animation.

```bash
mr-crabs +animation typewriter
mr-crabs +animation list
mr-crabs +animation
mr-crabs +animation menu
```

Bare `+animation` and `+animation menu` open the interactive TUI on
`/dev/tty`. The TUI queries the host and previews live overlay state.
Persistent save over PTY OSC is unavailable until a trusted host channel
exists. `+animation list` prints the preset menu and
exits. Named presets: `none` (off), `streaming` (left-to-right reveal,
default), `typewriter` (row-staggered reveal), `cursor-trail` (fading
cursor glow), `all` (typewriter plus trail). Unknown names print the
same menu, write an error to stderr, and exit 2.

`--animation` still launches with a named overlay. Bare `--animation`
and `--animation list` print the same menu:

```bash
./target/release/mr-crabs --animation
./target/release/mr-crabs --animation typewriter
```

`--animation` is shorthand for `--text-animation` and `--cursor-trail`.
A later explicit flag wins. `+animation` does not change that startup
overlay.

`startup-animation` chooses the new-window presentation: `none`,
`rustfetch` (default), or `molt`. `none` skips the presentation.
`rustfetch` stays on screen until you submit Enter; that Enter still
reaches the PTY. `molt` is a short full-terminal dissolve over the live
shell. Legacy `startup-fetch` / `startup-fetch-command` remain compatible.


`mr-crabs +rustfetch` replays rustfetch in the calling window. New panes
prepend the running `mr-crabs` executable directory onto the child PATH
and set `MR_CRABS_BIN`, so `+rustfetch` resolves without changing user
or process environment, wrappers, or dotfiles.


`Cmd+Shift+P` opens the command palette, where users choose animation
actions.

## License

MIT for original Mr Crabs code. Vendored Alacritty terminal sources, VTE, GPUI,
fonts, and Ghostty-origin resources keep their own licenses — see
`resources/THIRD_PARTY_NOTICES.txt`.

# Mr Crabs

A native macOS terminal emulator written in Rust.

This is not a Ghostty fork and does not link `libghostty`. It owns its PTY,
VT/grid, input encoding, and GPUI window.

## Status

macOS only. Early public alpha.

The app boots a login shell, paints a prompt, and sends typed keys plus Return
as CR. The window title is `Mr Crabs — shell`.

`AppAction` has 28 variants. `default_keybindings()` binds 23 of them.
`MenuModel::default_shell()` puts 23 of them on the menu bar. Five stay off
the menus and live in the command palette instead.

Tabs and splits ship in that action set. They are not a model-only 0.1.0
stub.

Default chords:

- `cmd+t` New Tab, `cmd+]` / `cmd+[` next and previous tab
- `cmd+d` split right, `cmd+shift+d` split down
- `cmd+w` close tab (last tab closes the window)
- `cmd+alt+w` close pane
- `cmd+alt+]` / `cmd+alt+[` next and previous pane
- `ctrl+cmd+arrow` move focus between splits
- `cmd+shift+n` new window, `cmd+shift+w` close window, `cmd+q` quit
- `cmd+shift+p` command palette, `ctrl+`` quick terminal
- `cmd+shift+r` reload config, `ctrl+cmd+o` secure input
- `cmd+shift+g` / `cmd+shift+h` search next and previous
- `cmd+shift+j` Chat presentation

Palette-only actions (no default chord) are `CheckForUpdates`, the three
text animation setters, and `ToggleCursorTrail`.

Current repo state lives in `STATE.md`.

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

Refresh a live bundle with rm-then-copy. In-place replacement can SIGKILL the
process on a stale signature.

## Config

```bash
./target/release/mr-crabs --config-file /path/to/config.json
```

The file is JSON. Fields use snake_case names from `AppSettings`. Unknown
fields are ignored. A missing `--config-file` path is a startup error.

```json
{"font_size": 16.0, "theme": "dark"}
```

`dark` canonicalizes to `ink`. `light` canonicalizes to `paper`. Named
themes are `auto`, `ink`, `paper`, `harbor`, and `ember`. `auto` follows
the system appearance.

Reload with `Cmd+Shift+R` or View → Reload Configuration. Layer order is
defaults, then file, then CLI, then runtime. CLI flags beat the file.

Other process flags from `help_text()`:

- `-h`, `--help`
- `--version`, `+version`
- `+show-config`, `--docs`, `--default`
- `--keybindings <JSON>`
- `--animation [list|<name>]` and `--animation=<name>`
- `+animation`, `+animation [menu|list|<name>]`, `+animation=<name>`
- `+fetch`

Every `SettingKey::ALL` entry is also a `--<flag>` (28 keys). Canonical
flags:

| Flag | JSON field |
| --- | --- |
| `--font-family` | `font_family` |
| `--font-size` | `font_size` |
| `--adjust-cell-height` | `line_height_adjust_percent` |
| `--theme` | `theme` |
| `--background-opacity` | `background_opacity` |
| `--background-blur` | `background_blur` |
| `--window-padding-x` | `padding_x` |
| `--window-padding-y` | `padding_y` |
| `--cursor-style-blink` | `cursor_blink` |
| `--scrollback-limit` | `scrollback_lines` |
| `--shell` | `shell` |
| `--working-directory` | `working_directory` |
| `--initial-window-size` | `default_grid` (`COLSxROWS`) |
| `--close-on-exit` | `close_on_exit` (`always`, `clean`, `never`) |
| `--cursor-trail` | `cursor_trail` |
| `--cursor-trail-opacity` | `cursor_trail_opacity` |
| `--cursor-trail-duration` | `cursor_trail_duration_ms` |
| `--text-animation` | `text_animation` (`none`, `streaming`, `typewriter`) |
| `--text-animation-duration` | `text_animation_duration_ms` |
| `--text-animation-intensity` | `text_animation_intensity` |
| `--clipboard-write` | `allow_osc52_write` |
| `--clipboard-read` | `allow_osc52_read` |
| `--startup-fetch` | `startup_fetch` |
| `--startup-animation` | `startup_animation` (`none`, `fetch`, `molt`) |
| `--prompt-presentation` | `prompt_presentation` (`dock`, `inline`) |
| `--startup-art` | `startup_art` (`none`, `native`, `apple`, `file:/absolute/path`) |
| `--fetch-arrangement` | `fetch_arrangement` (`below`, `beside`, `hidden`) |
| `--fetch-gif-path` | `fetch_gif_path` |

Aliases accepted by `SettingKey::from_flag`: `line-height-adjust-percent`,
`padding-x`, `padding-y`, `cursor-blink`, `scrollback-lines`, `command`,
`default-grid`, `text-animation-duration-ms`, `allow-osc52-write`,
`allow-osc52-read`.

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
`/dev/tty`. Persistent save over PTY OSC is blocked. Terminal output must
not authorize host writes. `+animation list` prints the preset menu and
exits. Named presets: `none` (off), `streaming` (left-to-right reveal,
default text animation), `typewriter` (character-staggered reveal),
`cursor-trail` (fading cursor glow), `all` (typewriter plus trail).
Unknown names print the same menu, write an error to stderr, and exit 2.

`--animation` still launches with a named overlay. Bare `--animation`
and `--animation list` print the same menu:

```bash
./target/release/mr-crabs --animation
./target/release/mr-crabs --animation typewriter
```

`--animation` is shorthand for `--text-animation` and `--cursor-trail`.
A later explicit flag wins. `+animation` does not change that startup
overlay.

`startup-animation` chooses the new-window presentation: `none`, `fetch`, or
`molt`. The default is `molt`. `none` skips the presentation. The app
collects system facts itself, composes them with the selected startup art,
and reveals the resulting block with the molt animation.

Molt waits for content before starting its clock. If there is nothing to
reveal, molt is skipped.

`mr-crabs +fetch` replays the composed fetch in the calling window. New panes
prepend the running `mr-crabs` executable directory onto the child PATH and
set `MR_CRABS_BIN`.

`Cmd+Shift+P` opens the command palette, where users choose animation
actions.

## License

MIT for original Mr Crabs code. Vendored Alacritty terminal sources, VTE, GPUI,
fonts, and Ghostty-origin resources keep their own licenses. See
`resources/THIRD_PARTY_NOTICES.txt`.

# AGENTS.md

## What this is

Mr Crabs is a native macOS terminal emulator written in pure Rust: it owns its
PTY, VT/grid engine, input encoding, extended protocols, and GPUI window (no
libghostty, no Zig). The library crates double as a terminal engine consumed by
the downstream `g-spot` project as git dependencies pinned to exact revisions.

## Build / run / check / test

```bash
cargo build --release --locked -p mr-crabs-app --bin mr-crabs   # build app
./target/release/mr-crabs                                       # run
cargo check --workspace --locked                                # check
cargo test --workspace --locked                                 # test
cargo fmt --all -- --check                                      # format (rustfmt.toml)
sh package/macos/package.sh                                     # optional: dist/Mr Crabs.app
```

Consumer embed check (builds a throwaway crate depending on the committed
tree via git+rev, the same way g-spot does):

```bash
sh verification/tools/git_consumer_check.sh
```

## Toolchain

- Rust 1.85+ (edition 2024), macOS only (Apple Silicon or Intel).
- All dependencies are exact-pinned (`=x.y.z`); GPUI comes from a pinned zed
  git revision (`03e5ad8a630c84c3990055905d0444ea0a519b7f`).
- **Vendored VTE:** `vte` is declared in `[workspace.dependencies]` as
  `version = "=0.15.0", path = "vendor/vte"` (a version+path dep, not a
  `[patch]`), so git consumers resolve the vendored copy too.
  `vendor/vte` and `vendor/alacritty-terminal` are `exclude`d from the
  workspace; `vendor/alacritty-terminal` is reference/verification source
  (hash-manifested in `verification/manifests/`), not a build dependency.

## Workspace layout

- `crates/mr-crabs-app` — GPUI product shell; bins `mr-crabs`, `phase-runner`.
- `crates/mr-crabs-terminal` — VT parser/grid terminal engine (uses vendored vte).
- `crates/mr-crabs-element` — GPUI `TerminalElement` rendering FrameDeltas.
- `crates/mr-crabs-pty` — pure-Rust PTY and process lifecycle (macOS).
- `crates/mr-crabs-input` — keyboard/mouse/paste/IME/clipboard encoders.
- `crates/mr-crabs-protocols` — bounded OSC/DCS/APC extended protocols.
- `crates/mr-crabs-graphics` — kitty/iTerm2/Ghostty image protocols + texture cache.
- `crates/mr-crabs-history` — scrollback viewport, search, selection, persistence.
- `crates/mr-crabs-effects` — deterministic headless animation effects.
- `crates/mr-crabs-config` — configuration model (JSON config file).
- `crates/mr-crabs-oracle` — verification oracle support.
- `crates/mr-crabs-bench` — benchmarks.
- `vendor/` — vendored `vte` and `alacritty-terminal` (workspace-excluded).
- `verification/` — parity manifests, corpora, results, and check tools.
- `resources/`, `package/` — app resources and macOS packaging.

## Downstream consumers

g-spot depends on `mr-crabs-element`, `mr-crabs-terminal`, `mr-crabs-pty`,
`mr-crabs-input`, `mr-crabs-config`, and `mr-crabs-history` via
`git = ..., rev = <pinned SHA>`. Any public API change here is invisible to
g-spot until its pins are bumped — breaking changes require a coordinated
downstream pin bump, and `verification/tools/git_consumer_check.sh` verifies
the committed tree still resolves for git consumers.

#!/bin/bash
set -euo pipefail

# Git+rev (not a path dep) so cargo resolves committed tree content,
# never the dirty working tree.
repo="${MR_CRABS_CONSUMER_REPO:-file://$(git rev-parse --show-toplevel)}"
rev="${MR_CRABS_CONSUMER_REV:-$(git rev-parse HEAD)}"

workdir="$(mktemp -d)"
trap 'rm -rf -- "$workdir"' EXIT

echo "checking consumer embed path against ${repo} @ ${rev}"

mkdir -p "$workdir/src"

python3 - "$workdir/Cargo.toml" "$repo" "$rev" <<'PY'
from pathlib import Path
import json
import sys

dest, repo, rev = sys.argv[1], sys.argv[2], sys.argv[3]
repo_lit = json.dumps(repo)
rev_lit = json.dumps(rev)
crates = (
    "mr-crabs-element",
    "mr-crabs-terminal",
    "mr-crabs-pty",
    "mr-crabs-input",
    "mr-crabs-config",
    "mr-crabs-history",
)
deps = "\n".join(
    f"{name} = {{ git = {repo_lit}, rev = {rev_lit} }}" for name in crates
)
Path(dest).write_text(
    "[package]\n"
    'name = "mr-crabs-git-consumer-check"\n'
    'version = "0.0.0"\n'
    'edition = "2024"\n'
    'rust-version = "1.85"\n'
    "publish = false\n"
    "\n"
    "[dependencies]\n"
    f"{deps}\n"
    "gpui = { version = \"0.2.2\", git = \"https://github.com/zed-industries/zed\", "
    'rev = "03e5ad8a630c84c3990055905d0444ea0a519b7f" }\n'
)
PY

cat >"$workdir/src/lib.rs" <<'EOF'
use mr_crabs_element::{CellMetrics, TerminalElement};
use mr_crabs_pty::{CommandBuilder, PtyConfig, PtySize};
use mr_crabs_terminal::{FrameDelta, FramePool, GridSize, Terminal};

pub fn public_embed_path() -> (
    GridSize,
    Terminal,
    FramePool,
    FrameDelta,
    CellMetrics,
    TerminalElement,
    PtySize,
    PtyConfig,
) {
    let size = GridSize::new(80, 24);
    let mut terminal = Terminal::new(size).expect("GridSize is valid");
    let mut pool = FramePool::new(4);
    terminal.feed(b"git-consumer-check\n").expect("feed");
    let frame = terminal.build_frame_delta(&mut pool);
    let metrics = CellMetrics::new(8.0, 16.0).expect("positive finite metrics");
    let element = TerminalElement::new(frame.clone(), metrics);
    let pty_size = PtySize::new(80, 24, 8, 16).expect("nonzero pty grid");
    let pty_config = PtyConfig::new(CommandBuilder::new("/bin/sh"), pty_size);
    (
        size,
        terminal,
        pool,
        frame,
        metrics,
        element,
        pty_size,
        pty_config,
    )
}
EOF

CARGO_TARGET_DIR="$workdir/target" cargo check --manifest-path "$workdir/Cargo.toml"

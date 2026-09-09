#!/bin/bash
# One command for Gate 5 Wave 1 timings. Does not mutate the repo workspace
# manifest or lockfile. Builds an out-of-tree crate in a temp dir.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../../.." && pwd)"
SHA="$(git -C "$ROOT" rev-parse HEAD)"
OUT_DIR="$(cd "$(dirname "$0")" && pwd)"
RUN_ID="${1:-1}"
WORKDIR="$(mktemp -d "${TMPDIR:-/tmp}/mr-crabs-f-timing.XXXXXX")"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$HOME/.cargo-target}"

cleanup() { rm -rf "$WORKDIR"; }
trap cleanup EXIT

mkdir -p "$WORKDIR/src"
cp "$OUT_DIR/measure.rs" "$WORKDIR/src/main.rs"
cat > "$WORKDIR/Cargo.toml" <<EOF
[package]
name = "mr-crabs-f-timing"
version = "0.0.0"
edition = "2024"
publish = false

[dependencies]
mr-crabs-config = { path = "$ROOT/crates/mr-crabs-config" }
mr-crabs-effects = { path = "$ROOT/crates/mr-crabs-effects" }
mr-crabs-terminal = { path = "$ROOT/crates/mr-crabs-terminal" }
EOF

echo "workdir=$WORKDIR sha=$SHA run=$RUN_ID CARGO_TARGET_DIR=$CARGO_TARGET_DIR"
MR_CRABS_SHA="$SHA" cargo run --release --manifest-path "$WORKDIR/Cargo.toml" --quiet -- "$RUN_ID" \
  | tee "$OUT_DIR/run-${RUN_ID}.txt"

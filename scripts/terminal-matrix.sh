#!/usr/bin/env bash
# usage: bash scripts/terminal-matrix.sh [report-dir]
# One command: build mr-crabs, launch via control-mr-crabs/launch-isolated.sh,
# run capability-probe.sh, save frames under /tmp/overnight-crabs/PotMatrix/.
# Requires a live emulator (skill://control-mr-crabs). TERM=dumb is not a substitute.
# Ghostty is optional if `ghostty` is on PATH. VM column is SKIP unless a guest is proven.
# Does not launch when MATRIX_NO_LAUNCH=1 (syntax/dry path for concurrent probe slots).
set -eu

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
REPORT_DIR="${1:-/tmp/overnight-crabs/PotMatrix}"
EVIDENCE="${REPORT_DIR}"
ISO_ROOT="/tmp/crabs-matrix"
LAUNCH_HELPER="${HOME}/.omp/agent/skills/control-mr-crabs/launch-isolated.sh"
PROBE="${ROOT}/scripts/fixtures/capability-probe.sh"
TARGET_DIR="${CARGO_TARGET_DIR:-${ROOT}/target-matrix}"
BINARY="${TARGET_DIR}/debug/mr-crabs"

mkdir -p "$EVIDENCE" "$EVIDENCE/crabs" "$EVIDENCE/ghostty" "$EVIDENCE/vm"

if [[ ! -x "$LAUNCH_HELPER" ]]; then
  printf 'missing launch helper: %s (skill://control-mr-crabs required)\n' "$LAUNCH_HELPER" >&2
  exit 1
fi
if [[ ! -x "$PROBE" ]]; then
  printf 'missing fixture: %s\n' "$PROBE" >&2
  exit 1
fi

if [[ "${TERM-}" == "dumb" && "${MATRIX_ALLOW_DUMB-}" != "1" ]]; then
  printf 'TERM=dumb is not a live emulator; refuse. Use skill://control-mr-crabs.\n' >&2
  exit 1
fi

if [[ "${MATRIX_NO_LAUNCH:-}" == "1" ]]; then
  printf 'MATRIX_NO_LAUNCH=1: skipping build/launch (probe slot owned elsewhere)\n'
  printf 'usage: bash scripts/terminal-matrix.sh [report-dir]\n'
  exit 0
fi

export CARGO_TARGET_DIR="$TARGET_DIR"
# Isolated HOME is applied only inside launch-isolated.sh; never export HOME here.
(
  unset HERDR_ENV HERDR_PANE_ID HERDR_WORKSPACE_ID HERDR_SOCKET_PATH HERDR_TAB_ID HERDR_SESSION_ID || true
  cargo build -p mr-crabs-app --bin mr-crabs
)

if [[ ! -x "$BINARY" ]]; then
  printf 'build produced no binary at %s\n' "$BINARY" >&2
  exit 1
fi

sha256sum "$BINARY" | tee "$EVIDENCE/crabs/binary-sha256.txt"
git -C "$ROOT" rev-parse --short HEAD >> "$EVIDENCE/crabs/binary-sha256.txt"

printf 'hub-send required before launch: Crabs-2.PotMatrix (probe slot, at most two live GUI probes)\n' | tee "$EVIDENCE/crabs/slot-note.txt"

"$LAUNCH_HELPER" "$BINARY" "$ISO_ROOT" "$EVIDENCE/app.log" "$EVIDENCE/app.pid"

if [[ ! -s "$EVIDENCE/app.pid" ]]; then
  printf 'empty pid file after launch\n' >&2
  exit 1
fi

cp "$PROBE" "$EVIDENCE/crabs/capability-probe.sh"
printf 'probe fixture copied; live lanes type it into the emulator and capture shots under %s/crabs/\n' "$EVIDENCE" | tee "$EVIDENCE/crabs/frames-note.txt"

if command -v ghostty >/dev/null 2>&1; then
  printf 'ghostty present: %s\n' "$(command -v ghostty)" | tee "$EVIDENCE/ghostty/present.txt"
  printf 'optional ghostty path: copy frames under %s/ghostty/ after a live Ghostty run\n' "$EVIDENCE" >> "$EVIDENCE/ghostty/present.txt"
else
  printf 'ghostty SKIP: binary not on PATH\n' | tee "$EVIDENCE/ghostty/SKIP.txt"
fi

if [[ -n "${MATRIX_VM_GUEST_PROVEN:-}" ]]; then
  printf 'VM guest proven by caller; frames under %s/vm/\n' "$EVIDENCE" | tee "$EVIDENCE/vm/note.txt"
else
  printf 'VM SKIP: no proven guest this run (set MATRIX_VM_GUEST_PROVEN=1 when guest is proven)\n' | tee "$EVIDENCE/vm/SKIP.txt"
fi

printf 'usage: bash scripts/terminal-matrix.sh [report-dir]\n'
printf 'launched pid=%s log=%s\n' "$(cat "$EVIDENCE/app.pid")" "$EVIDENCE/app.log"

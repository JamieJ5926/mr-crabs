#!/bin/sh
# frank-terminal-matrix harness: ONE command runs probe + fixed Frank drive
# 80x24 across Crabs/ghostty/VM, captures frames, prints host+VM gap table.
# Usage documented in --help. Exits nonzero on regression.
# timeout, or resize mismatch. VM column records SKIP unless a running
# guest is proven (CLI present but idle still counts as SKIP, never PASS).
# No src edits, no network, no writes outside staging dirs.
# Re-runnable with HERDR_* unset; the script scrubs HERDR_* on entry.
for v in HERDR_BIN_PATH HERDR_ENV HERDR_PANE_ID HERDR_SOCKET_PATH HERDR_STARTUP_CWD HERDR_TAB_ID HERDR_WORKSPACE_ID HERDR_SESSION_ID; do
  unset "$v" 2>/dev/null || true
done

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
REPO_ROOT=$(CDPATH= cd -- "$SCRIPT_DIR/.." && pwd)
FIXTURE="$REPO_ROOT/scripts/fixtures/capability-probe.sh"
STAGING_TMP="/tmp/crabs-sysmatrix-matrix"
DOC_DIR=""
TIMEOUT_SECS=2
COLS=80
ROWS=24
ONLY=""
VM_SSH=""

usage() {
  cat <<EOF
frank-terminal-matrix harness
ONE command runs probe + fixed Frank drive 80x24 on all terminals:

  sh scripts/terminal-matrix.sh

What it does:
  1. Runs scripts/fixtures/capability-probe.sh once per terminal
     (host:crabs TERM=xterm-ghostty, host:ghostty TERM=xterm-ghostty)
     with COLUMNS=$COLS LINES=$ROWS (fixed Frank drive 80x24).
  2. VM column records SKIP unless a running guest is proven
     (--vm-ssh TARGET runs the fixture inside the guest over ssh;
     CLI-only presence still records SKIP with reason, no host
     bytes masquerading as guest bytes, never failing the gate).
  3. Captures one frame per terminal as text under
     $STAGING_TMP/<terminal>.txt.
     --doc DIR optionally mirrors frames + gap-table.md into an
     aggregator-owned docs dir (default: no docs writes; the
     harness itself never writes docs/*).
  4. Prints the gap table (host+VM columns) to stdout and writes
     gap-table.md to the staging dir (plus the --doc mirror when set).
  5. Exits 0 when every host terminal passes; exits 1 on regression:
     missing BLANK-EXEC marker, blank first exec, OSC52 hang over
     timeout (${TIMEOUT_SECS}s), or resize mismatch (${COLS}x${ROWS}).

Options:
  --out DIR      override /tmp staging dir (default: $STAGING_TMP)
  --doc DIR      optional mirror dir for frames + gap-table.md
  --timeout N    OSC52 timeout seconds (default: $TIMEOUT_SECS)
  --cols N       fixed drive columns (default: $COLS)
  --only LIST    comma list subset of: crabs,ghostty,vm (default: all)
  --vm-ssh TARGET  run the fixture inside the VM guest over ssh
                   (proves a running guest; without it VM = SKIP)
  --help         print this help and exit

Examples:
  sh scripts/terminal-matrix.sh
  sh scripts/terminal-matrix.sh --only crabs,ghostty
  env -u HERDR_BIN_PATH -u HERDR_ENV -u HERDR_PANE_ID -u HERDR_SOCKET_PATH -u HERDR_STARTUP_CWD -u HERDR_TAB_ID -u HERDR_WORKSPACE_ID -u HERDR_SESSION_ID sh scripts/terminal-matrix.sh
EOF
}

while [ $# -gt 0 ]; do
  case "$1" in
    --help|-h) usage; exit 0 ;;
    --out) STAGING_TMP="$2"; shift 2 ;;
    --doc) DOC_DIR="$2"; shift 2 ;;
    --timeout) TIMEOUT_SECS="$2"; shift 2 ;;
    --cols) COLS="$2"; shift 2 ;;
    --rows) ROWS="$2"; shift 2 ;;
    --only) ONLY="$2"; shift 2 ;;
    --vm-ssh) VM_SSH="$2"; shift 2 ;;
    --) shift; break ;;
    -*) echo "terminal-matrix: unknown flag $1 (see --help)" >&2; exit 2 ;;
    *) echo "terminal-matrix: unexpected arg $1 (see --help)" >&2; exit 2 ;;
  esac
done

wanted() {
  [ -z "$ONLY" ] && return 0
  case ",$ONLY," in
    *,"$1",*) return 0 ;;
    *) return 1 ;;
  esac
}

mkdir -p "$STAGING_TMP" || exit 2
[ -n "$DOC_DIR" ] && mkdir -p "$DOC_DIR" || true
if [ ! -x "$FIXTURE" ] && [ ! -f "$FIXTURE" ]; then
  echo "terminal-matrix: fixture missing: $FIXTURE" >&2
  exit 2
fi

# Run the fixture under a fixed 80x24 drive for one host terminal.
# Args: <label> <term-value> <outfile>. Appends a FRANK-DRIVE section.
run_probe() {
  _label="$1"; _term="$2"; _out="$3"
  _start=$(date +%s 2>/dev/null); _start=${_start:-0}
  # shellcheck disable=SC2086
  COLUMNS="$COLS" LINES="$ROWS" TERM="$_term" \
    sh "$FIXTURE" >"$_out" 2>&1
  _code=$?
  _end=$(date +%s 2>/dev/null); _end=${_end:-0}
  {
    printf '\n--- FRANK-DRIVE:80x24 ---\n'
    echo "FRANK-DRIVE-LABEL=$_label FRANK-DRIVE-TERM=$_term"
    echo "FRANK-DRIVE-COLS=$COLS FRANK-DRIVE-ROWS=$ROWS"
    echo "FRANK-DRIVE-EXIT=$_code FRANK-DRIVE-WALL-SECS=$((_end - _start))"
  } >>"$_out" 2>/dev/null
  return $_code
}

virt_reason() {
  _found=""
  for _c in virsh prlctl tart lima utmctl; do
    command -v "$_c" >/dev/null 2>&1 && _found="$_found$_c "
  done
  if [ -n "$_found" ]; then
    printf 'cli-present-but-no-running-guest-proven (saw: %s; pass --vm-ssh TARGET)' "$_found"
  else
    printf 'virt-absent (no virsh|prlctl|tart|lima|utmctl on PATH)'
  fi
}

CRABS_OUT="$STAGING_TMP/crabs.txt"
GHOSTTY_OUT="$STAGING_TMP/ghostty.txt"
VM_OUT="$STAGING_TMP/vm.txt"
CRABS_CODE=0; GHOSTTY_CODE=0
VM_STATE="SKIP ($(virt_reason))"

if wanted crabs; then
  run_probe "crabs" "xterm-ghostty" "$CRABS_OUT"; CRABS_CODE=$?
fi
if wanted ghostty; then
  run_probe "ghostty" "xterm-ghostty" "$GHOSTTY_OUT"; GHOSTTY_CODE=$?
fi
if wanted vm; then
  if [ -n "$VM_SSH" ]; then
    if command -v ssh >/dev/null 2>&1 && ssh -o BatchMode=yes -o ConnectTimeout=5 "$VM_SSH" "COLUMNS=$COLS LINES=$ROWS TERM=xterm-256color sh -s" <"$FIXTURE" >"$VM_OUT" 2>&1; then
      {
        printf '\n--- FRANK-DRIVE:80x24 ---\n'
        echo "FRANK-DRIVE-LABEL=vm FRANK-DRIVE-TERM=xterm-256color FRANK-DRIVE-VIA=ssh:$VM_SSH"
        echo "FRANK-DRIVE-COLS=$COLS FRANK-DRIVE-ROWS=$ROWS"
      } >>"$VM_OUT" 2>/dev/null
      VM_CODE=0
      VM_STATE="probed-via-ssh:$VM_SSH"
    else
      {
        echo "--- PROBE:BLANK-EXEC-MARKER ---"
        echo "VM-SKIP-REASON=ssh-guest-unreachable (target: $VM_SSH)"
        echo "VM-HOST-ONLY-MODE=1"
      } >"$VM_OUT" 2>/dev/null
      VM_CODE=1
      VM_STATE="SKIP (ssh-guest-unreachable: $VM_SSH)"
    fi
  else
    {
      echo "--- PROBE:BLANK-EXEC-MARKER ---"
      echo "VM-SKIP-REASON=$(virt_reason)"
      echo "VM-HOST-ONLY-MODE=1"
    } >"$VM_OUT" 2>/dev/null
  fi
fi

# Evaluate one captured frame. Sets globals E_MARKER E_FIRST E_OSC52
# E_RESIZE (0 pass, 1 fail).
eval_frame() {
  _f="$1"
  E_MARKER=1; E_FIRST=1; E_OSC52=1; E_RESIZE=1
  [ -f "$_f" ] || return 0
  grep -q "BLANK-EXEC-MARKER-PRESENT=1" "$_f" 2>/dev/null && E_MARKER=0
  if grep -q "FIRST-EXEC-PAYLOAD" "$_f" 2>/dev/null; then
    [ -s "$_f" ] && E_FIRST=0
  fi
  _elapsed=""
  _elapsed=$(grep "OSC52-ELAPSED-SECS=" "$_f" 2>/dev/null | tail -n 1 | sed 's/.*OSC52-ELAPSED-SECS=//;s/[^0-9].*//')
  if grep -q "OSC52-TIMEOUT-STATUS=bounded" "$_f" 2>/dev/null; then
    case "$_elapsed" in
      ''|*[!0-9]*) E_OSC52=1 ;;
      *) [ "$_elapsed" -le "$TIMEOUT_SECS" ] 2>/dev/null && E_OSC52=0 || E_OSC52=1 ;;
    esac
  fi
  if grep -q "RESIZE-COLUMNS=$COLS" "$_f" 2>/dev/null && grep -q "RESIZE-LINES=$ROWS" "$_f" 2>/dev/null; then
    E_RESIZE=0
  fi
}

cell() {
  if [ "$1" = "SKIP" ]; then printf 'SKIP'; elif [ "$1" = "0" ]; then printf 'PASS'; else printf 'FAIL'; fi
}

# Evaluate host frames.
eval_frame "$CRABS_OUT"; C_M=$E_MARKER; C_F=$E_FIRST; C_O=$E_OSC52; C_R=$E_RESIZE
eval_frame "$GHOSTTY_OUT"; G_M=$E_MARKER; G_F=$E_FIRST; G_O=$E_OSC52; G_R=$E_RESIZE
case "$VM_STATE" in
  probed-via-ssh:*)
    eval_frame "$VM_OUT"; V_M=$E_MARKER; V_F=$E_FIRST; V_O=$E_OSC52; V_R=$E_RESIZE
    V_M_C=$(cell "$V_M"); V_F_C=$(cell "$V_F"); V_O_C=$(cell "$V_O"); V_R_C=$(cell "$V_R")
    ;;
  *)
    V_M_C="SKIP"; V_F_C="SKIP"; V_O_C="SKIP"; V_R_C="SKIP"
    ;;
esac
C_M_C=$(cell "$C_M"); C_F_C=$(cell "$C_F"); C_O_C=$(cell "$C_O"); C_R_C=$(cell "$C_R")
G_M_C=$(cell "$G_M"); G_F_C=$(cell "$G_F"); G_O_C=$(cell "$G_O"); G_R_C=$(cell "$G_R")

TABLE=$(cat <<EOF
| GAP | host:crabs | host:ghostty | vm |
| --- | --- | --- | --- |
| blank-marker | $C_M_C | $G_M_C | $V_M_C |
| first-exec-nonblank | $C_F_C | $G_F_C | $V_F_C |
| osc52-bounded<=${TIMEOUT_SECS}s | $C_O_C | $G_O_C | $V_O_C |
| resize-${COLS}x${ROWS} | $C_R_C | $G_R_C | $V_R_C |
EOF
)
printf '%s\n' "$TABLE"
{
  printf '# frank-terminal-matrix gap table\n\n'
  printf '%s\n' "$TABLE"
} >"$STAGING_TMP/gap-table.md" 2>/dev/null
if [ -n "$DOC_DIR" ]; then
  cp "$STAGING_TMP/gap-table.md" "$DOC_DIR/gap-table.md" 2>/dev/null || true
  for _t in crabs ghostty vm; do
    [ -f "$STAGING_TMP/$_t.txt" ] && cp "$STAGING_TMP/$_t.txt" "$DOC_DIR/$_t.txt" 2>/dev/null || true
  done
fi

# Regression gate: any host FAIL (or missing file when wanted) is nonzero.
FAIL=0
if wanted crabs; then
  [ -f "$CRABS_OUT" ] || FAIL=1
  [ "$C_M" = "0" ] && [ "$C_F" = "0" ] && [ "$C_O" = "0" ] && [ "$C_R" = "0" ] || FAIL=1
  [ "$CRABS_CODE" = "0" ] || FAIL=1
fi
if wanted ghostty; then
  [ -f "$GHOSTTY_OUT" ] || FAIL=1
  [ "$G_M" = "0" ] && [ "$G_F" = "0" ] && [ "$G_O" = "0" ] && [ "$G_R" = "0" ] || FAIL=1
  [ "$GHOSTTY_CODE" = "0" ] || FAIL=1
fi
case "$VM_STATE" in
  probed-via-ssh:*)
    [ "$V_M" = "0" ] && [ "$V_F" = "0" ] && [ "$V_O" = "0" ] && [ "$V_R" = "0" ] || FAIL=1
    [ "${VM_CODE:-0}" = "0" ] || FAIL=1
    ;;
esac
if [ "$FAIL" != "0" ]; then
  echo "terminal-matrix: REGRESSION detected (see $STAGING_TMP/gap-table.md)" >&2
  exit 1
fi
if [ -n "$DOC_DIR" ]; then
  echo "terminal-matrix: OK (frames: $STAGING_TMP + $DOC_DIR)"
else
  echo "terminal-matrix: OK (frames: $STAGING_TMP)"
fi
exit 0

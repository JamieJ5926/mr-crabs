#!/usr/bin/env bash
# Window-local PNG burst of palette open (cmd+shift+p) on a single-window instance.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
BIN="${MR_CRABS_BIN:-$HOME/.cargo-target/release/mr-crabs}"
CONFIG="$ROOT/config.json"
RUN_ID="${1:-open1}"
OUT="$ROOT/$RUN_ID"
CUA="${CUA:-$HOME/.local/bin/cua-driver}"
SESSION="palette-latency-$RUN_ID"
BURST_MS="${BURST_MS:-40}"
BURST_COUNT="${BURST_COUNT:-20}"

if [[ ! -x "$BIN" ]]; then
  echo "missing binary: $BIN" >&2
  exit 1
fi

rm -rf "$OUT"
mkdir -p "$OUT/frames" "$OUT/window_burst"

cleanup() {
  {
    echo "exit_status=${EXIT_STATUS:-unknown}"
    date '+%Y-%m-%d %H:%M:%S'
  } >>"$OUT/exit.txt" 2>/dev/null || true
  if [[ -n "${APP_PID:-}" ]] && kill -0 "$APP_PID" 2>/dev/null; then
    kill "$APP_PID" 2>/dev/null || true
    sleep 0.3
    kill -9 "$APP_PID" 2>/dev/null || true
  fi
  "$CUA" end_session "{\"session\":\"$SESSION\"}" >/dev/null 2>&1 || true
}
EXIT_STATUS=1
trap 'EXIT_STATUS=$?; cleanup' EXIT

{
  echo "run_id=$RUN_ID"
  echo "burst_ms=$BURST_MS"
  echo "burst_count=$BURST_COUNT"
  echo "started=$(date '+%Y-%m-%d %H:%M:%S')"
} >"$OUT/exit.txt"

"$CUA" start_session "{\"session\":\"$SESSION\",\"capture_scope\":\"window\"}" >/dev/null

"$BIN" \
  --default \
  --config-file "$CONFIG" \
  --animation none --text-animation-duration 0 --text-animation-intensity 0 \
  --cursor-trail=false \
  --cursor-blink=false \
  --startup-fetch=false \
  --startup-animation none \
  --working-directory "$OUT" \
  >"$OUT/app.stdout" 2>"$OUT/app.stderr" &
APP_PID=$!

ready=0
for _ in $(seq 1 50); do
  sleep 0.2
  if ! kill -0 "$APP_PID" 2>/dev/null; then
    echo "mr-crabs exited during launch" >&2
    cat "$OUT/app.stderr" >&2 || true
    exit 1
  fi
  WIN_JSON="$("$CUA" list_windows "{\"pid\":$APP_PID}" 2>/dev/null || true)"
  echo "$WIN_JSON" >"$OUT/list_windows.json"
  if python3 - "$OUT/list_windows.json" <<'PY'
import json, sys
p = sys.argv[1]
try:
    data = json.load(open(p))
except Exception:
    sys.exit(1)
wins = data.get("windows") or data.get("result", {}).get("windows") or []
if isinstance(data, list):
    wins = data
onscreen = [w for w in wins if w.get("is_on_screen", True)]
sys.exit(0 if len(onscreen) == 1 else 1)
PY
  then
    ready=1
    break
  fi
done

if [[ "$ready" != 1 ]]; then
  echo "timed out waiting for a single on-screen window for pid $APP_PID" >&2
  cat "$OUT/list_windows.json" >&2 || true
  exit 1
fi

WIN_ID="$(python3 - "$OUT/list_windows.json" <<'PY'
import json, sys
data = json.load(open(sys.argv[1]))
wins = data.get("windows") or data.get("result", {}).get("windows") or []
if isinstance(data, list):
    wins = data
onscreen = [w for w in wins if w.get("is_on_screen", True)]
print(onscreen[0]["window_id"])
PY
)"
echo "$APP_PID" >"$OUT/pid.txt"
echo "$WIN_ID" >"$OUT/window_id.txt"

"$CUA" get_window_state "{\"pid\":$APP_PID,\"window_id\":$WIN_ID}" \
  --screenshot-out-file "$OUT/pre.png" >"$OUT/pre_state.json"

W="$(python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); print(int(d.get("screenshot_width") or d.get("width") or 800))' "$OUT/pre_state.json")"
H="$(python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); print(int(d.get("screenshot_height") or d.get("height") or 600))' "$OUT/pre_state.json")"
CX=$((W / 2))
CY=$((H / 2))
echo "$W $H $CX $CY" >"$OUT/geom.txt"

"$CUA" click "{\"pid\":$APP_PID,\"window_id\":$WIN_ID,\"x\":$CX,\"y\":$CY,\"delivery_mode\":\"foreground\"}" >"$OUT/click.json"
sleep 0.4
python3 -c 'import time; print(int(time.time()*1000))' >"$OUT/t_record_start_ms.txt"
python3 "$ROOT/burst.py" "$OUT" "$CUA" "$APP_PID" "$WIN_ID" "$BURST_MS" "$BURST_COUNT" "$CX" "$CY"

python3 -c 'import time; print(int(time.time()*1000))' >"$OUT/t_record_stop_ms.txt"

"$CUA" get_window_state "{\"pid\":$APP_PID,\"window_id\":$WIN_ID}" \
  --screenshot-out-file "$OUT/post.png" >"$OUT/post_state.json" || true

python3 "$ROOT/analyze.py" "$OUT"

EXIT_STATUS=0
echo "ok $OUT"

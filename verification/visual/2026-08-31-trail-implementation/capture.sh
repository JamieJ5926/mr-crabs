#!/usr/bin/env bash
set -euo pipefail
ROOT="$(cd "$(dirname "$0")" && pwd)"
BIN="${MR_CRABS_BIN:-$HOME/.cargo-target/release/mr-crabs}"
CONFIG="$ROOT/config.json"
OUT="$ROOT/jump"
CUA="${CUA:-$HOME/.local/bin/cua-driver}"
SESSION="trail-impl-jump"

if [[ ! -x "$BIN" ]]; then
  echo "missing binary: $BIN" >&2
  exit 1
fi

rm -rf "$OUT"
mkdir -p "$OUT/frames"

cleanup() {
  if [[ -n "${APP_PID:-}" ]] && kill -0 "$APP_PID" 2>/dev/null; then
    kill "$APP_PID" 2>/dev/null || true
    sleep 0.2
    kill -9 "$APP_PID" 2>/dev/null || true
  fi
  "$CUA" end_session "{\"session\":\"$SESSION\"}" >/dev/null 2>&1 || true
}
trap cleanup EXIT

"$CUA" start_session "{\"session\":\"$SESSION\",\"capture_scope\":\"window\"}" >/dev/null

"$BIN" \
  --default \
  --config-file "$CONFIG" \
  --animation cursor-trail \
  --cursor-trail-opacity=1 \
  --cursor-trail-duration=2000ms \
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
data = json.load(open(sys.argv[1]))
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
  echo "timed out waiting for a single on-screen window" >&2
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
"$CUA" click "{\"pid\":$APP_PID,\"window_id\":$WIN_ID,\"x\":$CX,\"y\":$CY,\"delivery_mode\":\"foreground\"}" >"$OUT/click.json"
sleep 0.4
screencapture -x -o -S -l "$WIN_ID" "$OUT/frames/frame_0001.png"
"$CUA" press_key "{\"pid\":$APP_PID,\"window_id\":$WIN_ID,\"x\":$CX,\"y\":$CY,\"delivery_mode\":\"foreground\",\"key\":\"u\",\"modifiers\":[\"ctrl\"]}" >"$OUT/clear.json" || true
"$CUA" clipboard_write "{\"text\":\"printf xxxxxx\"}" >"$OUT/clip.json"
"$CUA" hotkey "{\"pid\":$APP_PID,\"window_id\":$WIN_ID,\"x\":$CX,\"y\":$CY,\"delivery_mode\":\"foreground\",\"keys\":[\"cmd\",\"v\"]}" >"$OUT/paste.json"
"$CUA" press_key "{\"pid\":$APP_PID,\"window_id\":$WIN_ID,\"x\":$CX,\"y\":$CY,\"delivery_mode\":\"foreground\",\"key\":\"enter\"}" >"$OUT/enter.json"

python3 - "$OUT" "$WIN_ID" <<'PY'
import subprocess, sys, time
from pathlib import Path
out = Path(sys.argv[1])
win = sys.argv[2]
frames = out / "frames"
t0 = time.perf_counter()
for i in range(2, 21):
    target = t0 + (i - 2) * 0.05
    now = time.perf_counter()
    if target > now:
        time.sleep(target - now)
    png = frames / f"frame_{i:04d}.png"
    r = subprocess.run(
        ["screencapture", "-x", "-o", "-S", "-l", str(win), str(png)],
        capture_output=True, text=True, timeout=5,
    )
    if r.returncode != 0 or not png.exists() or png.stat().st_size == 0:
        raise SystemExit(f"capture failed at {png}")
print("captured", len(list(frames.glob("frame_*.png"))))
PY

python3 - "$OUT" <<'PY'
import json, sys
from pathlib import Path
try:
    from PIL import Image
except ImportError:
    import subprocess
    subprocess.check_call([sys.executable, "-m", "pip", "install", "--quiet", "pillow"])
    from PIL import Image

out = Path(sys.argv[1])
frames = sorted((out / "frames").glob("frame_*.png"))
if len(frames) < 8:
    raise SystemExit(f"need >=8 frames, got {len(frames)}")
imgs = [Image.open(p).convert("RGB") for p in frames]
diffs = []
for a, b, pa, pb in zip(imgs, imgs[1:], frames, frames[1:]):
    da, db = a.tobytes(), b.tobytes()
    changed = sum(1 for x, y in zip(da, db) if x != y)
    diffs.append({"a": pa.name, "b": pb.name, "changed_bytes": changed})
(out / "frame_diffs.json").write_text(json.dumps(diffs, indent=2))
live = sum(1 for d in diffs if d["changed_bytes"] > 0)
if live < 2:
    raise SystemExit("vacated-cell capture did not change across frames")
print("live_diffs", live)
print("max_changed", max(d["changed_bytes"] for d in diffs))
PY

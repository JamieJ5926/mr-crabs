#!/usr/bin/env bash
# Window-local PNG burst of a single-window Mr Crabs instance.
# Burst starts first; trigger fires mid-span so the captured window contains it.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")" && pwd)"
BIN="${MR_CRABS_BIN:-$HOME/.cargo-target/release/mr-crabs}"
CONFIG="$ROOT/config.json"
RUN_ID="${1:-run1}"
OUT="$ROOT/$RUN_ID"
CUA="${CUA:-$HOME/.local/bin/cua-driver}"
SESSION="capture-pilot-$RUN_ID"
BURST_MS="${BURST_MS:-50}"
BURST_COUNT="${BURST_COUNT:-16}"
MODE="${CAPTURE_MODE:-marker}"

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
  echo "mode=$MODE"
  echo "burst_ms=$BURST_MS"
  echo "burst_count=$BURST_COUNT"
  echo "started=$(date '+%Y-%m-%d %H:%M:%S')"
} >"$OUT/exit.txt"

"$CUA" start_session "{\"session\":\"$SESSION\",\"capture_scope\":\"window\"}" >/dev/null

ANIMATION_ARGS=(--animation none --text-animation-duration 0 --text-animation-intensity 0)
if [[ "$MODE" == "typewriter" ]]; then
  ANIMATION_ARGS=(--animation typewriter --text-animation-duration 2000 --text-animation-intensity 1)
fi
"$BIN" \
  --default \
  --config-file "$CONFIG" \
  "${ANIMATION_ARGS[@]}" \
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
BOUNDS="$(python3 - "$OUT/list_windows.json" <<'PY'
import json, sys
data = json.load(open(sys.argv[1]))
wins = data.get("windows") or data.get("result", {}).get("windows") or []
if isinstance(data, list):
    wins = data
onscreen = [w for w in wins if w.get("is_on_screen", True)]
b = onscreen[0]["bounds"]
print("%d %d %d %d" % (int(b["x"]), int(b["y"]), int(b["width"]), int(b["height"])))
PY
)"
read -r WIN_X WIN_Y WIN_W WIN_H <<<"$BOUNDS"
echo "$WIN_X $WIN_Y $WIN_W $WIN_H" >"$OUT/bounds.txt"

"$CUA" get_window_state "{\"pid\":$APP_PID,\"window_id\":$WIN_ID}" \
  --screenshot-out-file "$OUT/pre.png" >"$OUT/pre_state.json"

W="$(python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); print(int(d.get("screenshot_width") or d.get("width") or 800))' "$OUT/pre_state.json")"
H="$(python3 -c 'import json,sys; d=json.load(open(sys.argv[1])); print(int(d.get("screenshot_height") or d.get("height") or 600))' "$OUT/pre_state.json")"
CX=$((W / 2))
CY=$((H / 2))
echo "$W $H $CX $CY" >"$OUT/geom.txt"

"$CUA" click "{\"pid\":$APP_PID,\"window_id\":$WIN_ID,\"x\":$CX,\"y\":$CY,\"delivery_mode\":\"foreground\"}" >"$OUT/click.json"
"$CUA" get_window_state "{\"pid\":$APP_PID,\"window_id\":$WIN_ID,\"include_screenshot\":false}" >"$OUT/post_click.json"

# Self-dating marker printed at 100 ms. Started before the burst.
cat >"$OUT/clock.py" <<'PY'
import sys, time
from pathlib import Path
n = 0
trig = Path("TRIGGER.flag")
fired = False
while True:
    if (not fired) and trig.exists():
        sys.stdout.write("ZX9Q7 TRIGGER n=%d t=%d\n" % (n, int(time.time() * 1000)))
        sys.stdout.flush()
        fired = True
    sys.stdout.write("MARK n=%d t=%d\n" % (n, int(time.time() * 1000)))
    sys.stdout.flush()
    n += 1
    time.sleep(0.1)
PY
"$CUA" press_key "{\"pid\":$APP_PID,\"window_id\":$WIN_ID,\"x\":$CX,\"y\":$CY,\"delivery_mode\":\"foreground\",\"key\":\"u\",\"modifiers\":[\"ctrl\"]}" >"$OUT/clock_clear.json" || true
"$CUA" clipboard_write "{\"text\":\"python3 -u clock.py\"}" >"$OUT/clip.json"
"$CUA" hotkey "{\"pid\":$APP_PID,\"window_id\":$WIN_ID,\"x\":$CX,\"y\":$CY,\"delivery_mode\":\"foreground\",\"keys\":[\"cmd\",\"v\"]}" >"$OUT/clock_paste.json"
"$CUA" press_key "{\"pid\":$APP_PID,\"window_id\":$WIN_ID,\"x\":$CX,\"y\":$CY,\"delivery_mode\":\"foreground\",\"key\":\"enter\"}" >"$OUT/clock_enter.json"
echo "ZX9Q7" >"$OUT/trigger_token.txt"
sleep 1.5
# Prove the clock is printing before bursting.
PROBE1="$OUT/probe1.png"
PROBE2="$OUT/probe2.png"
screencapture -x -o -S -l "$WIN_ID" "$PROBE1" || true
sleep 0.35
screencapture -x -o -S -l "$WIN_ID" "$PROBE2" || true
python3 - "$PROBE1" "$PROBE2" <<'PY'
import sys
from pathlib import Path
try:
    from PIL import Image
except ImportError:
    raise SystemExit(0)
a, b = Path(sys.argv[1]), Path(sys.argv[2])
if not a.exists() or not b.exists():
    raise SystemExit("probe screenshots missing")
ia, ib = Image.open(a), Image.open(b)
if ia.tobytes() == ib.tobytes():
    raise SystemExit("probe frames identical; window capture is frozen")
print("probe live", ia.size)
PY

python3 -c 'import time; print(int(time.time()*1000))' >"$OUT/t_record_start_ms.txt"

# Burst first. Trigger fires after a few frames so it lands inside the span.
python3 - "$OUT" "$CUA" "$APP_PID" "$WIN_ID" "$BURST_MS" "$BURST_COUNT" "$CX" "$CY" "$WIN_X" "$WIN_Y" "$WIN_W" "$WIN_H" <<'PY'
import json, subprocess, sys, time
from pathlib import Path

out = Path(sys.argv[1])
cua, pid, win = sys.argv[2], sys.argv[3], sys.argv[4]
burst_ms = int(sys.argv[5])
count = int(sys.argv[6])
cx, cy = int(sys.argv[7]), int(sys.argv[8])
wx, wy, ww, wh = map(int, sys.argv[9:13])
inner_x = wx + int(ww * 0.08)
inner_y = wy + int(wh * 0.12)
inner_w = int(ww * 0.84)
inner_h = int(wh * 0.76)
region = f"{inner_x},{inner_y},{inner_w},{inner_h}"
(out / "crop_region.txt").write_text(region + "\n")
frames = out / "frames"
times = []
interval = burst_ms / 1000.0
trigger_after = 4
t0 = time.perf_counter()
triggered = False
for i in range(1, count + 1):
    target = t0 + (i - 1) * interval
    now = time.perf_counter()
    if target > now:
        time.sleep(target - now)
    if (not triggered) and i >= trigger_after:
        t_trig = time.time()
        (out / "t_trigger_ms.txt").write_text(str(int(t_trig * 1000)) + "\n")
        (out / "TRIGGER.flag").write_text("1\n")
        triggered = True
    t_call = time.time()
    png = frames / f"frame_{i:04d}.png"
    js = out / "window_burst" / f"w_{i:04d}.json"
    r = subprocess.run(
        ["screencapture", "-x", "-o", "-S", "-l", str(win), str(png)],
        capture_output=True, text=True, timeout=5,
    )
    if r.returncode != 0 or not png.exists() or png.stat().st_size == 0:
        r = subprocess.run(
            [cua, "get_window_state",
             json.dumps({"pid": int(pid), "window_id": int(win), "include_screenshot": True}),
             "--screenshot-out-file", str(png)],
            capture_output=True, text=True, timeout=8,
        )
    t_done = time.time()
    js.write_text(r.stdout or r.stderr or "")
    times.append({
        "i": i,
        "wall_ms": int(t_call * 1000),
        "done_ms": int(t_done * 1000),
        "call_ms": (t_done - t_call) * 1000,
        "since_start_ms": (time.perf_counter() - t0) * 1000,
        "png_exists": png.exists(),
        "png_bytes": png.stat().st_size if png.exists() else 0,
        "rc": r.returncode,
        "triggered_this_frame": (i == trigger_after),
    })

(out / "burst_times.json").write_text(json.dumps(times, indent=2))
intervals = [b["wall_ms"] - a["wall_ms"] for a, b in zip(times, times[1:])]
summary = {
    "count": len(times),
    "png_ok": sum(1 for t in times if t["png_exists"] and t["png_bytes"] > 0),
    "mean_interval_ms": (sum(intervals) / len(intervals)) if intervals else None,
    "median_interval_ms": (sorted(intervals)[len(intervals)//2] if intervals else None),
    "min_interval_ms": min(intervals) if intervals else None,
    "max_interval_ms": max(intervals) if intervals else None,
    "mean_call_ms": sum(t["call_ms"] for t in times) / len(times),
    "intervals_ms": intervals,
}
(out / "interval.json").write_text(json.dumps(summary, indent=2))
print(json.dumps(summary, indent=2))
if summary["png_ok"] < 3:
    raise SystemExit("too few window PNGs")
if summary["mean_interval_ms"] is None or summary["mean_interval_ms"] > 100:
    print("WARN mean interval coarser than 100ms", file=sys.stderr)
if not (out / "t_trigger_ms.txt").exists():
    raise SystemExit("trigger never fired")
t_start = int((out / "t_record_start_ms.txt").read_text().strip())
t_trig = int((out / "t_trigger_ms.txt").read_text().strip())
if t_trig < t_start:
    raise SystemExit(f"ordering fail: trigger {t_trig} before burst start {t_start}")
PY

python3 -c 'import time; print(int(time.time()*1000))' >"$OUT/t_record_stop_ms.txt"

"$CUA" get_window_state "{\"pid\":$APP_PID,\"window_id\":$WIN_ID}" \
  --screenshot-out-file "$OUT/window_burst/w_post.png" \
  >"$OUT/window_burst/w_post.json" || true

python3 - "$OUT" <<'PY'
import json, re, subprocess, sys
from pathlib import Path
try:
    from PIL import Image
except ImportError:
    subprocess.check_call([sys.executable, "-m", "pip", "install", "--quiet", "pillow"])
    from PIL import Image

out = Path(sys.argv[1])
frames = sorted((out / "frames").glob("frame_*.png"))
if len(frames) < 3:
    raise SystemExit(f"need >=3 frames, got {len(frames)}")

token = (out / "trigger_token.txt").read_text().strip() if (out / "trigger_token.txt").exists() else ""

def crop_text(im):
    w, h = im.size
    l = int(w * 0.08)
    t = int(h * 0.12)
    r = int(w * 0.92)
    btm = int(h * 0.88)
    return im.crop((l, t, r, btm)).convert("RGB")

def series_for(paths, cropper):
    imgs = [cropper(Image.open(p)).resize((240, 180)) for p in paths]
    series = []
    for i in range(1, len(imgs)):
        a = imgs[i - 1]
        bimg = imgs[i]
        pa, pb = a.tobytes(), bimg.tobytes()
        changed = 0
        mad_acc = 0
        n = len(pa)
        step = 12
        pixels = max(1, n // step)
        for j in range(0, n, step):
            d = abs(pa[j] - pb[j]) + abs(pa[j + 1] - pb[j + 1]) + abs(pa[j + 2] - pb[j + 2])
            mad_acc += d
            if d > 12:
                changed += 1
        series.append({
            "pair": f"{paths[i-1].name}->{paths[i].name}",
            "changed_pixels": changed,
            "changed_frac": changed / pixels,
            "mean_abs_channel_sum": mad_acc / pixels,
        })
    return {
        "frame_count": len(paths),
        "diff_resize": "240x180",
        "nonzero_diff_pairs": sum(1 for s in series if s["changed_pixels"] > 0),
        "substantial_pairs": sum(1 for s in series if s["changed_frac"] > 0.002),
        "series": series,
    }

def ocr(path):
    r = subprocess.run(["tesseract", str(path), "stdout", "--psm", "6"], capture_output=True, text=True, timeout=20)
    return r.stdout or ""

ocr_hits = []
first_token_frame = None
first_mark_frame = None
last_mark = None
indices = sorted(set([1, len(frames)] + list(range(1, len(frames)+1, max(1, len(frames)//8)))))
for i in indices:
    p = frames[i - 1]
    text = ocr(p)
    marks = re.findall(r"MARK n=(\d+) t=(\d+)", text)
    hit = {
        "frame_index": i,
        "path": p.name,
        "has_token": "TRIGGER" in text or "ZX9Q7" in text or (token and token in text),
        "marks": [{"n": int(a), "t": int(b)} for a, b in marks],
        "snippet": " ".join(text.split())[:240],
    }
    if hit["has_token"] and first_token_frame is None:
        first_token_frame = i
    if marks and first_mark_frame is None:
        first_mark_frame = i
    if marks:
        last_mark = marks[-1]
    ocr_hits.append(hit)

post = out / "window_burst" / "w_post.png"
post_ocr = ocr(post) if post.exists() else ""

def read_ms(name):
    p = out / name
    if not p.exists():
        return None
    try:
        return int(p.read_text().strip())
    except Exception:
        return None

video = series_for(frames, crop_text)
interval = json.loads((out / "interval.json").read_text()) if (out / "interval.json").exists() else {}
t_start = read_ms("t_record_start_ms.txt")
t_stop = read_ms("t_record_stop_ms.txt")
t_trig = read_ms("t_trigger_ms.txt")
session_span_ms = (t_stop - t_start) if (t_start and t_stop) else None
order_ok = (t_start is not None and t_trig is not None and t_trig >= t_start)

payload = {
    "mechanism": "window-local screenshot PNG burst; trigger mid-span",
    "trigger_token": token,
    "mean_interval_ms": interval.get("mean_interval_ms"),
    "median_interval_ms": interval.get("median_interval_ms"),
    "min_interval_ms": interval.get("min_interval_ms"),
    "max_interval_ms": interval.get("max_interval_ms"),
    "mean_call_ms": interval.get("mean_call_ms"),
    "frame_count": len(frames),
    "png_ok": interval.get("png_ok"),
    "t_record_start_ms": t_start,
    "t_trigger_ms": t_trig,
    "t_record_stop_ms": t_stop,
    "session_span_ms": session_span_ms,
    "order_ok": order_ok,
    "first_mark_extracted_frame": first_mark_frame,
    "first_token_extracted_frame": first_token_frame,
    "last_ocr_mark": last_mark,
    "capture_end_frame": len(frames),
    "post_has_token": "TRIGGER" in post_ocr or "ZX9Q7" in post_ocr,
    "video": video,
    "ocr_sample": ocr_hits,
}
(out / "frame_diff.json").write_text(json.dumps(payload, indent=2))
print(json.dumps({k: payload[k] for k in payload if k not in ("video", "ocr_sample")}, indent=2))
print("--- series ---")
for s in video["series"]:
    print(f"{s['pair']}  changed={s['changed_pixels']}  frac={s['changed_frac']:.6f}  mad={s['mean_abs_channel_sum']:.3f}")
print(f"first_token_frame={first_token_frame} first_mark_frame={first_mark_frame} last_mark={last_mark}")
print(f"session_span_ms={session_span_ms} mean_interval_ms={interval.get('mean_interval_ms')} order_ok={order_ok}")
if not order_ok:
    raise SystemExit("burst did not start before trigger")
if first_mark_frame is None and first_token_frame is None:
    raise SystemExit("neither MARK nor ZX9Q7 visible in burst frames")
if interval.get("mean_interval_ms") is None:
    raise SystemExit("no interval measured")
PY

echo "OK $OUT"
EXIT_STATUS=0

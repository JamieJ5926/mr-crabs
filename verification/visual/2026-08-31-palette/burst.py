import json, subprocess, sys, threading, time
from pathlib import Path

out = Path(sys.argv[1])
cua, pid, win = sys.argv[2], sys.argv[3], sys.argv[4]
burst_ms = int(sys.argv[5])
count = int(sys.argv[6])
cx, cy = int(sys.argv[7]), int(sys.argv[8])
frames = out / "frames"
times = []
interval = burst_ms / 1000.0
trigger_after = 4
t0 = time.perf_counter()
triggered = False
trigger_thread = None
trigger_i = None
trigger_done = threading.Event()
lock = threading.Lock()
stop_after = count


def send_open():
    global stop_after
    t_trig = time.time()
    (out / "t_trigger_ms.txt").write_text(str(int(t_trig * 1000)) + "\n")
    r_hot = subprocess.run(
        [
            cua,
            "press_key",
            json.dumps({
                "pid": int(pid),
                "window_id": int(win),
                "x": cx,
                "y": cy,
                "delivery_mode": "foreground",
                "key": "p",
                "modifiers": ["cmd", "shift"],
            }),
        ],
        capture_output=True,
        text=True,
        timeout=20,
    )
    payload = r_hot.stdout or r_hot.stderr or ""
    payload += "\npress_key_rc=%s elapsed_ms=%s\n" % (
        r_hot.returncode,
        int((time.time() - t_trig) * 1000),
    )
    (out / "hotkey.json").write_text(payload)
    with lock:
        current = len(times)
        stop_after = max(stop_after, current + 15)
    trigger_done.set()


i = 0
while True:
    i += 1
    with lock:
        limit = stop_after
        done = trigger_done.is_set()
    if i > limit and done:
        break
    if i > 120:
        break
    target = t0 + (i - 1) * interval
    now = time.perf_counter()
    if target > now:
        time.sleep(target - now)
    if (not triggered) and i >= trigger_after:
        trigger_thread = threading.Thread(target=send_open, daemon=True)
        trigger_thread.start()
        triggered = True
        trigger_i = i
    t_call = time.time()
    png = frames / ("frame_%04d.png" % i)
    r = subprocess.run(
        ["screencapture", "-x", "-o", "-S", "-l", str(win), str(png)],
        capture_output=True,
        text=True,
        timeout=5,
    )
    t_done = time.time()
    times.append({
        "i": i,
        "wall_ms": int(t_call * 1000),
        "done_ms": int(t_done * 1000),
        "call_ms": (t_done - t_call) * 1000,
        "since_start_ms": (time.perf_counter() - t0) * 1000,
        "png_exists": png.exists(),
        "png_bytes": png.stat().st_size if png.exists() else 0,
        "rc": r.returncode,
        "triggered_this_frame": (i == trigger_i),
    })

if trigger_thread is not None:
    trigger_thread.join(timeout=25)

(out / "burst_times.json").write_text(json.dumps(times, indent=2))
intervals = [b["wall_ms"] - a["wall_ms"] for a, b in zip(times, times[1:])]
summary = {
    "count": len(times),
    "png_ok": sum(1 for t in times if t["png_exists"] and t["png_bytes"] > 0),
    "mean_interval_ms": (sum(intervals) / len(intervals)) if intervals else None,
    "median_interval_ms": (sorted(intervals)[len(intervals) // 2] if intervals else None),
    "min_interval_ms": min(intervals) if intervals else None,
    "max_interval_ms": max(intervals) if intervals else None,
    "mean_call_ms": sum(t["call_ms"] for t in times) / len(times) if times else None,
    "intervals_ms": intervals,
    "input_route": "press_key cmd-shift-p foreground, capture continues until key returns",
}
(out / "interval.json").write_text(json.dumps(summary, indent=2))
print(json.dumps(summary, indent=2))
if summary["png_ok"] < 3:
    raise SystemExit("too few window PNGs")
if not (out / "t_trigger_ms.txt").exists():
    raise SystemExit("trigger never fired")

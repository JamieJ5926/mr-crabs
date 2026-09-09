import json, sys
from pathlib import Path

try:
    from PIL import Image
except ImportError:
    import subprocess
    subprocess.check_call([sys.executable, "-m", "pip", "install", "--quiet", "pillow"])
    from PIL import Image

out = Path(sys.argv[1])
times = json.loads((out / "burst_times.json").read_text())
frames = sorted((out / "frames").glob("frame_*.png"))
if len(frames) < 3:
    raise SystemExit(f"need >=3 frames, got {len(frames)}")
trig_i = next(t["i"] for t in times if t.get("triggered_this_frame"))
trig_ms = int((out / "t_trigger_ms.txt").read_text().strip())
baseline = Image.open(frames[0]).convert("RGB")
bw, bh = baseline.size
base_bytes = baseline.tobytes()
first_change = None
for t, path in zip(times, frames):
    im = Image.open(path).convert("RGB")
    if im.tobytes() != base_bytes:
        first_change = t
        break
report = {
    "trigger_frame": trig_i,
    "trigger_wall_ms": trig_ms,
    "first_visual_change_frame": None if first_change is None else first_change["i"],
    "first_visual_change_wall_ms": None if first_change is None else first_change["wall_ms"],
    "open_latency_ms": None if first_change is None else (first_change["wall_ms"] - trig_ms),
    "frame_interval_from_trigger": None if first_change is None else (first_change["i"] - trig_i),
    "crop_box": [0, 0, bw, bh],
    "frame_size": [bw, bh],
    "hotkey": (out / "hotkey.json").read_text()[:2000] if (out / "hotkey.json").exists() else "",
}
(out / "latency.json").write_text(json.dumps(report, indent=2))
print(json.dumps(report, indent=2))
if first_change is None:
    raise SystemExit("no visual change after palette hotkey")

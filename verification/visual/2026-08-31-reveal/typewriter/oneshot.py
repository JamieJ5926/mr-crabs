import sys, time
from pathlib import Path
here = Path(__file__).resolve().parent
os_cwd = Path.cwd()
root = here if (here / "payload.txt").exists() else os_cwd
Path(root / "oneshot_ready.txt").write_text("1\n")
payload = Path(root / "payload.txt").read_text().rstrip("\n")
trig = Path(root / "TRIGGER.flag")
while not trig.exists():
    time.sleep(0.01)
sys.stdout.write(payload + "\n")
sys.stdout.flush()
Path(root / "oneshot_fired.txt").write_text("%d\n" % int(time.time() * 1000))
time.sleep(12)

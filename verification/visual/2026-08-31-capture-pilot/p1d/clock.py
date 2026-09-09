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

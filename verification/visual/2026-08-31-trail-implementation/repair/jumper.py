import sys
import time
from pathlib import Path

trig = Path("TRIGGER.flag")
sys.stdout.write("AAAAA")
sys.stdout.flush()
while not trig.exists():
    time.sleep(0.02)
sys.stdout.write("\033[5D")
sys.stdout.flush()
time.sleep(3.0)

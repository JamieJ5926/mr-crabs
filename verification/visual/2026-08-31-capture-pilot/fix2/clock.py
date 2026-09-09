import sys, time
n = 0
while True:
    sys.stdout.write("MARK n=%d t=%d\n" % (n, int(time.time() * 1000)))
    sys.stdout.flush()
    n += 1
    time.sleep(0.1)

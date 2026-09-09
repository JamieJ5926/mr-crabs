#!/bin/bash
python3 -c 'import sys; sys.stdout.write("SMOKE e\u0301x\n"); sys.stdout.flush()'
exec sleep 3600

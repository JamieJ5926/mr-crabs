#!/bin/bash
set -euo pipefail
# Launcher for the deterministic attribution harness (single real PTY path).
if [[ "${1:-}" == "--" ]]; then
  shift
fi
FORWARD_ARGS=("$@")
OUT="$(mktemp /tmp/mr-crabs-phase-runner.XXXXXX)"
export MR_CRABS_PHASE_OUT="$OUT"
echo "[phase_runner.sh] building+running headless phase-runner (single PTY→PaneModel→RenderCache path, fail-closed)"
echo "[phase_runner.sh] MR_CRABS_PHASE=1 cargo run --features phase-timing (always) -- ${FORWARD_ARGS[*]:-<no forwarded args>}"
RUNNER=(cargo run --manifest-path crates/mr-crabs-app/Cargo.toml --bin phase-runner --features phase-timing --)
if (( ${#FORWARD_ARGS[@]} )); then
  RUNNER+=("${FORWARD_ARGS[@]}")
fi
MR_CRABS_PHASE=1 "${RUNNER[@]}" 2>&1 | tee /tmp/phase-runner-stdout.log
echo "--- wall/sum/remainder + bytes/chunks/frames (stdout json) ---"
cat /tmp/phase-runner-stdout.log
echo "--- sidecar $OUT ---"
if [[ -f "$OUT" ]]; then
  cat "$OUT"
  echo "--- validating JSONL + attribution invariants ---"
  python3 - "$OUT" << 'PY'
import json, sys, os
path=sys.argv[1]
ok=True
# Fail closed on empty sidecar (mktemp pre-creates empty file).
try:
    st = os.stat(path)
    if st.st_size == 0:
        print(f"sidecar is empty at {path} (fail-closed)")
        sys.exit(1)
except Exception as e:
    print(f"sidecar stat failed at {path}: {e}")
    sys.exit(1)
has_summary=False
with open(path) as f:
  for lineno, line in enumerate(f,1):
    line=line.strip()
    if not line: continue
    try:
      obj=json.loads(line)
    except Exception as e:
      print(f"JSON invalid line {lineno}: {e}")
      ok=False
      continue
    if "phases" in obj:
      # summary line: must contain phases array (phase-only rows lack it)
      has_summary=True
      for k in ["ts_ms","workload","path","expected_bytes","drained_bytes","chunks","frames","wall_nanos","top_sum_nanos","remainder_nanos","success"]:
        if k not in obj:
          print(f"missing {k} at line {lineno}")
          ok=False
      if obj.get("top_sum_nanos",0) > obj.get("wall_nanos",0):
        print(f"top_sum > wall at line {lineno}: {obj}")
        ok=False
      if obj.get("remainder_nanos",0) != obj.get("wall_nanos",0) - obj.get("top_sum_nanos",0):
        print(f"remainder != wall-top at line {lineno}: {obj}")
        ok=False
      # Runner enforces the budget gate; wrapper only validates structural invariants.
      if obj.get("drained_bytes",-1) != obj.get("expected_bytes",-2):
        print(f"drained != expected at line {lineno}: {obj}")
        ok=False
      if not obj.get("success", False):
        print(f"workload failed (fail-closed) at line {lineno}: {obj.get('workload')} {obj.get('error')}")
        ok=False
      phases = obj.get("phases", None)
      if phases is None or len(phases) == 0:
        print(f"empty deltas/phases at line {lineno}: {obj}")
        ok=False
      else:
        by_phase = {p.get("phase"): p for p in phases if "phase" in p}
        for need in ["pane_pump", "render_cache_apply"]:
          rec = by_phase.get(need)
          if rec is None or int(rec.get("count", 0)) <= 0:
            print(f"missing or zero count for required top phase {need} at line {lineno}: {phases}")
            ok=False
        rec = by_phase.get("terminal_feed")
        if rec is None or int(rec.get("count", 0)) <= 0:
          print(f"missing or zero count for terminal_feed at line {lineno}: {phases}")
          ok=False
    else:
      # phase-only row: not a summary, skip summary checks
      continue
if not has_summary:
  print(f"sidecar has no summary objects containing 'phases' at {path} (fail-closed)")
  ok=False
if ok:
  print("JSONL ok, invariants hold")
else:
  sys.exit(1)
PY
else
  echo "sidecar missing at $OUT (fail-closed: wrapper always runs with --features phase-timing + MR_CRABS_PHASE=1)"
  exit 1
fi

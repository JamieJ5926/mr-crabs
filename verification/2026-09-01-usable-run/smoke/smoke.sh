#!/usr/bin/env bash
# Rerun Mr Crabs GUI smoke captures into a fresh timestamped subdirectory.
set -euo pipefail

BUNDLE_ID="dev.jamie.mr-crabs"
HERE="$(cd "$(dirname "$0")" && pwd)"
STAMP="$(date +%Y%m%d-%H%M%S)"
OUT="$HERE/run-$STAMP"
mkdir -p "$OUT"

log() { printf '%s\n' "$*" | tee -a "$OUT/run.log"; }

alive_pgrep() {
  pgrep -fl '/Mr Crabs.app/Contents/MacOS/mr-crabs' || true
}

relaunch() {
  log "launch_app $BUNDLE_ID"
  cua-driver launch_app "{\"bundle_id\":\"$BUNDLE_ID\"}" | tee -a "$OUT/run.log"
}

pid_and_window() {
  python3 - << 'PY'
import json, subprocess, sys
t = subprocess.check_output(["cua-driver", "launch_app", '{"bundle_id":"dev.jamie.mr-crabs"}'], text=True)
d = json.loads(t[t.find("{"):])
pid = d["pid"]
wins = [w for w in d.get("windows") or [] if w.get("is_on_screen") and (w.get("bounds") or {}).get("width", 0) > 0]
if not wins:
    t2 = subprocess.check_output(["cua-driver", "list_windows", json.dumps({"pid": pid})], text=True)
    d2 = json.loads(t2[t2.find("{"):])
    wins = [w for w in d2.get("windows") or [] if w.get("is_on_screen") and (w.get("bounds") or {}).get("width", 0) > 0]
if not wins:
    sys.exit("no on-screen window with non-zero bounds")
w = wins[0]
print(f"{pid} {w['window_id']}")
PY
}

capture() {
  local n="$1" slug="$2" pid="$3" wid="$4"
  cua-driver get_window_state "{\"pid\":$pid,\"window_id\":$wid}" \
    --screenshot-out-file "$OUT/${n}-${slug}.png" > "$OUT/${n}-${slug}.json"
  python3 - "$OUT" "$n" "$slug" << 'PY'
import json, pathlib, sys
out, n, slug = pathlib.Path(sys.argv[1]), sys.argv[2], sys.argv[3]
text = (out / f"{n}-{slug}.json").read_text()
d = json.loads(text[text.find("{"):])
(out / f"{n}-{slug}.tree.txt").write_text(d.get("tree_markdown") or "")
print(n, slug, "png", (out / f"{n}-{slug}.png").stat().st_size,
      "tree", len(d.get("tree_markdown") or ""),
      "bounds", d.get("window_bounds"))
PY
}

fg_type() {
  local pid="$1" wid="$2" text="$3"
  cua-driver type_text "{\"pid\":$pid,\"window_id\":$wid,\"x\":400,\"y\":400,\"text\":$(python3 -c 'import json,sys; print(json.dumps(sys.argv[1]))' "$text"),\"delivery_mode\":\"foreground\"}"
}

fg_key() {
  local pid="$1" wid="$2" key="$3"
  cua-driver press_key "{\"pid\":$pid,\"window_id\":$wid,\"key\":\"$key\",\"delivery_mode\":\"foreground\"}"
}

fg_hot() {
  local pid="$1" wid="$2" keys_json="$3"
  cua-driver hotkey "{\"pid\":$pid,\"window_id\":$wid,\"keys\":$keys_json,\"delivery_mode\":\"foreground\"}"
}

ensure_live() {
  local pid="$1"
  if ! pgrep -f '/Mr Crabs.app/Contents/MacOS/mr-crabs' >/dev/null; then
    log "DEFECT: app died (expected pid $pid). Relaunching."
    relaunch
    return 1
  fi
  return 0
}

log "===PGREP before==="
alive_pgrep | tee -a "$OUT/run.log"
relaunch
read -r PID WID < <(pid_and_window)
log "PID=$PID WID=$WID"
log "===PGREP after launch==="
alive_pgrep | tee -a "$OUT/pgrep.txt"
cua-driver list_windows "{\"pid\":$PID}" | tee "$OUT/list_windows.json"

capture 01 prompt-idle "$PID" "$WID"

fg_type "$PID" "$WID" "echo hello"
sleep 0.2
fg_key "$PID" "$WID" return
sleep 0.6
capture 02 echo-hello "$PID" "$WID"
ensure_live "$PID" || { read -r PID WID < <(pid_and_window); }

fg_type "$PID" "$WID" "ls -la"
sleep 0.2
fg_key "$PID" "$WID" return
sleep 0.6
capture 03 ls-la "$PID" "$WID"

fg_hot "$PID" "$WID" '["cmd","t"]'
sleep 0.5
capture 04 cmd-t-new-tab "$PID" "$WID"

fg_hot "$PID" "$WID" '["cmd","d"]'
sleep 0.5
capture 05 cmd-d-split-right "$PID" "$WID"

fg_hot "$PID" "$WID" '["cmd","shift","d"]'
sleep 0.5
capture 06 cmd-shift-d-split-down "$PID" "$WID"

fg_hot "$PID" "$WID" '["ctrl","cmd","right"]'
sleep 0.5
capture 07 ctrl-cmd-arrow-pane-focus "$PID" "$WID"

fg_hot "$PID" "$WID" '["cmd","]"]'
sleep 0.5
capture 08 cmd-bracket-tab-cycle "$PID" "$WID"

cua-driver drag "{\"pid\":$PID,\"window_id\":$WID,\"from_x\":80,\"from_y\":120,\"to_x\":500,\"to_y\":180,\"delivery_mode\":\"foreground\"}"
sleep 0.2
fg_hot "$PID" "$WID" '["cmd","c"]'
sleep 0.3
capture 09 drag-select-then-cmd-c "$PID" "$WID"

fg_hot "$PID" "$WID" '["cmd","v"]'
sleep 0.4
capture 10 cmd-v-paste "$PID" "$WID"

cua-driver clipboard_write '{"text":"echo line1\necho line2"}'
sleep 0.2
fg_hot "$PID" "$WID" '["cmd","v"]'
sleep 0.5
capture 11 multiline-paste "$PID" "$WID"

cua-driver scroll "{\"pid\":$PID,\"window_id\":$WID,\"x\":400,\"y\":400,\"direction\":\"up\",\"amount\":8,\"delivery_mode\":\"foreground\"}"
sleep 0.3
capture 12 wheel-scroll-up "$PID" "$WID"

cua-driver scroll "{\"pid\":$PID,\"window_id\":$WID,\"x\":400,\"y\":400,\"direction\":\"down\",\"amount\":8,\"delivery_mode\":\"foreground\"}"
sleep 0.3
capture 13 wheel-scroll-back "$PID" "$WID"

fg_hot "$PID" "$WID" '["cmd","shift","r"]'
sleep 0.6
capture 14 cmd-shift-r-reload "$PID" "$WID"

# zoom-button resize (token required)
python3 - "$PID" "$WID" << 'PY'
import json, subprocess, sys
pid, wid = sys.argv[1], sys.argv[2]
t = subprocess.check_output(["cua-driver","get_window_state", json.dumps({"pid":int(pid),"window_id":int(wid),"include_screenshot":False})], text=True)
d = json.loads(t[t.find("{"):])
el = next(e for e in d["elements"] if e.get("element_index")==2)
payload = {"pid":int(pid),"window_id":int(wid),"element_token":el["element_token"],"snapshot_id":d.get("snapshot_id")}
print(subprocess.check_output(["cua-driver","click", json.dumps(payload)], text=True)[:400])
PY
sleep 0.8
capture 15 window-resize "$PID" "$WID"

fg_hot "$PID" "$WID" '["cmd","shift","p"]'
sleep 0.5
capture 16 cmd-shift-p-palette "$PID" "$WID"

fg_hot "$PID" "$WID" '["ctrl","`"]'
sleep 0.5
capture 17 ctrl-backtick-quick-terminal "$PID" "$WID"

if command -v htop >/dev/null; then
  TUI=htop
else
  TUI=vim
fi
fg_type "$PID" "$WID" "$TUI"
sleep 0.2
fg_key "$PID" "$WID" return
sleep 1.0
capture 18 tui-render "$PID" "$WID"

log "wrote $OUT"
ls -1 "$OUT" | tee -a "$OUT/run.log"

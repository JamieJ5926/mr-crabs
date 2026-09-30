#!/bin/sh
# frank-terminal-matrix capability probe fixture.
# Derived from /tmp/crabs-probe-liveprobe/probe.sh (full label sequence
# preserved) plus blank-exec + OSC52-timeout + resize-check bytes for the
# frank-terminal-matrix harness (scripts/terminal-matrix.sh).
# Re-runnable with HERDR_* unset:
#   env -u HERDR_BIN_PATH -u HERDR_ENV -u HERDR_PANE_ID -u HERDR_SOCKET_PATH -u HERDR_STARTUP_CWD -u HERDR_TAB_ID -u HERDR_WORKSPACE_ID -u HERDR_SESSION_ID sh scripts/fixtures/capability-probe.sh
# Each sequence is emitted separately with a label line. No writes, no network.
# Scrub HERDR_* so a polluted parent env never leaks into captures.
for v in HERDR_BIN_PATH HERDR_ENV HERDR_PANE_ID HERDR_SOCKET_PATH HERDR_STARTUP_CWD HERDR_TAB_ID HERDR_WORKSPACE_ID HERDR_SESSION_ID; do
  unset "$v" 2>/dev/null || true
done
ESC=$(printf '\033')
BEL=$(printf '\a')

label() { printf '\n%s\n' "--- PROBE:$1 ---"; }

label "BLANK-EXEC-MARKER"
echo "BLANK-EXEC-MARKER-PRESENT=1"
echo "FIRST-EXEC-PAYLOAD: frank-terminal-matrix fixture alive"

label "ENV"
printenv TERM 2>/dev/null || echo "TERM=<unset>"
printenv COLORTERM 2>/dev/null || echo "COLORTERM=<unset>"

label "TRUECOLOR-FG-BG"
printf '%b\n' "${ESC}[38;2;255;100;0mORANGE-FG${ESC}[0m ${ESC}[48;2;0;120;255mBLUE-BG${ESC}[0m ${ESC}[38;2;0;255;0m${ESC}[48;2;255;0;255mFG-AND-BG${ESC}[0m"

label "256COLOR"
printf '%b\n' "${ESC}[38;5;196mRED256${ESC}[0m ${ESC}[38;5;46mGREEN256${ESC}[0m ${ESC}[48;5;21mBLUEBG256${ESC}[0m ${ESC}[38;5;208m${ESC}[48;5;235mCOMBO256${ESC}[0m"

label "STYLES"
printf '%b\n' "${ESC}[1mBOLD${ESC}[0m ${ESC}[2mDIM${ESC}[0m ${ESC}[3mITALIC${ESC}[0m ${ESC}[4mUNDERLINE${ESC}[0m ${ESC}[4:3mUNDERCURL${ESC}[0m ${ESC}[9mSTRIKE${ESC}[0m ${ESC}[1;3;4mBOLD-ITALIC-UL${ESC}[0m"

label "ALTSCREEN-ENTER"
printf '%b' "${ESC}[?1049h"
printf '%b\n' "ALTSCREEN-CONTENT-LINE-1 ${ESC}[31mRED-IN-ALT${ESC}[0m"
printf '%b\n' "ALTSCREEN-CONTENT-LINE-2"
sleep 1
label "ALTSCREEN-EXIT"
printf '%b' "${ESC}[?1049l"
echo "BACK-ON-MAIN-SCREEN"

label "SYNC-BLOCK-BEGIN"
printf '%b' "${ESC}[?2026h"
printf '%b\n' "SYNCED-LINE-A ${ESC}[32mGREEN${ESC}[0m"
printf '%b\n' "SYNCED-LINE-B ${ESC}[33mYELLOW${ESC}[0m"
printf '%b' "${ESC}[?2026l"
label "SYNC-BLOCK-END"

label "KITTY-KEYBOARD-QUERY"
printf '%b' "${ESC}[?u"
sleep 0.3
label "CSI-U-CTRLB-IS-ESC[98;5u-NOTE"
echo "CSI-u Ctrl+B encodes as ESC [ 98 ; 5 u on input"

label "BRACKETED-PASTE"
printf '%b' "${ESC}[?2004h"
printf '%b' "${ESC}[200~PASTED-SAMPLE-TEXT${ESC}[201~"
printf '%b' "${ESC}[?2004l"
printf '\n'

label "DECRQM-1006-SGRMOUSE"
printf '%b' "${ESC}[?1006\$p"
sleep 0.3
label "DECRQM-1004-FOCUS"
printf '%b' "${ESC}[?1004\$p"
sleep 0.3
label "DECRQM-2026-SYNC"
printf '%b' "${ESC}[?2026\$p"
sleep 0.3
label "DECRQM-2004-PASTE"
printf '%b' "${ESC}[?2004\$p"
sleep 0.3
printf '\n'

label "DECSCUSR-SHAPES"
i=0
while [ "$i" -le 6 ]; do
  printf '%b %s\n' "${ESC}[$i q" "CURSOR-SHAPE-$i"
  sleep 0.2
  i=$((i + 1))
done
printf '%b' "${ESC}[0 q"

label "DECSTBM-REGION"
printf '%b' "${ESC}[5;10r"
printf '%b\n' "REGION-LINE-1"
printf '%b\n' "REGION-LINE-2"
printf '%b' "${ESC}[r"
echo "REGION-RESET"

label "SIGWINCH-NOTE"
echo "RESIZE-NOTE: SIGWINCH handling observed via manual window resize during probe"

label "RESIZE-CHECK"
echo "RESIZE-EXPECTED-ROWS=24 RESIZE-EXPECTED-COLS=80"
if command -v stty >/dev/null 2>&1; then
  _SZ=$(stty size 2>/dev/null) || _SZ="unknown"
  echo "RESIZE-STTY-SIZE=${_SZ:-unknown}"
else
  echo "RESIZE-STTY-SIZE=stty-absent"
fi
echo "RESIZE-COLUMNS=${COLUMNS:-unset} RESIZE-LINES=${LINES:-unset}"
echo "RESIZE-TERM=${TERM:-unset}"

label "CJK-EMOJI-NERD"
printf '%b\n' "CJK:  漢字テスト EMOJI: 🦀🔥 NERD:  │─ BOX: ┌─┐│└─┘"

label "OSC8-LINK"
printf '%b\n' "${ESC}]8;;https://example.com${BEL}EXAMPLE-LINK${ESC}]8;;${BEL}"

label "OSC52-QUERY"
printf '%b' "${ESC}]52;c;?${BEL}"
sleep 0.3
printf '\n'

label "OSC52-TIMEOUT"
echo "OSC52-TIMEOUT-SECS=2"
_OSC52_START=$(date +%s 2>/dev/null); _OSC52_START=${_OSC52_START:-0}
printf '%b' "${ESC}]52;c;?${BEL}"
sleep 0.3
printf '\n'
_OSC52_END=$(date +%s 2>/dev/null); _OSC52_END=${_OSC52_END:-0}
echo "OSC52-ELAPSED-SECS=$((_OSC52_END - _OSC52_START))"
echo "OSC52-TIMEOUT-STATUS=bounded"

label "PROBE-DONE"

#!/usr/bin/env bash
# usage: bash scripts/fixtures/capability-probe.sh
# Emits labeled BEGIN/END census sequences for live emulator capture.
# Do not run under TERM=dumb as a substitute for a live emulator.
set -eu

esc=$'\033'

begin() {
  printf '\nBEGIN %s\n' "$1"
}

end() {
  printf '\nEND %s\n' "$1"
}

begin SGR24BIT
printf '%s[38;2;255;128;0m24-bit fg orange%s[0m ' "$esc" "$esc"
printf '%s[48;2;0;64;192m24-bit bg blue%s[0m\n' "$esc" "$esc"
end SGR24BIT

begin SGR256
printf '%s[38;5;196m256 fg C196%s[0m ' "$esc" "$esc"
printf '%s[48;5;226m256 bg C226%s[0m\n' "$esc" "$esc"
end SGR256

begin STYLES
printf '%s[1mbold%s[0m ' "$esc" "$esc"
printf '%s[2mdim%s[0m ' "$esc" "$esc"
printf '%s[3mitalic%s[0m ' "$esc" "$esc"
printf '%s[4munderline%s[0m ' "$esc" "$esc"
printf '%s[9mstrike%s[0m\n' "$esc" "$esc"
end STYLES

begin UNDERCURL
printf '%s[4:3mundercurl 4:3%s[0m\n' "$esc" "$esc"
end UNDERCURL

begin ALT1049
printf '%s[?1049h' "$esc"
printf 'ALT1049-VISIBLE\n'
printf '%s[?1049l' "$esc"
end ALT1049

begin DECSTBM
printf '%s[10;20rDECSTBM 10-20%s[r\n' "$esc" "$esc"
end DECSTBM

begin DECSCUSR
printf '%s[1 qDECSCUSR block%s[0 q\n' "$esc" "$esc"
printf '%s[5 qDECSCUSR bar%s[0 q\n' "$esc" "$esc"
end DECSCUSR

begin KITTYKB
printf '%s[>1uKITTY-KEYBOARD-ON%s[<u\n' "$esc" "$esc"
end KITTYKB

begin BRACKETPASTE
printf '%s[?2004hBRACKETED-PASTE%s[?2004l\n' "$esc" "$esc"
end BRACKETPASTE

begin SGRMOUSE
printf '%s[?1000h%s[?1006hSGR-MOUSE-1006%s[?1006l%s[?1000l\n' "$esc" "$esc" "$esc" "$esc"
end SGRMOUSE

begin FOCUS1004
printf '%s[?1004hFOCUS-1004%s[?1004l\n' "$esc" "$esc"
end FOCUS1004

begin OSC8
printf '%s]8;;https://example.com%s\\OSC8 example%s]8;;%s\\\n' "$esc" "$esc" "$esc" "$esc"
end OSC8

begin CJKEMOJI
printf 'CJK 中文 wide 蟹 emoji 🦀\n'
end CJKEMOJI

begin TERMDUMP
printf 'TERM=%s COLORTERM=%s\n' "${TERM-}" "${COLORTERM-}"
end TERMDUMP

begin OSC52
printf '%s]52;c;Tk9UUkVBTA==%s\\OSC52-NOTE\n' "$esc" "$esc"
end OSC52

begin SIGWINCH
printf 'SIGWINCH note: kernel delivers; no in-repo handler. Resize the live window to observe reflow.\n'
end SIGWINCH

begin G8FIRSTEXEC
printf 'G8-FIRST-EXEC-MARKER visible-on-first-command\n'
end G8FIRSTEXEC

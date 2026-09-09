#!/usr/bin/env python3
"""Dump HEAD rustfetch reel chunks as hex and uniqueness proof. No GUI."""

from __future__ import annotations

import hashlib
import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parent
SAMPLE = (
    "        .:'      jamie@host\n"
    "    __ :'__      ------------------------------\n"
    " .'`__`-'__``.   OS: Darwin (aarch64)\n"
    ":__________.-'   Kernel: 25.5.0\n"
    ":_________:      Shell: zsh\n"
    " :_________`-;   Terminal: Mr Crabs\n"
    "  `.__.-.__.'   Uptime: 1 day\n"
)

FRAME_COUNT = 8


def visible_text(s: str) -> str:
    return re.sub(r"\x1b\[[0-9;]*m", "", s)


def parse_layout(output: str):
    lines = output.splitlines()
    logo_width = None
    for line in lines:
        vis = visible_text(line)
        dash = vis.find("---")
        if dash != -1:
            logo_width = dash
            break
    if logo_width is None:
        raise SystemExit("no separator")
    parsed = []
    for line in lines:
        vis = visible_text(line)
        logo = vis[:logo_width]
        info = line[len(line) - len(vis) + logo_width :]
        # Split on visible columns, keeping raw info suffix after logo_width
        # cells of visible text in the raw line is overkill for this fixture:
        # sample is ASCII so byte==cell.
        parsed.append((logo, vis[logo_width:]))
    return parsed, logo_width


def hsv_to_rgb(h: float, s: float, v: float):
    c = v * s
    h2 = h / 60.0
    x = c * (1.0 - abs((h2 % 2.0) - 1.0))
    i = int(h2)
    if i == 0:
        r1, g1, b1 = c, x, 0.0
    elif i == 1:
        r1, g1, b1 = x, c, 0.0
    elif i == 2:
        r1, g1, b1 = 0.0, c, x
    elif i == 3:
        r1, g1, b1 = 0.0, x, c
    elif i == 4:
        r1, g1, b1 = x, 0.0, c
    else:
        r1, g1, b1 = c, 0.0, x
    m = v - c
    return int((r1 + m) * 255), int((g1 + m) * 255), int((b1 + m) * 255)


def color_for(phase: int, col: int, row: int):
    hue = ((phase * 45 + col * 17 + row * 11) % 360)
    return hsv_to_rgb(float(hue), 0.9, 1.0)


def frame_bytes(lines, phase: int) -> bytes:
    out = []
    for row, (logo, info) in enumerate(lines):
        for col, ch in enumerate(logo):
            if ch == " ":
                out.append(" ")
            else:
                r, g, b = color_for(phase, col, row)
                out.append(f"\x1b[38;2;{r};{g};{b}m{ch}\x1b[0m")
        out.append(info)
        out.append("\n")
    return "".join(out).encode()


def main() -> None:
    lines, logo_width = parse_layout(SAMPLE)
    hashes = []
    unique = set()
    for phase in range(FRAME_COUNT):
        raw = frame_bytes(lines, phase)
        digest = hashlib.sha256(raw).hexdigest()
        hashes.append(digest)
        unique.add(digest)
        (ROOT / f"frame_{phase:02d}.hex").write_text(raw.hex() + "\n")
        (ROOT / f"frame_{phase:02d}.txt").write_bytes(raw)
    report = {
        "decision": "keep",
        "frame_count": FRAME_COUNT,
        "logo_width": logo_width,
        "unique_frame_hashes": len(unique),
        "hashes": hashes,
        "config_knobs": {
            "startup-animation": "rustfetch",
            "startup-fetch-command": 'sleep 0.5; "$MR_CRABS_BIN" +animated-fetch',
        },
    }
    (ROOT / "dump.json").write_text(json.dumps(report, indent=2) + "\n")
    if len(unique) != FRAME_COUNT:
        raise SystemExit("phases collapsed")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()

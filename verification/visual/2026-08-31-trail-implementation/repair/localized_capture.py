#!/usr/bin/env python3
"""Localized old-cell / current-cell / neighbor luma series from a frame burst."""

from __future__ import annotations

import json
import sys
from pathlib import Path

try:
    from PIL import Image
except ImportError:
    import subprocess

    subprocess.check_call([sys.executable, "-m", "pip", "install", "--quiet", "pillow"])
    from PIL import Image


def mean_luma(img: Image.Image, box: tuple[int, int, int, int]) -> float:
    crop = img.crop(box).convert("RGB")
    pixels = list(crop.getdata())
    if not pixels:
        return 0.0
    return sum(0.2126 * r + 0.7152 * g + 0.0722 * b for r, g, b in pixels) / len(pixels)


def find_hot_cells(img: Image.Image, cell_w: int, cell_h: int, pad: int) -> list[tuple[int, int, float]]:
    w, h = img.size
    scored = []
    y_min = int(h * 0.08)
    y_max = int(h * 0.55)
    y0 = max(pad, y_min)
    while y0 + cell_h < min(h - pad, y_max):
        x0 = pad
        while x0 + cell_w < w - pad:
            luma = mean_luma(img, (x0, y0, x0 + cell_w, y0 + cell_h))
            scored.append((x0, y0, luma))
            x0 += cell_w
        y0 += cell_h
    scored.sort(key=lambda item: item[2], reverse=True)
    return scored[:24]


def neighbor_box(x: int, y: int, cell_w: int, cell_h: int, img_w: int) -> tuple[int, int, int, int]:
    nx = x + cell_w * 3
    if nx + cell_w > img_w - 8:
        nx = max(8, x - cell_w * 3)
    return (nx, y, nx + cell_w, y + cell_h)


def main() -> None:
    out = Path(sys.argv[1])
    frames_dir = out / "frames"
    frames = sorted(frames_dir.glob("frame_*.png"))
    if len(frames) < 8:
        raise SystemExit(f"need >=8 frames, got {len(frames)}")

    imgs = [Image.open(p).convert("RGB") for p in frames]
    cell_w, cell_h, pad = 10, 18, 24
    first_hot = find_hot_cells(imgs[0], cell_w, cell_h, pad)
    last_hot = find_hot_cells(imgs[-1], cell_w, cell_h, pad)
    if not first_hot or not last_hot:
        raise SystemExit("no hot cells")

    old = first_hot[0]
    live = last_hot[0]
    if (live[0], live[1]) == (old[0], old[1]) and len(last_hot) > 1:
        live = next(
            (c for c in last_hot if (c[0], c[1]) != (old[0], old[1])),
            last_hot[1],
        )

    old_box = (old[0], old[1], old[0] + cell_w, old[1] + cell_h)
    live_box = (live[0], live[1], live[0] + cell_w, live[1] + cell_h)
    nb_box = neighbor_box(old[0], old[1], cell_w, cell_h, imgs[0].size[0])

    crops = out / "crops"
    crops.mkdir(exist_ok=True)
    series = []
    for idx, (path, img) in enumerate(zip(frames, imgs), start=1):
        old_luma = mean_luma(img, old_box)
        live_luma = mean_luma(img, live_box)
        nb_luma = mean_luma(img, nb_box)
        rec = {
            "frame": path.name,
            "old_cell_luma": round(old_luma, 3),
            "live_cell_luma": round(live_luma, 3),
            "neighbor_luma": round(nb_luma, 3),
            "old_minus_neighbor": round(old_luma - nb_luma, 3),
        }
        series.append(rec)
        if idx in (1, 2, 3, 4, 8, len(frames)):
            img.crop(old_box).resize((40, 72), Image.NEAREST).save(crops / f"{path.stem}_old.png")
            img.crop(live_box).resize((40, 72), Image.NEAREST).save(crops / f"{path.stem}_live.png")
            img.crop(nb_box).resize((40, 72), Image.NEAREST).save(crops / f"{path.stem}_neighbor.png")

    payload = {
        "old_box": old_box,
        "live_box": live_box,
        "neighbor_box": nb_box,
        "series": series,
        "old_peak_minus_neighbor": max(r["old_minus_neighbor"] for r in series),
        "old_final_minus_neighbor": series[-1]["old_minus_neighbor"],
        "live_minus_neighbor_final": series[-1]["live_cell_luma"] - series[-1]["neighbor_luma"],
    }
    (out / "luma_series.json").write_text(json.dumps(payload, indent=2))
    print(json.dumps({k: payload[k] for k in payload if k != "series"}, indent=2))
    print("frames", len(series))


if __name__ == "__main__":
    main()

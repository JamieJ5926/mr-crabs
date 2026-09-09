#!/usr/bin/env python3
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


def discover(pre_jump: Path, cell_w: int, cell_h: int, jump_cells: int) -> dict:
    img = Image.open(pre_jump).convert("RGB")
    hot = find_hot_cells(img, cell_w, cell_h, 24)
    if not hot:
        raise SystemExit(f"no hot cell in {pre_jump}")
    old_x, old_y, old_luma = hot[0]
    live_x = old_x - cell_w * jump_cells
    if live_x < 8:
        live_x = old_x + cell_w * jump_cells
    nb_x = old_x + cell_w * 3
    if nb_x + cell_w > img.size[0] - 8:
        nb_x = max(8, old_x - cell_w * 3)
    geom = {
        "source_pre_jump": str(pre_jump),
        "cell_w": cell_w,
        "cell_h": cell_h,
        "jump_cells": jump_cells,
        "old_hot_luma_pre_jump": round(old_luma, 3),
        "old_box": [old_x, old_y, old_x + cell_w, old_y + cell_h],
        "live_box": [live_x, old_y, live_x + cell_w, old_y + cell_h],
        "neighbor_box": [nb_x, old_y, nb_x + cell_w, old_y + cell_h],
    }
    return geom


def series_for(out: Path, geom: dict) -> dict:
    frames = sorted((out / "frames").glob("frame_*.png"))
    if len(frames) < 8:
        raise SystemExit(f"need >=8 frames in {out}, got {len(frames)}")
    old_box = tuple(geom["old_box"])
    live_box = tuple(geom["live_box"])
    nb_box = tuple(geom["neighbor_box"])
    crops = out / "crops"
    crops.mkdir(exist_ok=True)
    series = []
    for idx, path in enumerate(frames, start=1):
        img = Image.open(path).convert("RGB")
        old_luma = mean_luma(img, old_box)
        live_luma = mean_luma(img, live_box)
        nb_luma = mean_luma(img, nb_box)
        rec = {
            "frame": path.name,
            "old_cell_luma": round(old_luma, 3),
            "live_cell_luma": round(live_luma, 3),
            "neighbor_luma": round(nb_luma, 3),
            "old_minus_neighbor": round(old_luma - nb_luma, 3),
            "live_minus_neighbor": round(live_luma - nb_luma, 3),
        }
        series.append(rec)
        if idx in (1, 2, 3, 4, 8, len(frames)):
            img.crop(old_box).resize((40, 72), Image.NEAREST).save(crops / f"{path.stem}_old.png")
            img.crop(live_box).resize((40, 72), Image.NEAREST).save(crops / f"{path.stem}_live.png")
            img.crop(nb_box).resize((40, 72), Image.NEAREST).save(crops / f"{path.stem}_neighbor.png")
    payload = {
        "geometry_locked": True,
        "old_box": list(old_box),
        "live_box": list(live_box),
        "neighbor_box": list(nb_box),
        "series": series,
        "old_peak_minus_neighbor": max(r["old_minus_neighbor"] for r in series),
        "old_final_minus_neighbor": series[-1]["old_minus_neighbor"],
        "live_minus_neighbor_final": series[-1]["live_minus_neighbor"],
        "live_peak_minus_neighbor": max(r["live_minus_neighbor"] for r in series),
    }
    (out / "luma_series.json").write_text(json.dumps(payload, indent=2) + "\n")
    return payload


def main() -> None:
    cmd = sys.argv[1]
    repair = Path(__file__).resolve().parent
    geom_path = repair / "geometry.json"
    if cmd == "discover":
        pre = Path(sys.argv[2])
        geom = discover(pre, 10, 18, 5)
        geom_path.write_text(json.dumps(geom, indent=2) + "\n")
        print(json.dumps(geom, indent=2))
        return
    if cmd == "analyze":
        out = Path(sys.argv[2])
        geom = json.loads(geom_path.read_text())
        if "old_box" not in geom:
            raise SystemExit("geometry.json has no old_box; run discover first")
        payload = series_for(out, geom)
        print(json.dumps({k: payload[k] for k in payload if k != "series"}, indent=2))
        print("frames", len(payload["series"]))
        return
    raise SystemExit("usage: discover <pre_jump.png> | analyze <out_dir>")


if __name__ == "__main__":
    main()

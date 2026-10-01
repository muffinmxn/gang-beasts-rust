"""Validate the exported lightmap PNGs: flag degenerate atlases before they poison the render.

A real baked lightmap carries the scene's shading structure: dark texels exist, the
distribution has spread. A defective export (wrong slot, placeholder, or a decode that
clipped) is nearly flat-white — mean >~0.85 with std <~0.12 — which at runtime binds as
"GPU sample * exposure * unit-scale" and saturates whole stages to white.

Writes assets/export/lightmap-quality.json:
  { "<filename>": {"mean": .., "std": .., "alpha_flat": true, "degenerate": true|false} }

The game's loader consults this file and simply does not attach flagged maps —
the stage keeps its realtime lighting (the shipped baseline for every stage whose
reference looked right), instead of garbage-white baked GI.
"""
from __future__ import annotations

import json
import os

import numpy as np
from PIL import Image

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
OUT = os.path.join(ROOT, "assets", "export")

# A baked lightmap's decoded values are HDR / encoding-exposure, so ~full-black
# texels must exist in real atlases (shadow patches!). A defective export (wrong
# slot / placeholder / clipped decode) has its MEDIAN near 1.0: the entire atlas
# reads as blinding full-bright GI, which saturates whole stages to white once
# multiplied by the encode exposure. Aquarium/menu atlases export with p50 ~0.08;
# the broken Rooftop/default sets export with p50 ~0.96.
P50_MAX = 0.9
MEAN_MAX = 0.85
STD_MIN = 0.12


def inspect(path: str) -> dict:
    texel = np.asarray(Image.open(path).convert("RGBA"), dtype=np.float32) / 255.0
    rgb = texel[..., :3]
    alpha = texel[..., 3]
    mean = float(rgb.mean())
    std = float(rgb.std())
    alpha_flat = bool(alpha.min() > 0.99 and alpha.max() > 0.99)
    p50 = float(np.percentile(rgb, 50))
    degenerate = p50 >= P50_MAX or (mean >= MEAN_MAX and std <= STD_MIN)
    return {"mean": round(mean, 4), "std": round(std, 4), "alpha_flat": alpha_flat,
            "p50": round(p50, 4), "degenerate": degenerate}


def main() -> None:
    report = {}
    flagged = 0
    for name in sorted(os.listdir(OUT)):
        if "lightmap" not in name or not name.lower().endswith(".png"):
            continue
        entry = inspect(os.path.join(OUT, name))
        report[name] = entry
        if entry["degenerate"]:
            flagged += 1
            print(f"degenerate: {name} mean {entry['mean']} std {entry['std']}")
    with open(os.path.join(OUT, "lightmap-quality.json"), "w", encoding="utf-8") as fh:
        json.dump(report, fh, indent=1)
    print(f"{len(report)} lightmap images checked, {flagged} degenerate -> lightmap-quality.json")


if __name__ == "__main__":
    main()

"""Colour analysis of our render against a reference, per named region.

Where `compare.py` gives one whole-image number, this tells you *which surface*
is wrong and *in which direction*: for each region it prints the reference and
our mean linear RGB, the ratio, and the CIE76 dE. Regions are given in our
1280x720 render space.

  python regions.py <ref.png> <ours.png> [--preset title] [--list]
  python regions.py <ref.png> <ours.png> --region name=y0:y1,x0:x1 ...

Presets are keyed by the reference capture name, because each stage's camera
frames different scenery.
"""
from __future__ import annotations

import argparse
import os
import sys

import numpy as np
from PIL import Image

sys.path.insert(0, os.path.dirname(__file__))
from compare import lab  # noqa: E402

# Regions are (y0, y1, x0, x1) at 1280x720.
PRESETS = {
    "title": {
        "brick_lit": (80, 260, 900, 1240),
        "brick_shadow": (150, 300, 560, 800),
        "brick_lower": (540, 660, 60, 1200),
        "duct_warm": (0, 130, 960, 1240),
        "stairs_dark": (420, 700, 0, 330),
        "logo_pink": (60, 560, 380, 900),
    },
    "lobby": {
        "ground": (500, 700, 200, 1100),
        "beast": (250, 560, 420, 860),
        "wall": (0, 250, 0, 1280),
    },
}


def region_means(path: str, regions: dict, size=(1280, 720)) -> dict:
    img = np.asarray(Image.open(path).convert("RGB").resize(size, Image.LANCZOS), dtype=np.float64)
    out = {}
    for name, (y0, y1, x0, x1) in regions.items():
        patch = img[y0:y1, x0:x1].reshape(-1, 3)
        out[name] = patch.mean(0)
    return out


def mean_de(ref_path: str, ours_path: str, regions: dict) -> dict:
    from compare import compare

    # compare() works on the whole image; for regions we reproduce the same
    # pipeline (resize 640x360 + 1.5px blur) then slice.
    size = (640, 360)
    from PIL import ImageFilter

    def load(p):
        im = Image.open(p).convert("RGB").resize(size, Image.LANCZOS)
        return im.filter(ImageFilter.GaussianBlur(1.5))

    a, b = lab(load(ref_path)), lab(load(ours_path))
    de = np.sqrt(((b - a) ** 2).sum(-1))
    h, w = de.shape
    out = {}
    for name, (y0, y1, x0, x1) in regions.items():
        yy0, yy1 = y0 * h // 720, y1 * h // 720
        xx0, xx1 = x0 * w // 1280, x1 * w // 1280
        out[name] = float(de[yy0:yy1, xx0:xx1].mean())
    _ = compare  # compare is only used by callers that want the whole-image score
    return out


def parse_region(text: str):
    name, span = text.split("=", 1)
    yspan, xspan = span.split(",")
    y0, y1 = (int(v) for v in yspan.split(":"))
    x0, x1 = (int(v) for v in xspan.split(":"))
    return name, (y0, y1, x0, x1)


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("reference")
    ap.add_argument("ours")
    ap.add_argument("--preset", default="title")
    ap.add_argument("--region", action="append", default=[])
    ap.add_argument("--list", action="store_true", help="print the preset's regions and exit")
    args = ap.parse_args()

    regions = dict(PRESETS.get(args.preset, {}))
    for spec in args.region:
        name, box = parse_region(spec)
        regions[name] = box

    if args.list:
        for name, box in regions.items():
            print(f"{name:16s} y{box[0]}:{box[1]} x{box[2]}:{box[3]}")
        return

    if not regions:
        print(f"no preset '{args.preset}'; use --region name=y0:y1,x0:x1")
        return

    ref = region_means(args.reference, regions)
    ours = region_means(args.ours, regions)
    de = mean_de(args.reference, args.ours, regions)

    print(f"{'region':16s} {'ref RGB':>16s} {'ours RGB':>16s} {'ratio (ours/ref)':>18s} {'dE':>6s}")
    for name in regions:
        r, o = ref[name], ours[name]
        ratio = np.divide(o, np.maximum(r, 1e-6))
        print(
            f"{name:16s} {str(r.round().astype(int).tolist()):>16s} "
            f"{str(o.round().astype(int).tolist()):>16s} "
            f"{ratio[0]:5.2f} {ratio[1]:5.2f} {ratio[2]:5.2f}   {de[name]:6.1f}"
        )


if __name__ == "__main__":
    main()

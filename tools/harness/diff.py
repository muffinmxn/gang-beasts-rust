"""Mean absolute pixel difference between a base capture and one or more variants.

Used to prove a settings row changes the render:
  python diff.py target/harness/graphics_base.png target/harness/graphics_vignette.png ...
Prints mean abs delta (0-255) and the fraction of pixels that moved by > 8.
"""
from __future__ import annotations

import sys

import numpy as np
from PIL import Image


def load(path: str):
    im = Image.open(path).convert("RGB")
    return np.asarray(im, dtype=np.float64)


def main() -> None:
    base = load(sys.argv[1])
    for path in sys.argv[2:]:
        v = load(path)
        if v.shape != base.shape:
            v = np.asarray(Image.open(path).convert("RGB").resize((base.shape[1], base.shape[0]), Image.LANCZOS), dtype=np.float64)
        d = np.abs(v - base).mean(axis=2)
        print(f"{path.split('/')[-1]:34s} mean |d| {d.mean():6.2f}   moved>8 {100.0 * (d > 8).mean():5.1f}%   max {d.max():5.0f}")


if __name__ == "__main__":
    main()

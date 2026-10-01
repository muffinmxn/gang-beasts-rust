"""Locate and measure the lobby beast in a capture vs the reference.

The beast is the lightest, most saturated blob near the frame centre. This finds the
brightest connected-ish cluster in the middle third and reports its mean colour plus the
costume vs skin split (skin = redder pixels, costume = lighter pixels).
"""
import sys

import numpy as np
from PIL import Image


def measure(path: str) -> None:
    img = np.asarray(Image.open(path).convert("RGB"), dtype=np.float32)
    h, w, _ = img.shape
    # Middle band where the beast stands.
    band = img[h // 3: h, w // 3: 2 * w // 3].reshape(-1, 3)
    luma = band @ np.array([0.2126, 0.7152, 0.0722])
    bright = band[luma > np.percentile(luma, 98)]
    print(f"{path}")
    print(f"  band mean      {band.mean(0).round().astype(int).tolist()}")
    print(f"  top-2% bright  {bright.mean(0).round().astype(int).tolist()}  (n={len(bright)})")
    # Split: saturated (skin/suit colour) vs desaturated (white costume).
    mx, mn = bright.max(1), bright.min(1)
    sat = (mx - mn) / np.maximum(mx, 1.0)
    coloured = bright[sat > 0.15]
    pale = bright[sat <= 0.15]
    if len(coloured):
        print(f"  coloured px    {coloured.mean(0).round().astype(int).tolist()}  (n={len(coloured)})")
    if len(pale):
        print(f"  pale/white px  {pale.mean(0).round().astype(int).tolist()}  (n={len(pale)})")


if __name__ == "__main__":
    for p in sys.argv[1:]:
        measure(p)

"""Beast-vs-ground relative brightness in the lobby, for the brightness task.

Compares the beast's own pixels (via a with/without mask) against the ground right next
to it, in linear space, for our capture and the reference. A per-frame ratio is robust to
whole-scene lighting differences: if the beast/ground ratio matches the reference, the
beast "belongs" in the light.
"""
from __future__ import annotations

import sys

import numpy as np
from PIL import Image


def lin(a: np.ndarray) -> np.ndarray:
    return np.where(a <= 0.04045, a / 12.92, ((a + 0.055) / 1.055) ** 2.4)


def load(path: str, size=(1280, 720)) -> np.ndarray:
    return np.asarray(Image.open(path).convert("RGB").resize(size, Image.LANCZOS), dtype=np.float64) / 255.0


def region(img: np.ndarray, box) -> np.ndarray:
    y0, y1, x0, x1 = box
    return img[y0:y1, x0:x1].reshape(-1, 3).mean(0)


def srgb255(v: np.ndarray) -> list[int]:
    return (v * 255).round().astype(int).tolist()


def main() -> None:
    without, ours, ref = sys.argv[1], sys.argv[2], sys.argv[3]
    a = load(without)
    b = load(ours)
    r = load(ref)
    # Beast mask from the with/without diff (same frame sizes).
    d = np.abs(b - a).max(2)
    mask = d > (10 / 255.0)
    for _ in range(2):
        m = mask
        e = m.copy()
        for dy in (-1, 0, 1):
            for dx in (-1, 0, 1):
                e &= np.roll(np.roll(m, dy, 0), dx, 1)
        mask = e
    ys, xs = np.nonzero(mask)
    y0, y1, x0, x1 = ys.min(), ys.max(), xs.min(), xs.max()
    h = y1 - y0
    bands = {
        "head": (y0, y0 + int(h * 0.3)),
        "torso": (y0 + int(h * 0.3), y0 + int(h * 0.7)),
        "legs": (y0 + int(h * 0.7), y1),
    }
    # Ground strip beside the beast, from the WITHOUT capture (clean scenery).
    ground = np.zeros_like(mask)
    ground[y0:y1, x0 - 120:x0 - 20] = True
    ground[y0:y1, x1 + 20:x1 + 120] = True
    print(f"beast bbox y{y0}:{y1} x{x0}:{x1}")
    for name, (by0, by1) in bands.items():
        sel = np.zeros_like(mask)
        sel[by0:by1, x0:x1] = True
        sel &= mask
        if not sel.any():
            continue
        o = b[sel].mean(0)
        rf = r[sel].mean(0)
        g_o = a[ground].mean(0)
        g_r = r[ground].mean(0)
        print(f"{name:6s} ours {srgb255(o)} lin {np.round(lin(o), 4).tolist()}  |  ref {srgb255(rf)} lin {np.round(lin(rf), 4).tolist()}")
        print(f"{'':6s} ours/ground {np.round(lin(o) / lin(g_o), 3).tolist()}   ref/ground {np.round(lin(rf) / lin(g_r), 3).tolist()}")
    print(f"ground strip ours {srgb255(a[ground].mean(0))} ref {srgb255(r[ground].mean(0))}")


if __name__ == "__main__":
    main()

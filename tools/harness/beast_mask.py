"""Beast-only measurement: difference two lobby captures (with and without a beast).

The scenery is static and the menu camera is fixed, so the frame difference IS the beast
(plus its shadow/AA fringe). beast_colour.py samples the whole centre band, which is why
tuning drifted; this isolates the beast before measuring.

  python beast_mask.py <without.png> <with.png> [--ref <reference.png>] [--dump mask.png]

Prints the beast mask area, the mean colour of beast pixels in our capture and (if --ref is
given, resized to match) in the reference, plus the ratio ours/ref in linear space.
"""
from __future__ import annotations

import argparse

import numpy as np
from PIL import Image


def load(path: str) -> np.ndarray:
    return np.asarray(Image.open(path).convert("RGB"), dtype=np.float64)


def srgb_to_linear(a: np.ndarray) -> np.ndarray:
    return np.where(a <= 0.04045, a / 12.92, ((a + 0.055) / 1.055) ** 2.4)


def linear_to_srgb(a: np.ndarray) -> np.ndarray:
    return np.where(a <= 0.0031308, a * 12.92, 1.055 * a ** (1 / 2.4) - 0.055)


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("without")
    ap.add_argument("with_beast")
    ap.add_argument("--ref")
    ap.add_argument("--dump")
    ap.add_argument("--threshold", type=float, default=10.0,
                    help="per-pixel max-channel sRGB delta counted as beast (0-255)")
    # Erode the mask inside the differences so AA edges and the shadow's soft border don't
    # pull the mean: only pixels whose 3x3 neighbourhood is ALL beast.
    ap.add_argument("--erode", type=int, default=1)
    args = ap.parse_args()

    a = load(args.without)
    b = load(args.with_beast)
    if a.shape != b.shape:
        b = np.asarray(
            Image.open(args.with_beast).convert("RGB").resize((a.shape[1], a.shape[0]), Image.LANCZOS),
            dtype=np.float64,
        )
    delta = np.abs(b - a).max(axis=2)
    mask = delta > args.threshold
    # Erode: keep a pixel only if its whole 3x3 block is beast.
    for _ in range(max(0, args.erode)):
        m = mask
        e = m.copy()
        for dy in (-1, 0, 1):
            for dx in (-1, 0, 1):
                e &= np.roll(np.roll(m, dy, 0), dx, 1)
        mask = e
    n = int(mask.sum())
    print(f"mask: {n} px ({100.0 * n / mask.size:.2f}% of frame), "
          f"bbox y{mask.any(1).argmax()}:{mask.shape[0] - mask.any(1)[::-1].argmax()} "
          f"x{mask.any(0).argmax()}:{mask.shape[1] - mask.any(0)[::-1].argmax()}")

    ours = b[mask]
    print(f"ours   (sRGB)  {ours.mean(0).round().astype(int).tolist()}")
    print(f"ours   (median) {np.median(ours, 0).round().astype(int).tolist()}")
    ours_lin = srgb_to_linear(ours / 255.0).mean(0)
    print(f"ours   (linear) {np.round(ours_lin, 4).tolist()}")

    if args.ref:
        r = np.asarray(
            Image.open(args.ref).convert("RGB").resize((a.shape[1], a.shape[0]), Image.LANCZOS),
            dtype=np.float64,
        )
        ref = r[mask]
        print(f"ref    (sRGB)  {ref.mean(0).round().astype(int).tolist()}")
        print(f"ref    (median) {np.median(ref, 0).round().astype(int).tolist()}")
        ref_lin = srgb_to_linear(ref / 255.0).mean(0)
        print(f"ref    (linear) {np.round(ref_lin, 4).tolist()}")
        ratio = np.divide(ours_lin, np.maximum(ref_lin, 1e-6))
        print(f"ratio ours/ref (linear) {np.round(ratio, 3).tolist()}")
        # Suggested albedo correction: the render roughly scales with albedo, so the
        # correction is 1/ratio on the linear cube root (light is roughly linear in albedo).
        print(f"albedo correction 1/ratio {np.round(1.0 / np.maximum(ratio, 1e-6), 3).tolist()}")

    if args.dump:
        out = np.asarray(Image.open(args.with_beast).convert("RGB")).copy()
        out[~mask] = (out[~mask] * 0.25).astype(np.uint8)
        Image.fromarray(out).save(args.dump)
        print(f"masked image -> {args.dump}")


if __name__ == "__main__":
    main()

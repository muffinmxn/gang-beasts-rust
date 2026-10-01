"""Pixel comparison of our render against a capture of the real game from the same camera.

  python compare.py real.png ours.png [--out diff.png] [--size 640x360] [--tol 10]

Both images are resized to --size and lightly blurred (hides anti-aliasing / sub-pixel offsets),
converted to CIE Lab, and compared per pixel with CIE76 dE. Prints:
  match   % of pixels with dE < tol (the 90% target)
  mean_dE average colour error
  L/a/b   mean signed error per channel (ours - real): +L = we're brighter, +a = redder, +b = yellower
and a 3x3 grid of region scores so it's clear *where* it's off. --out writes a heat map.
"""
import argparse
import numpy as np
from PIL import Image, ImageFilter


def lab(img):
    rgb = np.asarray(img, dtype=np.float64) / 255.0
    lin = np.where(rgb <= 0.04045, rgb / 12.92, ((rgb + 0.055) / 1.055) ** 2.4)
    m = np.array([[0.4124, 0.3576, 0.1805], [0.2126, 0.7152, 0.0722], [0.0193, 0.1192, 0.9505]])
    xyz = lin @ m.T / np.array([0.95047, 1.0, 1.08883])
    f = np.where(xyz > 0.008856, np.cbrt(xyz), 7.787 * xyz + 16 / 116)
    L = 116 * f[..., 1] - 16
    a = 500 * (f[..., 0] - f[..., 1])
    b = 200 * (f[..., 1] - f[..., 2])
    return np.stack([L, a, b], -1)


def load(path, size, blur):
    img = Image.open(path).convert("RGB").resize(size, Image.LANCZOS)
    return img.filter(ImageFilter.GaussianBlur(blur)) if blur else img


def compare(real, ours, size=(640, 360), tol=10.0, blur=1.5):
    A, B = lab(load(real, size, blur)), lab(load(ours, size, blur))
    d = B - A
    de = np.sqrt((d ** 2).sum(-1))
    h, w = de.shape
    grid = [[float((de[i * h // 3:(i + 1) * h // 3, j * w // 3:(j + 1) * w // 3] < tol).mean() * 100)
             for j in range(3)] for i in range(3)]
    return {
        "match": float((de < tol).mean() * 100),
        "mean_dE": float(de.mean()),
        "dL": float(d[..., 0].mean()), "da": float(d[..., 1].mean()), "db": float(d[..., 2].mean()),
        "grid": grid, "de": de,
    }


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("real"); ap.add_argument("ours")
    ap.add_argument("--out"); ap.add_argument("--size", default="640x360")
    ap.add_argument("--tol", type=float, default=10.0)
    a = ap.parse_args()
    size = tuple(int(x) for x in a.size.split("x"))
    r = compare(a.real, a.ours, size, a.tol)
    print(f"match {r['match']:.1f}%  mean_dE {r['mean_dE']:.1f}  dL {r['dL']:+.1f} da {r['da']:+.1f} db {r['db']:+.1f}")
    for row in r["grid"]:
        print("   " + "  ".join(f"{v:5.1f}" for v in row))
    if a.out:
        heat = np.clip(r["de"] / 30.0, 0, 1)
        rgb = np.stack([heat, 1 - heat, np.zeros_like(heat)], -1)
        Image.fromarray((rgb * 255).astype(np.uint8)).save(a.out)


if __name__ == "__main__":
    main()

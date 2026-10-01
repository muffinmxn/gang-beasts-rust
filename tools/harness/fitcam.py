"""Fit our camera (GB_CAM_EYE / GB_CAM_AT, Unity space) to a real-game capture by maximising
edge correlation (lighting-independent), so pixel scoring compares the same view.

  python fitcam.py rooftop referen/harness/live/roof_1.png --eye -0.5,3.5,-5.5 --at -0.5,0,0.8

Writes target/harness/cam_<stage>.json with the best eye/at. One render at a time, low priority.
"""
import argparse, json, os, sys
import numpy as np
from PIL import Image, ImageFilter
sys.path.insert(0, os.path.dirname(__file__))
from shot import render, ROOT


def edges(path, size=(320, 180)):
    im = Image.open(path).convert("L").resize(size, Image.LANCZOS).filter(ImageFilter.GaussianBlur(1.2))
    a = np.asarray(im, dtype=float)
    gx = np.zeros_like(a); gy = np.zeros_like(a)
    gx[:, 1:-1] = a[:, 2:] - a[:, :-2]
    gy[1:-1, :] = a[2:, :] - a[:-2, :]
    m = np.hypot(gx, gy)
    m8 = Image.fromarray(np.clip(m / (m.max() + 1e-9) * 255, 0, 255).astype(np.uint8))
    m = np.asarray(m8.filter(ImageFilter.GaussianBlur(2.0)), dtype=float)
    m -= m.mean()
    return m / (np.linalg.norm(m) + 1e-9)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("stage"); ap.add_argument("ref")
    ap.add_argument("--eye", required=True); ap.add_argument("--at", required=True)
    ap.add_argument("--passes", type=int, default=3)
    ap.add_argument("--metric", default="edges", choices=["edges", "match"])
    ap.add_argument("--step", type=float, default=1.0)
    ap.add_argument("--fov", type=float, default=None)
    a = ap.parse_args()
    ref = edges(a.ref)
    shot = os.path.join(ROOT, "target", "harness", "fitcam.png")
    os.makedirs(os.path.dirname(shot), exist_ok=True)
    cur = [float(x) for x in a.eye.split(",")] + [float(x) for x in a.at.split(",")]
    if a.fov is not None:
        cur.append(a.fov)

    def score(p):
        env = {"GB_CAM_EYE": ",".join(f"{v:.3f}" for v in p[:3]),
               "GB_CAM_AT": ",".join(f"{v:.3f}" for v in p[3:6])}
        if len(p) > 6:
            env["GB_CAM_FOV"] = f"{p[6]:.2f}"
        if not render(a.stage, shot, env, after=200):
            return -1.0
        if a.metric == "match":
            from compare import compare
            return compare(a.ref, shot)["match"]
        return float((edges(shot) * ref).sum())

    best = score(cur)
    print(f"start {best:.4f} {cur}", flush=True)
    step = a.step
    for p in range(a.passes):
        for i in range(len(cur)):
            improved = True
            while improved:
                improved = False
                for sign in (1, -1):
                    trial = list(cur); trial[i] += sign * step * (5.0 if i == 6 else 1.0)
                    s = score(trial)
                    if s > best:
                        best, cur, improved = s, trial, True
                        print(f"pass {p} param {i} -> {s:.4f} {[round(v, 3) for v in cur]}", flush=True)
                        json.dump({"eye": cur[:3], "at": cur[3:6], "fov": cur[6] if len(cur) > 6 else None, "score": best},
                                  open(os.path.join(ROOT, "target", "harness", f"cam_{a.stage}.json"), "w"))
                        break
        step /= 2
    print(f"done {best:.4f} eye={cur[:3]} at={cur[3:]}")


if __name__ == "__main__":
    main()

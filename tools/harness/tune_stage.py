"""Pixel-matched coordinate-descent tuner for a stage view (uses the fitted camera).

  python tune_stage.py rooftop referen/harness/live/roof_1.png [--passes 2]

Camera from target/harness/cam_<stage>.json (fitcam.py). Scores with compare.py (% pixels dE<10,
minus 0.2 x mean dE). Best -> target/harness/tune_<stage>.json. One low-priority render at a time.
"""
import argparse, json, os, sys
sys.path.insert(0, os.path.dirname(__file__))
from shot import render, ROOT
from compare import compare

PARAMS = {  # name: (start, step, lo, hi)
    "GB_SUN_SCALE": (1.0, 0.25, 0.25, 3.0),
    "GB_LIGHTMAP_SCALE": (0.7, 0.15, 0.2, 1.5),
    "GB_LOOK_ROOFTOP_EV": (-0.8, 0.2, -2.0, 1.0),
    "GB_SKY_REFLECTION_SCALE": (1.0, 0.5, 0.0, 4.0),
    "GB_LOOK_ROOFTOP_SHADOW_FILL": (0.2, 0.1, 0.0, 0.6),
}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("stage"); ap.add_argument("ref"); ap.add_argument("--passes", type=int, default=2)
    a = ap.parse_args()
    cam = json.load(open(os.path.join(ROOT, "target", "harness", f"cam_{a.stage}.json")))
    fixed = {"GB_CAM_EYE": ",".join(str(v) for v in cam["eye"]), "GB_CAM_AT": ",".join(str(v) for v in cam["at"])}
    if "fov" in cam:
        fixed["GB_CAM_FOV"] = str(cam["fov"])
    shot = os.path.join(ROOT, "target", "harness", "tune_shot.png")
    cur = {k: v[0] for k, v in PARAMS.items()}

    def score(p):
        env = dict(fixed); env.update({k: str(round(v, 4)) for k, v in p.items()})
        if not render(a.stage, shot, env, after=200):
            return -1e9, None
        r = compare(a.ref, shot)
        return r["match"] - 0.2 * r["mean_dE"], r

    best, r = score(cur)
    print(f"start {best:.2f} match {r['match']:.1f}", flush=True)
    for p in range(a.passes):
        for k, (_, step, lo, hi) in PARAMS.items():
            s = step / (2 ** p)
            improved = True
            while improved:
                improved = False
                for sign in (1, -1):
                    t = dict(cur); t[k] = min(hi, max(lo, cur[k] + sign * s))
                    if t[k] == cur[k]:
                        continue
                    sc, rr = score(t)
                    if sc > best:
                        best, cur, r, improved = sc, t, rr, True
                        print(f"{k}={t[k]:.3f} -> {sc:.2f} match {rr['match']:.1f}", flush=True)
                        json.dump({"params": cur, "match": rr["match"], "mean_dE": rr["mean_dE"]},
                                  open(os.path.join(ROOT, "target", "harness", f"tune_{a.stage}.json"), "w"), indent=1)
                        break
    print(f"done match {r['match']:.1f} mean_dE {r['mean_dE']:.1f} {cur}")


if __name__ == "__main__":
    main()

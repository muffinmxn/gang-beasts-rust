"""Coordinate-descent tuner: render a fixed-camera shot, score it against the real-game capture
(compare.py), nudge one global look parameter at a time, keep improvements.

  python tune.py --ref referen/harness/title_1080.png --screen splash [--passes 3]

One render at a time at below-normal priority (the PC stays usable). Results go to
target/harness/tune.log and the best settings to target/harness/best.json.
"""
import argparse, json, os, subprocess, sys, time
sys.path.insert(0, os.path.dirname(__file__))
from compare import compare

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
EXE = os.path.join(ROOT, "target", "debug", "gb_game.exe")
OUT = os.path.join(ROOT, "target", "harness")
BELOW_NORMAL = 0x00004000

# name: (start, step, min, max)
PARAMS = {
    "GB_LOOK_EV": (0.0, 0.25, -2.0, 2.0),
    "GB_LOOK_AMBIENT": (280.0, 60.0, 50.0, 900.0),
    "GB_LOOK_BOUNCE": (0.35, 0.1, 0.0, 1.0),
    "GB_LOOK_SHADOW_FILL": (0.45, 0.1, 0.0, 0.9),
    "GB_MENU_SUN_SCALE": (1950.0, 300.0, 600.0, 4000.0),
    "GB_LOOK_TINT": (0.0, 0.15, 0.0, 1.0),
}


def render(env_vals, screen, path):
    env = dict(os.environ)
    env.update({k: str(v) for k, v in env_vals.items()})
    env.update({"GB_MENU_SCREEN": screen, "GB_SCREENSHOT": path,
                "GB_SCREENSHOT_AFTER": str(env_vals.get("GB_SCREENSHOT_AFTER", 600)), "GB_WINDOW_OFFSCREEN": "1"})
    if os.path.exists(path):
        os.remove(path)
    os.makedirs(OUT, exist_ok=True)
    # Keep one overwritten log so shader compilation errors cannot silently score as scenery.
    log_path = os.path.join(OUT, "render.log")
    with open(log_path, "w") as log:
        result = subprocess.run([EXE, "menu"], env=env, cwd=ROOT, stdout=log,
                                stderr=subprocess.STDOUT, timeout=240, creationflags=BELOW_NORMAL)
    errors = open(log_path, encoding="utf8", errors="replace").read()
    if result.returncode or "failed to process shader" in errors.lower() or "error: shader" in errors.lower():
        print(errors[-8000:])
        return False
    return os.path.exists(path)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--ref", required=True)
    ap.add_argument("--screen", default="splash")
    ap.add_argument("--passes", type=int, default=3)
    a = ap.parse_args()
    os.makedirs(OUT, exist_ok=True)
    log = open(os.path.join(OUT, "tune.log"), "a")
    shot = os.path.join(OUT, "shot.png")
    cur = {k: v[0] for k, v in PARAMS.items()}
    best_path = os.path.join(OUT, "best.json")
    if os.path.exists(best_path):
        cur.update(json.load(open(best_path))["params"])

    def score(vals):
        if not render(vals, a.screen, shot):
            return -1.0
        r = compare(a.ref, shot)
        return r["match"] - r["mean_dE"] * 0.2  # tie-break toward lower average error

    best = score(cur)
    print(f"start {best:.2f} {cur}", file=log, flush=True)
    for p in range(a.passes):
        for k, (_, step, lo, hi) in PARAMS.items():
            s = step / (2 ** p)
            improved = True
            while improved:
                improved = False
                for sign in (1, -1):
                    trial = dict(cur)
                    trial[k] = round(min(hi, max(lo, cur[k] + sign * s)), 4)
                    if trial[k] == cur[k]:
                        continue
                    sc = score(trial)
                    print(f"pass {p} {k}={trial[k]} -> {sc:.2f} (best {best:.2f})", file=log, flush=True)
                    if sc > best:
                        best, cur, improved = sc, trial, True
                        json.dump({"score": best, "params": cur}, open(best_path, "w"), indent=1)
                        break
    r = compare(a.ref, shot) if render(cur, a.screen, shot) else None
    print(f"done {best:.2f} {cur} match={r and r['match']:.1f}", file=log, flush=True)


if __name__ == "__main__":
    main()

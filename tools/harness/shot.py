"""Render one off-screen shot of a stage (low priority, one retry) and optionally print region
colours / a compare score. Usage:

  python shot.py rooftop out.png [KEY=VAL ...] [--regions rooftop] [--ref real.png]
  python shot.py menu out.png GB_MENU_SCREEN=splash --ref referen/harness/title_1080.png

Stage args: `rooftop` -> `rooftop --players 1` (costumes off), `menu` -> `menu`.
"""
import os, subprocess, sys, time
sys.path.insert(0, os.path.dirname(__file__))

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
EXE = os.path.join(ROOT, "target", "debug", "gb_game.exe")
BELOW_NORMAL = 0x00004000

# (y0, y1, x0, x1) at 1280x720 for our default rooftop spawn camera.
REGIONS = {
    "rooftop": {"ground": (380, 450, 800, 1100), "brick": (20, 90, 250, 420),
                "ac_top": (560, 640, 420, 700), "ac_side": (650, 710, 420, 700)},
}


def render(stage, out, env_vals, after=300):
    args = {"rooftop": ["rooftop", "--players", "1"], "menu": ["menu"]}.get(stage, [stage])
    env = dict(os.environ)
    env.update({"GB_WINDOW_OFFSCREEN": "1", "GB_SCREENSHOT": out, "GB_SCREENSHOT_AFTER": str(after),
                "GB_COSTUME": env.get("GB_COSTUME", "none")})
    env.update(env_vals)
    for attempt in range(2):
        if os.path.exists(out):
            os.remove(out)
        log = open(os.path.join(ROOT, "target", "harness_render.log"), "w")
        subprocess.run([EXE] + args, env=env, cwd=ROOT, stdout=log, stderr=subprocess.STDOUT,
                       timeout=240, creationflags=BELOW_NORMAL)
        log.close()
        if os.path.exists(out):
            return True
        time.sleep(1)
    return False


def region_means(path, regions):
    import numpy as np
    from PIL import Image
    a = np.asarray(Image.open(path).convert("RGB").resize((1280, 720)), dtype=float)
    return {k: a[y0:y1, x0:x1].reshape(-1, 3).mean(0).round().astype(int).tolist()
            for k, (y0, y1, x0, x1) in regions.items()}


def main():
    stage, out = sys.argv[1], os.path.abspath(sys.argv[2])
    rest = sys.argv[3:]
    env_vals, regions, ref = {}, None, None
    i = 0
    while i < len(rest):
        if rest[i] == "--regions":
            regions = REGIONS[rest[i + 1]]; i += 2
        elif rest[i] == "--ref":
            ref = rest[i + 1]; i += 2
        else:
            k, v = rest[i].split("=", 1); env_vals[k] = v; i += 1
    if not render(stage, out, env_vals):
        print("render failed; see target/harness_render.log"); sys.exit(1)
    if regions:
        print(" ".join(f"{k}={v}" for k, v in region_means(out, regions).items()))
    if ref:
        from compare import compare
        r = compare(ref, out)
        print(f"match {r['match']:.1f}%  mean_dE {r['mean_dE']:.1f}  dL {r['dL']:+.1f} da {r['da']:+.1f} db {r['db']:+.1f}")


if __name__ == "__main__":
    main()

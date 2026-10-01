"""One-shot rooftop colour sweep against the real capture's region colours (camera-independent).
Prints the best settings only. Targets from referen/harness/live/roof_1.png."""
import itertools, os, sys
import numpy as np
sys.path.insert(0, os.path.dirname(__file__))
from shot import render, region_means, REGIONS, ROOT

TARGET = {"ground": (144, 127, 135), "brick": (215, 159, 111)}
out = os.path.join(ROOT, "target", "harness", "sweep.png")
best = None
grid = {
    "GB_LIGHTMAP_SCALE": ["0.85", "1.0", "1.2"],
    "GB_LOOK_ROOFTOP_EV": ["-0.4", "0"],
}
keys = list(grid)
for vals in itertools.product(*grid.values()):
    env = dict(zip(keys, vals))
    if not render("rooftop", out, env, after=200):
        continue
    m = region_means(out, REGIONS["rooftop"])
    err = sum(np.linalg.norm(np.array(m[k]) - np.array(t)) for k, t in TARGET.items())
    if best is None or err < best[0]:
        best = (err, env, m)
print(f"best err {best[0]:.1f} env {best[1]} regions {best[2]}")

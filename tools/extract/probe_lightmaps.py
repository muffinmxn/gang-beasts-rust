"""Probe: which texture objects do the LightmapSettings pointers resolve to?

Run: python probe_lightmaps.py rotopo
Prints, per lightmap slot: path_id, candidate objects, resolved name/format/size,
and raw-texel statistics (before any decode) so we can see what the exporter
actually grabbed.
"""
import glob
import os
import sys

import numpy as np
import UnityPy

UnityPy.config.FALLBACK_UNITY_VERSION = "2021.3.33f1"

GAME = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..",
    "Base Game (Microsoft Store Version)", "Gang Beasts", "Content"))
AA = os.path.join(GAME, "Gang Beasts_Data/StreamingAssets/aa/StandaloneWindows64-LocalBundles")


def main() -> None:
    pattern = sys.argv[1] if len(sys.argv) > 1 else "rooftop"
    files = sorted(glob.glob(os.path.join(AA, "core-*.bundle")))
    files += [f for f in sorted(glob.glob(os.path.join(AA, "*.bundle"))) if pattern in os.path.basename(f).lower()]
    files.append(os.path.join(GAME, "Resources", "unity default resources"))
    env = UnityPy.load(*files)

    data, owner = None, None
    for obj in env.objects:
        if obj.type.name == "LightmapSettings":
            data = obj.read_typetree()
            owner = obj
            break
    if data is None:
        print("no LightmapSettings in", pattern)
        return
    print("lightmap slots:", len(data.get("m_Lightmaps", [])))
    for index, lm in enumerate(data.get("m_Lightmaps", [])):
        p = lm.get("m_Lightmap") if isinstance(lm, dict) else lm.m_Lightmap
        pid = p.get("m_PathID", 0) if isinstance(p, dict) else p.m_PathID
        if not pid:
            print(f"slot {index}: none")
            continue
        cands = [o for o in env.objects if o.path_id == pid]
        details = []
        for c in cands[:4]:
            try:
                t = c.read()
                name = getattr(t, "m_Name", "?")
                fmt = getattr(t, "m_TextureFormat", "?")
                size = (getattr(t, "m_Width", 0), getattr(t, "m_Height", 0))
                img = getattr(t, "image", None)
                stats = ""
                if img is not None:
                    a = np.asarray(img.convert("RGBA"), dtype=np.float32) / 255.0
                    rgb = a[...,:3]
                    stats = f" mean {rgb.mean():.3f} std {rgb.std():.3f} max {rgb.max():.2f}"
                details.append(f"{c.type.name} '{name}' fmt {fmt} {size}{stats}")
            except Exception as error:  # noqa: BLE001
                details.append(f"{c.type.name}: {error}")
        print(f"slot {index}: path_id {pid}")
        for d in details:
            print("   ", d)


if __name__ == "__main__":
    main()

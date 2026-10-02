"""Dump the Water4 sea materials (Gerstner waves, bump map, foam) to assets/export/water-params.json and save the
wave normal map as water_bump.png.

  python water_params.py                # all stages that have "Water - *" materials

Reads the game only (never writes into it).
"""
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import export as E  # noqa: E402

STAGES = ["buoy", "trawler", "wheel", "containers", "lighthouse", "crane"]


def main():
    out = {}
    bump_saved = False
    for stage in STAGES:
        try:
            env = E.load_env(f"stages-{stage}")
        except Exception as e:  # missing bundle
            print("skip", stage, e)
            continue
        by_path = {}
        for obj in env.objects:
            by_path[obj.path_id] = obj
        for obj in env.objects:
            if obj.type.name != "Material":
                continue
            m = obj.read()
            if not m.m_Name.startswith("Water - "):
                continue
            sp = obj.read_typetree()["m_SavedProperties"]
            entry = {
                "floats": {k: v for k, v in sp["m_Floats"]},
                "colors": {k: [v["r"], v["g"], v["b"], v["a"]] for k, v in sp["m_Colors"]},
            }
            for k, v in sp["m_TexEnvs"]:
                pid = v["m_Texture"]["m_PathID"]
                if k == "_BumpMap" and pid and pid in by_path:
                    tex = by_path[pid].read()
                    entry["bump"] = tex.m_Name
                    if not bump_saved:
                        E.unity_normal_map(tex.image).save(os.path.join(E.OUT, "water_bump.png"), "PNG")
                        bump_saved = True
            out[m.m_Name] = entry
            print("water", stage, m.m_Name)
    with open(os.path.join(E.OUT, "water-params.json"), "w") as f:
        json.dump(out, f, indent=1)


if __name__ == "__main__":
    main()

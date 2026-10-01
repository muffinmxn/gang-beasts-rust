"""Show the authored baseColorFactor of every TintA_*/TintB_* material in the costume GLBs.

Decides how the player palette is applied: if the TintA slots are authored ~white, the source
REPLACES the colour (multiplying a white base by the palette gives the palette); if they carry
distinct authored tones, the palette MULTIPLIES the authored tone.
"""
import glob
import json
import os
import struct
import sys

root = sys.argv[1] if len(sys.argv) > 1 else "assets/export/costumes"
rows = []
for path in sorted(glob.glob(os.path.join(root, "*.glb"))):
    try:
        d = open(path, "rb").read()
        cl = struct.unpack_from("<I", d, 12)[0]
        gltf = json.loads(d[20:20 + cl].decode("utf-8"))
    except Exception as exc:  # noqa: BLE001
        print("skip", path, exc)
        continue
    for m in gltf.get("materials", []):
        name = m.get("name", "")
        if "TintA" not in name and "TintB" not in name:
            continue
        factor = (m.get("pbrMetallicRoughness") or {}).get("baseColorFactor")
        rows.append((os.path.basename(path), name, factor))

for r in rows[:40]:
    print(f"{r[0]:12s} {r[1]:34s} {r[2]}")
print(f"... {len(rows)} TintA/TintB materials across {len(set(r[0] for r in rows))} items")

tint_a = [r for r in rows if "TintA" in r[1]]
if tint_a:
    print("\nTintA factors only:")
    for r in tint_a:
        print(f"  {r[1]:34s} {r[2]}")

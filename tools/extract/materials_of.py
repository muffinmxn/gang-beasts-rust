"""Print rooftop materials' shader + key colors from the GLB JSON chunk extras."""
import json
import struct
import sys

path = sys.argv[1] if len(sys.argv) > 1 else "assets/export/rooftop.glb"
with open(path, "rb") as fh:
    data = fh.read()
magic, version, length = struct.unpack_from("<III", data, 0)
offset = 12
chunk_len, chunk_type = struct.unpack_from("<II", data, offset)
gltf = json.loads(data[offset + 8: offset + 8 + chunk_len].decode("utf-8"))
seen = {}
for material in gltf.get("materials", []):
    extras = material.get("extras") or {}
    shader = extras.get("shader", "?")
    colors = extras.get("colors") or {}
    floats = extras.get("floats") or {}
    key = (shader, material.get("name", ""))
    if key in seen:
        continue
    seen[key] = True
    interesting = {k: v for k, v in colors.items()
                   if any(s in k for s in ("BaseColor", "_Color", "Emission", "Tint", "Color"))
                   and not k.startswith("Vector2")}
    f = {k: round(v, 3) for k, v in floats.items()
         if k in ("_Smoothness", "_Glossiness", "_Metallic", "_BumpScale")}
    print(f"{material.get('name',''):34s} {shader[:44]:46s} colors={json.dumps(interesting)[:90]} floats={f}")

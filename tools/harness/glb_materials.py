"""List a GLB's materials with their texture slots, to find materials whose texture we drop.

  python tools/harness/glb_materials.py assets/export/rooftop.glb [name_filter]
"""
import json
import struct
import sys

path = sys.argv[1]
needle = sys.argv[2].lower() if len(sys.argv) > 2 else ""
d = open(path, "rb").read()
cl = struct.unpack_from("<I", d, 12)[0]
gltf = json.loads(d[20:20 + cl].decode("utf-8"))

textures = gltf.get("textures", [])
images = gltf.get("images", [])


def image_of(slot):
    if not isinstance(slot, dict):
        return None
    tex = textures[slot["index"]]
    src = tex.get("source")
    if src is None:
        return "?"
    return images[src].get("uri") or images[src].get("name") or f"image{src}"


for m in gltf.get("materials", []):
    name = m.get("name", "")
    if needle and needle not in name.lower():
        continue
    pbr = m.get("pbrMetallicRoughness", {})
    extras = m.get("extras") or {}
    slots = {
        "base": image_of(pbr.get("baseColorTexture")),
        "mr": image_of(pbr.get("metallicRoughnessTexture")),
        "normal": image_of(m.get("normalTexture")),
        "occlusion": image_of(m.get("occlusionTexture")),
        "emissive": image_of(m.get("emissiveTexture")),
    }
    print(
        f"{name:28s} shader={str(extras.get('shader'))[:34]:34s} "
        f"factor={pbr.get('baseColorFactor')} slots={{"
        + ", ".join(f"{k}: {v}" for k, v in slots.items() if v)
        + "}"
    )

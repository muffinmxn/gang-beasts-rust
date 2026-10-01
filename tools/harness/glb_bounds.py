"""Report the beast GLB's mesh bounds per primitive (body, head, eyes) so a capture can be
matched to the geometry that produced each screen region."""
import json
import struct
import sys

import numpy as np

path = sys.argv[1] if len(sys.argv) > 1 else "assets/export/beast.glb"
d = open(path, "rb").read()
cl = struct.unpack_from("<I", d, 12)[0]
gltf = json.loads(d[20:20 + cl].decode("utf-8"))
bin_off = 20 + cl + 8
blob = d[bin_off:]

bufs = gltf["bufferViews"]


def accessor_values(index):
    a = gltf["accessors"][index]
    view = bufs[a["bufferView"]]
    comp = {5120: "b", 5121: "B", 5122: "h", 5123: "H", 5125: "I", 5126: "f"}[a["componentType"]]
    ncomp = {"SCALAR": 1, "VEC2": 2, "VEC3": 3, "VEC4": 4}[a["type"]]
    start = view.get("byteOffset", 0) + a.get("byteOffset", 0)
    stride = view.get("byteStride") or np.dtype(comp).itemsize * ncomp
    out = np.zeros((a["count"], ncomp), dtype=np.float64)
    for i in range(a["count"]):
        off = start + i * stride
        out[i] = struct.unpack_from("<" + comp * ncomp, blob, off)
    return out


for mesh in gltf.get("meshes", []):
    for p, prim in enumerate(mesh.get("primitives", [])):
        pos = accessor_values(prim["attributes"]["POSITION"])
        mat = gltf["materials"][prim["material"]]["name"]
        print(f"{mesh['name']}.{p} material {mat:16} verts {len(pos):5d} "
              f"y {pos[:,1].min():+.3f}..{pos[:,1].max():+.3f} "
              f"x {pos[:,0].min():+.3f}..{pos[:,0].max():+.3f} "
              f"z {pos[:,2].min():+.3f}..{pos[:,2].max():+.3f}")

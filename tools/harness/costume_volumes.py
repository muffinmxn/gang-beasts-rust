"""Show which beast bones a costume's KeepIn/KeepOut volumes cover.

  python tools/harness/costume_volumes.py <uid> [uid...]

Prints each volume's rest-pose centre and the nearest beast bones, so the intended semantics
(KeepIn = hide the wearer there / KeepOut = keep it visible) can be checked against the rig.
"""
import json
import math
import sys

BEAST = json.load(open("assets/export/beast.json"))


def beast_bones():
    """Rest-pose world positions of the beast's nodes (sidecar space)."""
    world = []
    for node in BEAST["nodes"]:
        t = node["transform"]
        pos = [t["translation"][0], t["translation"][1], t["translation"][2]]
        rot = t["rotation"]
        scale = t["scale"]
        parent = node.get("parent")
        if parent is None:
            world.append((pos, rot, scale))
            continue
        pp, pr, ps = world[parent]
        # parent transform applied to the local position
        x, y, z, w = pr
        # quaternion rotate
        px, py, pz = pos
        qx, qy, qz, qw = x, y, z, w
        # rotate vector by quaternion
        ix = qw * px + qy * pz - qz * py
        iy = qw * py + qz * px - qx * pz
        iz = qw * pz + qx * py - qy * px
        iw = -qx * px - qy * py - qz * pz
        rx = ix * qw + iw * -qx + iy * -qz - iz * -qy
        ry = iy * qw + iw * -qy + iz * -qx - ix * -qz
        rz = iz * qw + iw * -qz + ix * -qy - iy * -qx
        world.append(([pp[0] + rx * ps[0], pp[1] + ry * ps[1], pp[2] + rz * ps[2]],
                      rot, [ps[0] * scale[0], ps[1] * scale[1], ps[2] * scale[2]]))
    return [(n["path"].rsplit("/", 1)[-1], w[0]) for n, w in zip(BEAST["nodes"], world)]


def load(uid):
    d = json.load(open(f"assets/export/costumes/{uid}.json"))
    world = []
    out = []
    for node in d["nodes"]:
        t = node["transform"]
        pos = [t["translation"][0], t["translation"][1], t["translation"][2]]
        rot = t["rotation"]
        parent = node.get("parent")
        if parent is None:
            world.append((pos, rot))
        else:
            pp, pr = world[parent]
            x, y, z, w = pr
            px, py, pz = pos
            qx, qy, qz, qw = x, y, z, w
            ix = qw * px + qy * pz - qz * py
            iy = qw * py + qz * px - qx * pz
            iz = qw * pz + qx * py - qy * px
            iw = -qx * px - qy * py - qz * pz
            rx = ix * qw + iw * -qx + iy * -qz - iz * -qy
            ry = iy * qw + iw * -qy + iz * -qx - ix * -qz
            rz = iz * qw + iw * -qz + ix * -qy - iy * -qx
            world.append(([pp[0] + rx, pp[1] + ry, pp[2] + rz], rot))
        name = node["path"].rsplit("/", 1)[-1]
        low = name.lower()
        if low.startswith("keepin") or low.startswith("keepout"):
            for c in node.get("components", []):
                if "Collider" not in c["type"]:
                    continue
                data = c["data"]
                radius = data.get("m_Radius", 0.1)
                height = data.get("m_Height", 0.0)
                out.append((name, world[-1][0], radius, height))
    return out


bones = beast_bones()
for uid in sys.argv[1:]:
    print(f"===== costume {uid}")
    for name, center, radius, height in load(uid):
        near = sorted(bones, key=lambda b: sum((b[1][i] - center[i]) ** 2 for i in range(3)))[:3]
        near = [f"{n}@{d:.2f}" for n, p in near for d in [math.dist(p, center)]]
        print(f"  {name:14s} center {[round(v, 2) for v in center]} r {radius:.2f} h {height:.2f}"
              f"  nearest bones: {', '.join(near)}")

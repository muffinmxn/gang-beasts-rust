"""List the beast's colliders/rigidbodies: shape, layer and joint links.

Used for the ragdoll snagging work: shows which parts are capsules/boxes/convex meshes and which
layer each part is on (8 Actor, 9 Ball, 10 Ignore Self, 21 Ignore Actors, ...).
"""
import json
import sys

path = sys.argv[1] if len(sys.argv) > 1 else "assets/export/beast.json"
d = json.load(open(path))
print(f"{'node':30s} {'component':22s} {'layer':>5s}  detail")
for n in d["nodes"]:
    for c in n.get("components", []):
        kind = c["type"]
        if "Collider" not in kind and kind != "Rigidbody":
            continue
        data = c.get("data", {})
        detail = {k: v for k, v in data.items() if k.startswith("m_") and not isinstance(v, dict)}
        shape = {k: detail.get(k) for k in ("m_Type", "m_Radius", "m_Height", "m_Size", "m_Scale")
                 if detail.get(k) is not None}
        if kind == "Rigidbody":
            shape = {k: detail.get(k) for k in ("m_Mass", "m_Drag", "m_UseGravity", "m_IsKinematic")
                     if detail.get(k) is not None}
        print(f"{n['path'].split('/')[-1]:30s} {kind:22s} {n['layer']:5d}  {shape} mesh={c.get('collision_mesh')}")
print()
print("ConfigurableJoints:")
for n in d["nodes"]:
    for c in n.get("components", []):
        if c["type"] != "ConfigurableJoint":
            continue
        data = c.get("data", {})
        axes = {a: data.get(f"m_{a}Motion") for a in ("X", "Y", "Z", "AngularX", "AngularY", "AngularZ")}
        print(f"  {n['path'].split('/')[-1]:26s} enabled={data.get('m_EnableCollision')} limits={data.get('m_AngularXLimit') is not None} {axes}")

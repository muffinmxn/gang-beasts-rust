"""Headless validation of exported geometry, skinning, hierarchy and physics references."""
import argparse
import json
import struct
from pathlib import Path
import numpy as np
from geometry import trs


def load_glb(path):
    raw = Path(path).read_bytes()
    magic, version, length = struct.unpack_from("<III", raw)
    if (magic, version, length) != (0x46546C67, 2, len(raw)):
        raise ValueError(f"{path}: invalid GLB header")
    size, tag = struct.unpack_from("<II", raw, 12)
    if tag != 0x4E4F534A:
        raise ValueError("missing JSON chunk")
    doc = json.loads(raw[20:20+size])
    offset = 20+size
    bin_size, bin_tag = struct.unpack_from("<II", raw, offset)
    if bin_tag != 0x004E4942 or offset+8+bin_size != len(raw):
        raise ValueError("invalid binary chunk")
    return doc, raw[offset+8:]


def accessor(doc, data, index):
    a = doc["accessors"][index]
    v = doc["bufferViews"][a["bufferView"]]
    dtype = {5121: "<u1", 5123: "<u2", 5125: "<u4", 5126: "<f4"}[a["componentType"]]
    width = {"SCALAR": 1, "VEC2": 2, "VEC3": 3, "VEC4": 4, "MAT4": 16}[a["type"]]
    offset = v.get("byteOffset", 0)+a.get("byteOffset", 0)
    size = a["count"]*width*np.dtype(dtype).itemsize
    if offset+size > v.get("byteOffset", 0)+v["byteLength"] or offset+size > len(data):
        raise ValueError(f"accessor {index} exceeds buffer view")
    result = np.frombuffer(data, dtype=dtype, count=a["count"]*width, offset=offset).reshape(-1, width)
    if not np.isfinite(result).all():
        raise ValueError(f"accessor {index} contains nonfinite numbers")
    return result


def validate(path):
    doc, data = load_glb(path)
    side = json.loads(Path(path).with_suffix(".json").read_text())
    nodes = doc["nodes"]
    if side.get("version") != 3 or len(nodes) != len(side["nodes"]):
        raise ValueError("sidecar must be version 3 and match the GLB node count")
    arrays = [accessor(doc, data, i) for i in range(len(doc.get("accessors", [])))]
    physics_skin_joints = set()
    if side["kind"] == "prefab":
        for skin in doc.get("skins", []):
            for joint in skin["joints"]:
                if not 0 <= joint < len(side["nodes"]):
                    raise ValueError(f"skin joint index out of range: {joint}")
                ancestor = joint
                while ancestor is not None and not any(
                    component["type"] == "Rigidbody"
                    for component in side["nodes"][ancestor]["components"]
                ):
                    ancestor = side["nodes"][ancestor]["parent"]
                if ancestor is None:
                    path = side["nodes"][joint]["path"]
                    raise ValueError(f"ragdoll skin joint has no simulated body ancestor: {path}")
                physics_skin_joints.add(joint)
    world = {}
    def visit(i, parent, matrix):
        if i in world:
            raise ValueError("cycle or duplicate parent in node hierarchy")
        n = nodes[i]
        meta = side["nodes"][i]
        if meta["parent"] != parent or n["extras"]["gb_node"] != i:
            raise ValueError("sidecar/GLB hierarchy mismatch")
        world[i] = matrix @ trs(n["translation"], n["rotation"], n["scale"])
        for child in n.get("children", []):
            visit(child, i, world[i])
    for root in doc["scenes"][doc.get("scene", 0)]["nodes"]:
        visit(root, None, np.eye(4))
    if len(world) != len(nodes):
        raise ValueError("orphaned nodes")
    triangles = 0
    mesh_bounds = {}
    bodies, joints = 0, 0
    for i, n in enumerate(nodes):
        meta = side["nodes"][i]
        for c in meta["components"]:
            if c["type"] == "Rigidbody":
                bodies += 1
                if c["data"]["m_Mass"] <= 0:
                    raise ValueError("nonpositive rigidbody mass")
            if c["type"].endswith("Joint"):
                joints += 1
                target = c["connected_node"]
                if target is not None and not any(x["type"] == "Rigidbody" for x in side["nodes"][target]["components"]):
                    raise ValueError("joint target is not a rigidbody")
        if "mesh" not in n: continue
        if (not meta["renderer"]["enabled"] or
                (not meta["active_in_hierarchy"] and not meta.get("render_override", False))):
            raise ValueError("hidden renderer exported as visible geometry")
        bounds = []
        for p in doc["meshes"][n["mesh"]]["primitives"]:
            attrs = p["attributes"]
            pos = arrays[attrs["POSITION"]]
            indices = arrays[p["indices"]].ravel()
            if len(indices)%3 or int(indices.max()) >= len(pos):
                raise ValueError("invalid triangle indices")
            triangles += len(indices)//3
            if any(len(arrays[a]) != len(pos) for a in attrs.values()):
                raise ValueError("mismatched vertex attribute counts")
            if "skin" in n:
                skin = doc["skins"][n["skin"]]
                weights = arrays[attrs["WEIGHTS_0"]]
                joint_ids = arrays[attrs["JOINTS_0"]]
                if not np.allclose(weights.sum(axis=1), 1, atol=1e-5) or int(joint_ids.max()) >= len(skin["joints"]):
                    raise ValueError("invalid skin weights or joint indices")
                inverse = arrays[skin["inverseBindMatrices"]].reshape(-1, 4, 4).transpose(0, 2, 1)
                matrices = np.array([world[j] @ inverse[k] for k, j in enumerate(skin["joints"])])
                vertices = np.column_stack((pos, np.ones(len(pos))))
                deformed = np.zeros((len(pos), 4))
                for k in range(4):
                    deformed += np.einsum("nij,nj->ni", matrices[joint_ids[:, k]], vertices)*weights[:, k, None]
                points = deformed[:, :3]
            else:
                if "JOINTS_0" in attrs or "WEIGHTS_0" in attrs:
                    raise ValueError("skin attributes on an unskinned node")
                points = (world[i] @ np.column_stack((pos, np.ones(len(pos)))).T).T[:, :3]
            if not np.isfinite(points).all(): raise ValueError("nonfinite world geometry")
            bounds.extend([points.min(axis=0), points.max(axis=0)])
        points = np.array(bounds)
        mesh_bounds[n["name"]] = [points.min(axis=0).round(5).tolist(), points.max(axis=0).round(5).tolist()]
        if "static_batch" in meta and len(doc["meshes"][n["mesh"]]["primitives"]) > meta["static_batch"]["submesh_count"]:
            raise ValueError("renderer contains another object's static batch geometry")
    if side["kind"] == "prefab":
        for root in doc["scenes"][0]["nodes"]:
            if not np.allclose(nodes[root]["translation"], 0):
                raise ValueError("prefab retains scene placement")
        head = mesh_bounds.get("actor_head_skinnedMesh")
        if head and not (0.8 < head[0][1] < head[1][1] < 2.0 and max(abs(head[0][2]), abs(head[1][2])) < 0.7):
            raise ValueError(f"head is not attached in upright bind pose: {head}")
    return {"file": str(path), "nodes": len(nodes), "meshes": len(doc["meshes"]),
            "triangles": triangles, "skins": len(doc.get("skins", [])), "rigidbodies": bodies, "joints": joints,
            "physics_skin_joints": len(physics_skin_joints),
            "static_batch_renderers": sum("static_batch" in n for n in side["nodes"]),
            "actor_bounds": {k:v for k,v in mesh_bounds.items() if k.startswith("actor_")}}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("files", nargs="+")
    parser.add_argument("--report")
    args = parser.parse_args()
    reports = [validate(p) for p in args.files]
    text = json.dumps(reports, indent=2)
    print(text)
    if args.report:
        Path(args.report).write_text(text+"\n")


if __name__ == "__main__":
    main()

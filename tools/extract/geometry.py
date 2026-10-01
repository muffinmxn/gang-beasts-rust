"""Coordinate and skinning conversion, independent of UnityPy for regression tests."""
import numpy as np


def trs(position, rotation, scale):
    x, y, z, w = rotation
    result = np.eye(4)
    result[:3, :3] = np.array([
        [1-2*(y*y+z*z), 2*(x*y-z*w), 2*(x*z+y*w)],
        [2*(x*y+z*w), 1-2*(x*x+z*z), 2*(y*z-x*w)],
        [2*(x*z-y*w), 2*(y*z+x*w), 1-2*(x*x+y*y)],
    ]) * scale
    result[:3, 3] = position
    return result


def geometry(handler, submeshes=None, to_local=None, bind_count=0):
    """Select a renderer's own submeshes, compact vertices, unbatch, then mirror X."""
    triangles = handler.get_triangles()
    selected = list(range(len(triangles))) if submeshes is None else list(submeshes)
    if any(i < 0 or i >= len(triangles) for i in selected):
        raise ValueError("static batch submesh range is outside its combined mesh")
    groups = [triangles[i] for i in selected]
    used = sorted({int(v) for group in groups for tri in group for v in tri})
    if not used:
        return None
    remap = {old: new for new, old in enumerate(used)}
    p = np.array([handler.m_Vertices[i][:3] for i in used], dtype=float)
    normal = np.array([handler.m_Normals[i][:3] for i in used], dtype=float) if handler.m_Normals else None
    source_tangents = getattr(handler, "m_Tangents", None)
    tangent = (np.array([source_tangents[i][:4] for i in used], dtype=float)
               if source_tangents else None)
    tangent_handedness = 1.0
    if to_local is not None:
        p = (to_local @ np.column_stack((p, np.ones(len(p)))).T).T[:, :3]
        if normal is not None:
            normal = (np.linalg.inv(to_local[:3, :3]).T @ normal.T).T
        if tangent is not None:
            tangent[:, :3] = (to_local[:3, :3] @ tangent[:, :3].T).T
            # A reflected unbatch transform reverses the tangent frame. The later X
            # coordinate mirror and UV-V flip reverse it twice more and cancel out.
            tangent_handedness = -1.0 if np.linalg.det(to_local[:3, :3]) < 0 else 1.0
    p[:, 0] *= -1
    attrs = {"POSITION": p.tolist()}
    if normal is not None:
        normal[:, 0] *= -1
        lengths = np.linalg.norm(normal, axis=1, keepdims=True)
        normal = normal / np.maximum(lengths, 1e-12)
        attrs["NORMAL"] = normal.tolist()
    if tangent is not None:
        tangent[:, 0] *= -1
        # Non-uniform static-batch transforms can make the transformed tangent no
        # longer perpendicular to the normal. Rebuild the orthonormal direction.
        if normal is not None:
            tangent[:, :3] -= normal * np.sum(tangent[:, :3] * normal, axis=1, keepdims=True)
        lengths = np.linalg.norm(tangent[:, :3], axis=1, keepdims=True)
        tangent[:, :3] /= np.maximum(lengths, 1e-12)
        tangent[:, 3] *= tangent_handedness
        attrs["TANGENT"] = tangent.tolist()
    if handler.m_UV0:
        attrs["TEXCOORD_0"] = [[handler.m_UV0[i][0], 1-handler.m_UV0[i][1]] for i in used]
    # Unity's second UV set commonly stores baked-lightmap coordinates. Keep it separate
    # from the material UVs so the renderer can sample exported lightmaps without repacking.
    uv1 = getattr(handler, "m_UV1", None)
    if uv1:
        attrs["TEXCOORD_1"] = [[uv1[i][0], 1-uv1[i][1]] for i in used]
        if not handler.m_UV0:
            # Unity's missing TEXCOORD0 vertex input defaults to (0, 0). Preserve that
            # behavior explicitly: Bevy's PBR texture variant needs UV0 when a material
            # samples channel 0, even if the mesh only contains its lightmap UV1 channel.
            attrs["TEXCOORD_0"] = [[0.0, 0.0] for _ in used]
    if handler.m_Colors:
        cols = [list(handler.m_Colors[i][:4]) for i in used]
        # Color32 vertex colours can come back as 0..255; glTF COLOR_0 floats are 0..1.
        if cols and max(max(c) for c in cols) > 1.5:
            cols = [[v / 255.0 for v in c] for c in cols]
        attrs["COLOR_0"] = cols
    if bind_count:
        weights, indices = handler.m_BoneWeights, handler.m_BoneIndices
        if weights and indices:
            joint_rows, weight_rows = [], []
            for i in used:
                if len(indices[i]) != len(weights[i]):
                    raise ValueError("bone indices and weights have different influence counts")
                influences = sorted(
                    ((int(joint), float(weight)) for joint, weight in zip(indices[i], weights[i])),
                    key=lambda item: item[1],
                    reverse=True,
                )[:4]
                total = sum(weight for _, weight in influences)
                if total <= 0:
                    raise ValueError("skinned vertex has no positive bone weight")
                joints = [joint for joint, _ in influences]
                normalized = [weight / total for _, weight in influences]
                joint_rows.append(joints + [0] * (4 - len(joints)))
                weight_rows.append(normalized + [0.0] * (4 - len(normalized)))
            attrs["JOINTS_0"] = joint_rows
            attrs["WEIGHTS_0"] = weight_rows
        elif bind_count == 1 and (not indices or all(all(int(v) == 0 for v in indices[i]) for i in used)):
            # Unity omits the redundant weight channel on rigid, single-bone skins.
            attrs["JOINTS_0"] = [[0, 0, 0, 0] for _ in used]
            attrs["WEIGHTS_0"] = [[1.0, 0.0, 0.0, 0.0] for _ in used]
        else:
            raise ValueError("skinned mesh has bind poses but no usable bone weights")
    # Mirroring X reverses winding; unbatching a reflected transform reverses it again.
    flip = to_local is None or np.linalg.det(to_local[:3, :3]) > 0
    groups = [[tuple(remap[int(v)] for v in (a, c, b)) if flip
               else tuple(remap[int(v)] for v in (a, b, c))
               for a, b, c in group] for group in groups]
    return attrs, groups

//! `RemoveUnseenMesh`: the source hides the beast's own mesh where an outfit covers it.
//!
//! Costume prefabs carry a `CostumeSettings` MonoBehaviour (`ReduceUsingMesh`, `PartSize`) plus
//! colliders named `KeepIn*` / `KeepOut*` on the `UnseenFaces` layer. When an item is equipped the
//! wearer's mesh is "reduced": the parts of the body that sit inside the costume (the `KeepIn`
//! volumes) and outside every `KeepOut` volume are removed, so the outfit is what you see instead
//! of the body poking through it.
//!
//! Port: the costume sidecar gives the volumes in the rig's rest pose, which is the pose the
//! costume is authored in. A beast bone whose rest position is inside a `KeepIn` volume (and not
//! inside a `KeepOut` volume) is *covered*; every beast mesh vertex weighted mostly to a covered
//! bone is collapsed onto that bone, which removes it exactly where the outfit hides it.
use bevy::prelude::*;


/// One costume collider volume, in the rig's rest-pose space (a capsule/sphere/box approximated
/// by its oriented bounding box, which is what "is this bone inside" needs).
#[derive(Clone, Copy, Debug)]
pub struct Volume {
    pub center: Vec3,
    pub rotation: Quat,
    pub half_extents: Vec3,
}

impl Volume {
    pub fn contains(&self, point: Vec3) -> bool {
        let local = self.rotation.inverse() * (point - self.center);
        local.abs().cmple(self.half_extents).all()
    }
}

/// The `KeepIn`/`KeepOut` volumes of one costume item, read from its sidecar.
#[derive(Clone, Debug, Default)]
pub struct CostumeVolumes {
    pub reduce: bool,
    pub part_size: u64,
    pub keep_in: Vec<Volume>,
    pub keep_out: Vec<Volume>,
}

/// Reads `assets/export/costumes/<uid>.json`: `CostumeSettings` + the `KeepIn*`/`KeepOut*`
/// colliders, converted into rest-pose volumes.
pub fn load(root: &std::path::Path, uid: u16) -> Option<CostumeVolumes> {
    let path = root.join(format!("costumes/{uid}.json"));
    let raw = std::fs::read(&path).ok()?;
    let data: serde_json::Value = serde_json::from_slice(&raw).ok()?;
    let nodes = data["nodes"].as_array()?;
    let mut out = CostumeVolumes::default();
    // World poses in the rig's own space (the costume is authored on the beast rig).
    let mut world: Vec<(Vec3, Quat, Vec3)> = Vec::with_capacity(nodes.len());
    for node in nodes.iter() {
        let t = &node["transform"];
        let translation = Vec3::new(
            t["translation"][0].as_f64().unwrap_or(0.0) as f32,
            t["translation"][1].as_f64().unwrap_or(0.0) as f32,
            t["translation"][2].as_f64().unwrap_or(0.0) as f32,
        );
        let rotation = Quat::from_xyzw(
            t["rotation"][0].as_f64().unwrap_or(0.0) as f32,
            t["rotation"][1].as_f64().unwrap_or(0.0) as f32,
            t["rotation"][2].as_f64().unwrap_or(0.0) as f32,
            t["rotation"][3].as_f64().unwrap_or(1.0) as f32,
        )
        .normalize();
        let scale = Vec3::new(
            t["scale"][0].as_f64().unwrap_or(1.0) as f32,
            t["scale"][1].as_f64().unwrap_or(1.0) as f32,
            t["scale"][2].as_f64().unwrap_or(1.0) as f32,
        );
        let (parent_pos, parent_rot, parent_scale) = node["parent"]
            .as_u64()
            .and_then(|p| world.get(p as usize).copied())
            .unwrap_or((Vec3::ZERO, Quat::IDENTITY, Vec3::ONE));
        let pos = parent_pos + parent_rot * (parent_scale * translation);
        let rot = (parent_rot * rotation).normalize();
        world.push((pos, rot, parent_scale * scale));

        // CostumeSettings: the item's reduce flag.
        for component in node["components"].as_array().into_iter().flatten() {
            let script = component["script"].as_str().unwrap_or("");
            if script != "CostumeSettings" {
                continue;
            }
            let d = &component["data"];
            out.reduce = d["ReduceUsingMesh"].as_u64().unwrap_or(0) != 0;
            out.part_size = d["PartSize"].as_u64().unwrap_or(0);
        }
        // Colliders named KeepIn*/KeepOut* define the covered regions.
        let name = node["path"]
            .as_str()
            .unwrap_or("")
            .rsplit('/')
            .next()
            .unwrap_or("")
            .to_ascii_lowercase();
        let keep_in = name.starts_with("keepin");
        let keep_out = name.starts_with("keepout");
        if !keep_in && !keep_out {
            continue;
        }
        for component in node["components"].as_array().into_iter().flatten() {
            if !component["type"].as_str().unwrap_or("").ends_with("Collider") {
                continue;
            }
            let d = &component["data"];
            let radius = d["m_Radius"].as_f64().unwrap_or(0.1) as f32;
            let half_height = d["m_Height"].as_f64().unwrap_or(0.0) as f32 * 0.5;
            let size = &d["m_Size"];
            let half_extents = if d["m_Type"].as_u64() == Some(2) || size.is_array() {
                Vec3::new(
                    size[0].as_f64().unwrap_or(0.2) as f32 * 0.5,
                    size[1].as_f64().unwrap_or(0.2) as f32 * 0.5,
                    size[2].as_f64().unwrap_or(0.2) as f32 * 0.5,
                )
            } else {
                // Sphere (0) / capsule (1): a box that contains the shape.
                let (x, y, z) = if d["m_Direction"].as_u64() == Some(0) {
                    (half_height + radius, radius, radius)
                } else if d["m_Direction"].as_u64() == Some(2) {
                    (radius, radius, half_height + radius)
                } else {
                    (radius, half_height + radius, radius)
                };
                Vec3::new(x, y, z)
            };
            let volume = Volume {
                center: pos,
                rotation: rot,
                half_extents: half_extents * parent_scale.abs().max(Vec3::splat(1e-3)),
            };
            if keep_in {
                out.keep_in.push(volume);
            } else {
                out.keep_out.push(volume);
            }
        }
    }
    Some(out)
}

/// Averaged vertex normals (`getAvroagedNormals`): normals of vertices sharing a position (UV /
/// hard-edge seams) are averaged, so a seam does not split the ray direction.
fn averaged_normals(positions: &[[f32; 3]], normals: &[[f32; 3]]) -> Vec<Vec3> {
    use std::collections::HashMap;
    let key = |p: &[f32; 3]| {
        [(p[0] * 2000.0).round() as i32, (p[1] * 2000.0).round() as i32, (p[2] * 2000.0).round() as i32]
    };
    let mut sums: HashMap<[i32; 3], Vec3> = HashMap::new();
    for (p, n) in positions.iter().zip(normals.iter()) {
        *sums.entry(key(p)).or_insert(Vec3::ZERO) += Vec3::from_array(*n);
    }
    positions
        .iter()
        .map(|p| sums.get(&key(p)).copied().unwrap_or(Vec3::Y).normalize_or_zero())
        .collect()
}

/// Moller-Trumbore ray vs triangle for the segment `origin + dir * t`, `0 <= t <= max`. Unity's
/// MeshCollider raycast ignores back faces: the ray must travel against the face normal. The
/// costume glTF keeps Unity's clockwise winding, so the cross product points inward and the
/// test is inverted (`normal . dir` must be positive).
fn ray_hits_front_face(origin: Vec3, dir: Vec3, max: f32, tri: &[Vec3; 3]) -> bool {
    let e1 = tri[1] - tri[0];
    let e2 = tri[2] - tri[0];
    let normal = e1.cross(e2);
    // Costume meshes mix windings (and many cloth shells are open sheets), so treat the cloth as
    // double sided: a one-sided test left holes of visible body inside coats.
    let _ = normal;
    let p = dir.cross(e2);
    let det = e1.dot(p);
    if det.abs() < 1e-12 {
        return false;
    }
    let inv = 1.0 / det;
    let t = origin - tri[0];
    let u = t.dot(p) * inv;
    if !(0.0..=1.0).contains(&u) {
        return false;
    }
    let q = t.cross(e1);
    let v = dir.dot(q) * inv;
    if v < 0.0 || u + v > 1.0 {
        return false;
    }
    let d = e2.dot(q) * inv;
    d >= 0.0 && d <= max
}

/// `RemoveUnseenMesh.UnseenFaces` for ONE costume item, in the rig's rest pose.
///
/// Source (0x4F9C30): for every wearer vertex, a ray starts `RaycastDistance - RayInnerOffset`
/// (0.5 - 0.05) outside the surface along the averaged vertex normal and travels back through the
/// vertex for `RaycastDistance`; the vertex is hidden when that ray hits the item's mesh. Vertices
/// inside a `KeepIn*` volume are the ones the item keeps visible (their hits cancel), so they are
/// never hidden. (The old port had this backwards: it forced KeepIn vertices hidden.)
pub fn covered_vertices(
    positions: &[[f32; 3]],
    normals: &[[f32; 3]],
    cloth: &[[Vec3; 3]],
    volumes: &CostumeVolumes,
) -> Vec<bool> {
    if !volumes.reduce || cloth.is_empty() {
        return vec![false; positions.len()];
    }
    let distance = 0.5_f32;
    let inner = 0.05_f32;
    let avg = averaged_normals(positions, normals);
    positions
        .iter()
        .zip(avg.iter())
        .map(|(p, n)| {
            if *n == Vec3::ZERO {
                return false;
            }
            let v = Vec3::from_array(*p);
            if volumes.keep_in.iter().any(|vol| vol.contains(v)) {
                return false;
            }
            let origin = v + *n * (distance - inner);
            let dir = -*n;
            cloth.iter().any(|tri| ray_hits_front_face(origin, dir, distance, tri))
        })
        .collect()
}

/// Removes the wearer triangles the item hides (`CreateNewTriangles` 0x4F98B0): a triangle goes
/// only when ALL THREE vertices are hidden, after `grow` erosion rings
/// (`reduceTrianglesSelection`: a triangle with an unhidden vertex un-hides its other two).
/// Nothing is shrunk, so a partly covered part keeps its full size.
pub fn remove_covered_faces(mesh: &mut Mesh, original: &Mesh, vertex_covered: &[bool], grow: usize) -> bool {
    use bevy::render::mesh::Indices;
    let Some(indices) = original.indices() else { return false };
    let tris: Vec<u32> = match indices {
        Indices::U16(i) => i.iter().map(|v| *v as u32).collect(),
        Indices::U32(i) => i.clone(),
    };
    let mut sel: Vec<bool> = vertex_covered.to_vec();
    sel.resize(sel.len().max(tris.iter().copied().max().unwrap_or(0) as usize + 1), false);
    for _ in 0..grow {
        let before = sel.clone();
        for t in tris.chunks_exact(3) {
            if !(before[t[0] as usize] && before[t[1] as usize] && before[t[2] as usize]) {
                for &i in t {
                    sel[i as usize] = false;
                }
            }
        }
    }
    let mut kept: Vec<u32> = Vec::with_capacity(tris.len());
    for t in tris.chunks_exact(3) {
        if !(sel[t[0] as usize] && sel[t[1] as usize] && sel[t[2] as usize]) {
            kept.extend_from_slice(t);
        }
    }
    if kept.len() == tris.len() {
        return false;
    }
    mesh.insert_indices(Indices::U32(kept));
    true
}

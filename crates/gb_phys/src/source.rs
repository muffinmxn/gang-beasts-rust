//! Physics-relevant slice of the exported sidecars, in raw Unity (left-handed) space.
//!
//! Unity hands its coordinates to PhysX unchanged, so the simulation runs in Unity space too;
//! only presentation mirrors X (see `unity_to_gltf`).
use glam::{Mat4, Quat, Vec3};
use serde::Deserialize;
use serde_json::Value;
use std::{fs, path::Path};

/// `physics.json`: project settings from `globalgamemanagers`.
#[derive(Deserialize, Clone, Debug)]
pub struct Settings {
    pub gravity: [f32; 3],
    pub fixed_timestep: f32,
    pub maximum_timestep: f32,
    pub solver_iterations: u32,
    pub solver_velocity_iterations: u32,
    pub bounce_threshold: f32,
    pub sleep_threshold: f32,
    pub contact_offset: f32,
    pub max_depenetration_velocity: f32,
    pub max_angular_speed: f32,
    pub adaptive_force: bool,
    pub enhanced_determinism: bool,
    pub friction_type: String,
    pub solver_type: String,
    pub broadphase: String,
    pub contacts_generation: String,
    pub layers: Vec<String>,
    pub layer_collision_matrix: Vec<Vec<bool>>,
}

impl Settings {
    pub fn load(root: &Path) -> Result<Self, String> {
        let path = root.join("physics.json");
        let raw = fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let s: Self = serde_json::from_slice(&raw).map_err(|e| {
            format!(
                "{}: {e}; run tools/extract/export.py settings physics",
                path.display()
            )
        })?;
        if s.layer_collision_matrix.len() != 32
            || s.layer_collision_matrix.iter().any(|r| r.len() != 32)
        {
            return Err("physics.json: layer_collision_matrix must be 32x32".into());
        }
        if !(s.fixed_timestep > 0.0) || s.solver_iterations == 0 {
            return Err("physics.json: invalid timestep or solver iterations".into());
        }
        Ok(s)
    }

    /// Bitmask of layers that `layer` generates contacts with.
    pub fn collision_mask(&self, layer: u32) -> u32 {
        let row = &self.layer_collision_matrix[layer as usize & 31];
        row.iter()
            .enumerate()
            .fold(0, |m, (j, &on)| if on { m | 1 << j } else { m })
    }
}

#[derive(Deserialize, Clone, Debug)]
pub struct Sidecar {
    pub nodes: Vec<Node>,
    #[serde(default)]
    pub collision_meshes: Vec<CollisionMesh>,
}

fn default_true() -> bool {
    true
}

#[derive(Deserialize, Clone, Debug)]
pub struct Node {
    pub path: String,
    pub parent: Option<usize>,
    pub layer: u32,
    pub active_in_hierarchy: bool,
    /// The GameObject's own activeSelf flag (children of an inactive node are not drawn).
    #[serde(default = "default_true")]
    pub active: bool,
    /// The exporter kept this inactive node's meshes on purpose (render-only previews).
    #[serde(default)]
    pub render_override: bool,
    pub transform: GltfTransform,
    pub components: Vec<Component>,
}

/// Node-local transform as written to the GLB (X mirrored).
#[derive(Deserialize, Clone, Copy, Debug)]
pub struct GltfTransform {
    pub translation: [f32; 3],
    pub rotation: [f32; 4],
    pub scale: [f32; 3],
}

#[derive(Deserialize, Clone, Debug)]
pub struct Component {
    #[serde(rename = "type")]
    pub kind: String,
    pub script: Option<String>,
    pub connected_node: Option<usize>,
    pub material: Option<Material>,
    pub collision_mesh: Option<usize>,
    pub data: Value,
}

#[derive(Deserialize, Clone, Debug, PartialEq)]
pub struct Material {
    pub name: String,
    pub dynamic_friction: f32,
    pub static_friction: f32,
    pub bounciness: f32,
    pub friction_combine: String,
    pub bounce_combine: String,
}

#[derive(Deserialize, Clone, Debug)]
pub struct CollisionMesh {
    pub name: String,
    pub vertices: Vec<f32>,
    pub triangles: Vec<u32>,
}

/// Rigid pose plus Unity's lossy scale.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Pose {
    pub position: Vec3,
    pub rotation: Quat,
    pub scale: Vec3,
}

impl Pose {
    pub const IDENTITY: Self = Self {
        position: Vec3::ZERO,
        rotation: Quat::IDENTITY,
        scale: Vec3::ONE,
    };

    pub fn matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.scale, self.rotation, self.position)
    }
}

/// glTF (X-mirrored) <-> Unity. The mapping is its own inverse.
pub fn mirror_position(p: Vec3) -> Vec3 {
    Vec3::new(-p.x, p.y, p.z)
}
pub fn mirror_rotation(q: Quat) -> Quat {
    Quat::from_xyzw(q.x, -q.y, -q.z, q.w)
}

impl Node {
    pub fn local_pose(&self) -> Pose {
        let t = &self.transform;
        Pose {
            position: mirror_position(Vec3::from_array(t.translation)),
            rotation: mirror_rotation(Quat::from_array(t.rotation)).normalize(),
            scale: Vec3::from_array(t.scale),
        }
    }

    pub fn component(&self, kind: &str) -> Option<&Component> {
        self.components.iter().find(|c| c.kind == kind)
    }

    pub fn name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }
}

impl Sidecar {
    pub fn load(root: &Path, name: &str) -> Result<Self, String> {
        let path = root.join(format!("{name}.json"));
        let raw = fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let s: Self =
            serde_json::from_slice(&raw).map_err(|e| format!("{}: {e}", path.display()))?;
        for (i, n) in s.nodes.iter().enumerate() {
            if n.parent.is_some_and(|p| p >= i) {
                return Err(format!("{}: parent must precede child", n.path));
            }
            for c in &n.components {
                if c.collision_mesh
                    .is_some_and(|m| m >= s.collision_meshes.len())
                {
                    return Err(format!("{}: collision mesh index out of range", n.path));
                }
            }
        }
        Ok(s)
    }

    /// Unity world poses (position, rotation, lossy scale) with the root nodes placed at `origin`.
    /// Rotation composes like Transform.rotation; lossyScale ignores skew, like Unity's approximation.
    pub fn world_poses(&self, origin: Pose) -> Vec<Pose> {
        let mut out: Vec<Pose> = Vec::with_capacity(self.nodes.len());
        for n in &self.nodes {
            let local = n.local_pose();
            let parent = match n.parent {
                Some(p) => out[p],
                None => origin,
            };
            out.push(Pose {
                position: parent.matrix().transform_point3(local.position),
                rotation: (parent.rotation * local.rotation).normalize(),
                scale: parent.scale * local.scale,
            });
        }
        out
    }
}

/// Unity serializes Vector3/Quaternion as {"x":..}; missing fields read as the default.
pub fn vec3(v: &Value) -> Vec3 {
    let f = |k: &str| v[k].as_f64().unwrap_or(0.0) as f32;
    Vec3::new(f("x"), f("y"), f("z"))
}
pub fn quat(v: &Value) -> Quat {
    let f = |k: &str, d: f64| v[k].as_f64().unwrap_or(d) as f32;
    let q = Quat::from_xyzw(f("x", 0.0), f("y", 0.0), f("z", 0.0), f("w", 1.0));
    if q.length_squared() > 1e-12 {
        q.normalize()
    } else {
        Quat::IDENTITY
    }
}
pub fn num(v: &Value) -> f32 {
    // Unity writes +inf (e.g. break force) as null in the sidecar.
    v.as_f64().map_or(f32::INFINITY, |x| x as f32)
}
/// Numeric field with a default for absent/null values.
pub fn num_or(v: &Value, default: f32) -> f32 {
    v.as_f64().map_or(default, |x| x as f32)
}
pub fn flag(v: &Value) -> bool {
    v.as_bool().unwrap_or_else(|| v.as_i64().unwrap_or(0) != 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mirror_round_trips() {
        let q = Quat::from_euler(glam::EulerRot::YXZ, 0.3, -0.7, 1.1);
        assert!(mirror_rotation(mirror_rotation(q)).abs_diff_eq(q, 1e-6));
        // Mirroring is a reflection: rotating a mirrored vector == mirroring the rotated vector.
        let v = Vec3::new(0.2, -1.0, 3.0);
        let a = mirror_rotation(q) * mirror_position(v);
        assert!(a.abs_diff_eq(mirror_position(q * v), 1e-5));
    }

    #[test]
    fn world_poses_compose_parent_scale_and_rotation() {
        let sidecar: Sidecar = serde_json::from_value(serde_json::json!({"nodes": [
            {"path":"a","parent":null,"layer":0,"active_in_hierarchy":true,"components":[],
             "transform":{"translation":[-1,0,0],"rotation":[0,0,0,1],"scale":[2,2,2]}},
            {"path":"a/b","parent":0,"layer":0,"active_in_hierarchy":true,"components":[],
             "transform":{"translation":[0,1,0],"rotation":[0,0,0,1],"scale":[1,1,1]}}]}))
        .unwrap();
        let w = sidecar.world_poses(Pose::IDENTITY);
        assert_eq!(w[0].position, Vec3::new(1.0, 0.0, 0.0)); // un-mirrored
        assert_eq!(w[1].position, Vec3::new(1.0, 2.0, 0.0));
        assert_eq!(w[1].scale, Vec3::splat(2.0));
    }

    #[test]
    fn collision_mask_reads_matrix_rows() {
        let mut m = vec![vec![false; 32]; 32];
        m[8][0] = true;
        m[8][8] = true;
        let s = Settings {
            gravity: [0.0, -20.0, 0.0],
            fixed_timestep: 0.02,
            maximum_timestep: 0.04,
            solver_iterations: 8,
            solver_velocity_iterations: 5,
            bounce_threshold: 2.0,
            sleep_threshold: 0.01,
            contact_offset: 0.01,
            max_depenetration_velocity: 10.0,
            max_angular_speed: 7.0,
            adaptive_force: true,
            enhanced_determinism: false,
            friction_type: "patch".into(),
            solver_type: "pgs".into(),
            broadphase: "abp".into(),
            contacts_generation: "pcm".into(),
            layers: vec![],
            layer_collision_matrix: m,
        };
        assert_eq!(s.collision_mask(8), 1 | 1 << 8);
        assert_eq!(s.collision_mask(0), 0);
    }
}

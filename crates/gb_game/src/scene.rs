//! Validated source data for the viewer and the future physics importer.
use bevy::prelude::*;
use serde::Deserialize;
use serde_json::Value;
use std::{fs, path::Path};

#[derive(Deserialize)]
pub struct SceneData {
    version: u32,
    pub nodes: Vec<SourceNode>,
}

#[derive(Deserialize)]
pub struct SourceNode {
    pub path: String,
    pub parent: Option<usize>,
    pub active_in_hierarchy: bool,
    pub transform: SourceTransform,
    pub components: Vec<SourceComponent>,
    #[serde(default)]
    pub renderer: Option<SourceRenderer>,
    /// GameObject.activeSelf.
    #[serde(default = "yes")]
    pub active: bool,
    #[serde(default)]
    pub rect: Option<SourceRect>,
    /// Renderer.m_StaticBatchInfo: the mesh is a slice of a build-time combined mesh.
    #[serde(default)]
    pub static_batch: Option<serde_json::Value>,
}

fn yes() -> bool {
    true
}

/// uGUI RectTransform layout (Unity units: canvas pixels).
#[derive(Deserialize, Clone, Copy, Debug)]
pub struct SourceRect {
    pub anchor_min: [f32; 2],
    pub anchor_max: [f32; 2],
    pub anchored_position: [f32; 2],
    pub size_delta: [f32; 2],
    pub pivot: [f32; 2],
}

#[derive(Deserialize)]
pub struct SourceRenderer {
    pub enabled: bool,
    #[serde(default)]
    pub lightmap_index: Option<u32>,
    #[serde(default)]
    pub lightmap_scale_offset: Option<[f32; 4]>,
}

#[derive(Clone, Copy)]
pub struct SceneLightmapAssignment {
    pub node: usize,
    pub index: usize,
    pub scale_offset: [f32; 4],
}

#[derive(Deserialize)]
pub struct SourceTransform {
    translation: [f32; 3],
    rotation: [f32; 4],
    scale: [f32; 3],
}

impl SourceTransform {
    pub(crate) fn to_transform(&self) -> Transform {
        Transform {
            translation: Vec3::from_array(self.translation),
            rotation: Quat::from_array(self.rotation),
            scale: Vec3::from_array(self.scale),
        }
    }
}

#[derive(Deserialize)]
pub struct SourceComponent {
    #[serde(rename = "type")]
    pub kind: String,
    pub script: Option<String>,
    pub connected_node: Option<usize>,
    pub data: Value,
}

#[derive(Clone, Copy)]
pub struct SceneSun {
    pub transform: Transform,
    pub color: [f32; 3],
    pub intensity: f32,
    pub casts_shadows: bool,
    /// Light.shadowStrength: shadowed areas keep (1 - strength) of the direct light.
    pub shadow_strength: f32,
}

#[derive(Clone, Copy)]
pub enum SceneLightKind {
    Point,
    Spot,
    Area,
}

#[derive(Clone, Copy)]
pub struct SceneLight {
    pub transform: Transform,
    pub color: [f32; 3],
    pub intensity: f32,
    pub range: f32,
    pub inner_angle: f32,
    pub outer_angle: f32,
    pub casts_shadows: bool,
    pub kind: SceneLightKind,
}

#[derive(Clone, Copy, Debug)]
pub struct SceneReflectionProbe {
    pub cubemap_path_id: u64,
    pub transform: Transform,
}

impl SceneData {
    pub fn reflection_probes(&self) -> Vec<SceneReflectionProbe> {
        let world = self.world_transforms();
        self.nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| node.active_in_hierarchy)
            .filter_map(|(index, node)| {
                let component = node
                    .components
                    .iter()
                    .find(|component| component.kind == "ReflectionProbe")?;
                let data = &component.data;
                let cubemap_path_id = data["m_CustomBakedTexture"]["m_PathID"]
                    .as_u64()
                    .filter(|id| *id != 0)
                    .or_else(|| data["m_BakedTexture"]["m_PathID"].as_u64())?;
                if cubemap_path_id == 0 {
                    return None;
                }
                let vector = |value: &Value| -> Option<Vec3> {
                    Some(Vec3::new(
                        value["x"].as_f64()? as f32,
                        value["y"].as_f64()? as f32,
                        value["z"].as_f64()? as f32,
                    ))
                };
                let box_size = vector(&data["m_BoxSize"])?;
                let mut box_offset = vector(&data["m_BoxOffset"])?;
                // Component data stays in raw Unity space while node transforms are already
                // mirrored by the exporter. Convert this local point before applying the node.
                box_offset.x = -box_offset.x;
                let source = world[index].compute_transform();
                let mut transform = source;
                transform.translation = source.transform_point(box_offset);
                transform.scale = source.scale * box_size;
                Some(SceneReflectionProbe {
                    cubemap_path_id,
                    transform,
                })
            })
            .collect()
    }

    pub fn lightmap_assignments(&self) -> Vec<SceneLightmapAssignment> {
        self.nodes
            .iter()
            .enumerate()
            .filter_map(|(node, source)| {
                let renderer = source.renderer.as_ref()?;
                let index = renderer.lightmap_index?;
                if !renderer.enabled || index >= u16::MAX as u32 {
                    return None;
                }
                // Build-time static batching (Mesh.CombineMeshes with hasLightmapData) already
                // transformed the combined mesh's UV2 by lightmapScaleOffset.
                let scale_offset = if source.static_batch.is_some() {
                    [1.0, 1.0, 0.0, 0.0]
                } else {
                    renderer.lightmap_scale_offset.unwrap_or([1.0, 1.0, 0.0, 0.0])
                };
                Some(SceneLightmapAssignment {
                    node,
                    index: index as usize,
                    scale_offset,
                })
            })
            .collect()
    }

    pub fn load(root: &Path, name: &str) -> Result<Self, String> {
        let path = root.join(format!("{name}.json"));
        let raw = fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        let scene: Self = serde_json::from_slice(&raw).map_err(|e| {
            format!(
                "{}: {e}; re-export with tools/extract/export.py",
                path.display()
            )
        })?;
        scene.validate()?;
        let glb = fs::read(root.join(format!("{name}.glb"))).map_err(|e| e.to_string())?;
        if glb.len() < 20 || &glb[..4] != b"glTF" {
            return Err(format!("{name}: invalid GLB header"));
        }
        let word = |start| u32::from_le_bytes(glb[start..start + 4].try_into().unwrap());
        let size = word(12) as usize;
        if word(4) != 2
            || word(8) as usize != glb.len()
            || word(16) != 0x4e4f534a
            || size > glb.len() - 20
        {
            return Err(format!("{name}: invalid GLB length/version/JSON chunk"));
        }
        let doc: Value = serde_json::from_slice(&glb[20..20 + size]).map_err(|e| e.to_string())?;
        if doc["nodes"].as_array().map(Vec::len) != Some(scene.nodes.len()) {
            return Err(format!("{name}: GLB and sidecar node counts differ"));
        }
        for (index, node) in scene.nodes.iter().enumerate() {
            let gltf = &doc["nodes"][index];
            if gltf["extras"]["gb_node"].as_u64() != Some(index as u64)
                || gltf["translation"] != serde_json::json!(node.transform.translation)
                || gltf["rotation"] != serde_json::json!(node.transform.rotation)
                || gltf["scale"] != serde_json::json!(node.transform.scale)
            {
                // Compare transforms numerically below: source JSON may retain f64 precision.
                if gltf["extras"]["gb_node"].as_u64() != Some(index as u64) {
                    return Err(format!("{name}: missing GLB node identity at {index}"));
                }
                let exported: SourceTransform =
                    serde_json::from_value(gltf.clone()).map_err(|e| e.to_string())?;
                if exported.translation != node.transform.translation
                    || exported.rotation != node.transform.rotation
                    || exported.scale != node.transform.scale
                {
                    return Err(format!("{name}: GLB/sidecar transform mismatch at {index}"));
                }
            }
        }
        Ok(scene)
    }

    fn validate(&self) -> Result<(), String> {
        if self.version != 3 {
            return Err("sidecar version 3 required; re-export the assets".into());
        }
        for (index, node) in self.nodes.iter().enumerate() {
            if node.parent.is_some_and(|p| p >= index) {
                return Err(format!("{}: parent must precede child", node.path));
            }
            let tf = node.transform.to_transform();
            if !tf.translation.is_finite()
                || !tf.rotation.is_finite()
                || !tf.scale.is_finite()
                // Unity's exported UI scenes contain zero-scale RectTransforms (for example the
                // hidden Credits canvas). Preserve those source-authored UI transforms; zero-scale
                // physics/world nodes remain invalid.
                || (node.rect.is_none() && tf.scale.abs().min_element() < 1e-8)
                || (tf.rotation.length_squared() - 1.0).abs() > 0.01
            {
                return Err(format!("{}: invalid transform", node.path));
            }
            for c in &node.components {
                if c.kind == "Rigidbody"
                    && !c.data["m_Mass"]
                        .as_f64()
                        .is_some_and(|v| v.is_finite() && v > 0.0)
                {
                    return Err(format!("{}: invalid mass", node.path));
                }
                if c.kind.ends_with("Joint") {
                    if let Some(target) = c.connected_node {
                        if !self
                            .nodes
                            .get(target)
                            .is_some_and(|n| n.components.iter().any(|c| c.kind == "Rigidbody"))
                        {
                            return Err(format!(
                                "{}: joint references a missing rigidbody",
                                node.path
                            ));
                        }
                    } else if c.data["m_ConnectedBody"]["m_PathID"].as_i64().unwrap_or(0) != 0 {
                        return Err(format!("{}: unresolved joint reference", node.path));
                    }
                }
            }
        }
        Ok(())
    }

    pub fn world_transforms(&self) -> Vec<GlobalTransform> {
        let mut result: Vec<GlobalTransform> = Vec::with_capacity(self.nodes.len());
        for node in &self.nodes {
            let local = node.transform.to_transform();
            result.push(match node.parent {
                Some(parent) => result[parent].mul_transform(local),
                None => GlobalTransform::from(local),
            });
        }
        result
    }

    pub fn directional_sun(&self) -> Option<SceneSun> {
        let world = self.world_transforms();
        self.nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| node.active_in_hierarchy)
            .find_map(|(index, node)| {
                let component = node.components.iter().find(|component| {
                    component.kind == "Light"
                        && component.data["m_Type"].as_u64() == Some(1)
                        && component.data["m_Enabled"].as_u64() == Some(1)
                })?;
                let data = &component.data;
                let color = &data["m_Color"];
                Some(SceneSun {
                    transform: unity_light_to_bevy(world[index].compute_transform()),
                    color: [
                        color["r"].as_f64()? as f32,
                        color["g"].as_f64()? as f32,
                        color["b"].as_f64()? as f32,
                    ],
                    intensity: data["m_Intensity"].as_f64()? as f32,
                    casts_shadows: data["m_Shadows"]["m_Type"].as_u64().unwrap_or(0) > 0,
                    shadow_strength: data["m_Shadows"]["m_Strength"].as_f64().unwrap_or(1.0) as f32,
                })
            })
    }

    pub fn local_lights(&self) -> Vec<SceneLight> {
        let world = self.world_transforms();
        self.nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| node.active_in_hierarchy)
            .filter_map(|(index, node)| {
                let component = node.components.iter().find(|component| {
                    component.kind == "Light" && component.data["m_Enabled"].as_u64() == Some(1)
                })?;
                let kind = match component.data["m_Type"].as_u64()? {
                    0 => SceneLightKind::Spot,
                    2 => SceneLightKind::Point,
                    3 => SceneLightKind::Area,
                    _ => return None,
                };
                let data = &component.data;
                let color = &data["m_Color"];
                let channel = |name: &str| color[name].as_f64().map(|v| v as f32);
                Some(SceneLight {
                    transform: unity_light_to_bevy(world[index].compute_transform()),
                    color: [channel("r")?, channel("g")?, channel("b")?],
                    intensity: data["m_Intensity"].as_f64()? as f32,
                    range: data["m_Range"].as_f64().unwrap_or(10.0) as f32,
                    inner_angle: data["m_InnerSpotAngle"].as_f64().unwrap_or(21.8) as f32,
                    outer_angle: data["m_SpotAngle"].as_f64().unwrap_or(30.0) as f32,
                    casts_shadows: data["m_Shadows"]["m_Type"].as_u64().unwrap_or(0) > 0,
                    kind,
                })
            })
            .collect()
    }

    pub fn spawns(&self) -> Vec<Transform> {
        let world = self.world_transforms();
        // Player spawn scripts differ per stage: Rooftop has plain `GBSpawnPoint`s (the 1-8 solo
        // set); Incinerator has none and uses `GBSpawnCircle`s under `Spawners/Solo`. Both carry
        // `SpawnPointTypes` Player|StartPoint (5). `GBGangSpawnPoint` is deliberately NOT used
        // here: it belongs to the Gang/Waves spawners (`Spawners/Gang 2`, `Spawners/Waves`) and
        // derives several positions per gang, so it is a different mode's spawn system.
        // Containers, Elevators, Gondola and Train only have gang markers: use them rather than nothing.
        for script in ["GBSpawnPoint", "GBSpawnCircle", "GBGangSpawnPoint"] {
            let found: Vec<Transform> = self
                .nodes
                .iter()
                .enumerate()
                .filter(|(_, n)| {
                    n.active_in_hierarchy
                        && n.components
                            .iter()
                            .any(|c| c.script.as_deref() == Some(script))
                })
                .flat_map(|(i, n)| {
                    let centre = world[i].compute_transform();
                    // A GBSpawnCircle places fighters ON a ring of `_raduis` metres (Towers: 9 m
                    // around the tower top); the centre itself is a hole. Small radii stay a point.
                    let radius = n
                        .components
                        .iter()
                        .find(|c| c.script.as_deref() == Some("GBSpawnCircle"))
                        .and_then(|c| c.data["_raduis"].as_f64())
                        .unwrap_or(0.0) as f32;
                    if script == "GBSpawnCircle" && radius > 1.5 {
                        (0..10)
                            .map(|k| {
                                let angle = k as f32 / 10.0 * std::f32::consts::TAU;
                                let mut t = centre;
                                t.translation += Vec3::new(angle.cos(), 0.0, angle.sin()) * radius;
                                // `_lookIn`: face the circle's centre.
                                t.look_at(centre.translation, Vec3::Y);
                                t
                            })
                            .collect::<Vec<_>>()
                    } else {
                        vec![centre]
                    }
                })
                .collect();
            if found.len() >= 4 {
                return found;
            }
            // Some stages (Ring) keep their melee spawn groups switched off in the saved scene
            // and a game-mode script enables them at runtime (GamemodeEnabled / mode managers).
            // With almost nothing active, take every spawn marker of the melee family.
            let melee: Vec<Transform> = self
                .nodes
                .iter()
                .enumerate()
                .filter(|(_, n)| {
                    let path = n.path.to_ascii_lowercase();
                    n.components.iter().any(|c| c.script.as_deref() == Some(script))
                        && !path.contains("rumble")
                        && !path.contains("wave")
                        && !path.contains("gang")
                })
                .map(|(i, _)| world[i].compute_transform())
                .collect();
            if melee.len() > found.len() {
                return melee;
            }
            if !found.is_empty() {
                return found;
            }
        }
        Vec::new()
    }

    pub fn summary(&self, name: &str) -> String {
        let count = |kind: &str| {
            self.nodes
                .iter()
                .flat_map(|n| &n.components)
                .filter(|c| c.kind == kind)
                .count()
        };
        let mass: f64 = self
            .nodes
            .iter()
            .flat_map(|n| &n.components)
            .filter(|c| c.kind == "Rigidbody")
            .filter_map(|c| c.data["m_Mass"].as_f64())
            .sum();
        format!("{name}: {} nodes, {} rigidbodies ({mass:.1} kg), {} configurable joints, {} spawn points",
                self.nodes.len(), count("Rigidbody"), count("ConfigurableJoint"), self.spawns().len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> SceneData {
        serde_json::from_value(serde_json::json!({
        "version":3, "nodes":[
            {"path":"root","parent":null,"active_in_hierarchy":true,
             "transform":{"translation":[10,0,0],"rotation":[0,0,0,1],"scale":[2,2,2]},
             "components":[{"type":"Rigidbody","data":{"m_Mass":10}}]},
            {"path":"root/spawn","parent":0,"active_in_hierarchy":true,
             "transform":{"translation":[1,2,3],"rotation":[0,0,0,1],"scale":[1,1,1]},
             "components":[{"type":"MonoBehaviour","script":"GBSpawnPoint","data":{}}]}
        ]}))
        .unwrap()
    }

    #[test]
    fn spawn_uses_parent_transform() {
        let scene = fixture();
        scene.validate().unwrap();
        assert_eq!(scene.spawns()[0].translation, Vec3::new(12.0, 4.0, 6.0));
    }

    #[test]
    fn inactive_spawn_is_not_used() {
        let mut scene = fixture();
        scene.nodes[1].active_in_hierarchy = false;
        assert!(scene.spawns().is_empty());
    }

    #[test]
    fn lightmap_assignments_keep_source_atlas_and_filter_unlit_renderers() {
        let mut scene = fixture();
        scene.nodes[0].renderer = Some(SourceRenderer {
            enabled: true,
            lightmap_index: Some(1),
            lightmap_scale_offset: Some([0.5, 0.25, 0.125, 0.375]),
        });
        scene.nodes[1].renderer = Some(SourceRenderer {
            enabled: true,
            lightmap_index: Some(u16::MAX as u32),
            lightmap_scale_offset: None,
        });
        let assignments = scene.lightmap_assignments();
        assert_eq!(assignments.len(), 1);
        assert_eq!(assignments[0].node, 0);
        assert_eq!(assignments[0].index, 1);
        assert_eq!(assignments[0].scale_offset, [0.5, 0.25, 0.125, 0.375]);
    }

    #[test]
    fn reflection_probe_uses_world_box_offset_and_size() {
        let mut scene = fixture();
        scene.nodes[1].components.push(SourceComponent {
            kind: "ReflectionProbe".into(),
            script: None,
            connected_node: None,
            data: serde_json::json!({
                "m_CustomBakedTexture": {"m_FileID": 3, "m_PathID": 70},
                "m_BoxOffset": {"x": 1.0, "y": 0.0, "z": -1.0},
                "m_BoxSize": {"x": 3.0, "y": 4.0, "z": 5.0}
            }),
        });
        let probes = scene.reflection_probes();
        assert_eq!(probes.len(), 1);
        assert_eq!(probes[0].cubemap_path_id, 70);
        assert_eq!(probes[0].transform.translation, Vec3::new(10.0, 4.0, 4.0));
        assert_eq!(probes[0].transform.scale, Vec3::new(6.0, 8.0, 10.0));
    }

    #[test]
    fn reflection_probe_falls_back_to_baked_texture() {
        let mut scene = fixture();
        scene.nodes[1].components.push(SourceComponent {
            kind: "ReflectionProbe".into(),
            script: None,
            connected_node: None,
            data: serde_json::json!({
                "m_CustomBakedTexture": {"m_FileID": 0, "m_PathID": 0},
                "m_BakedTexture": {"m_FileID": 2, "m_PathID": 54},
                "m_BoxOffset": {"x": 0.0, "y": 0.0, "z": 0.0},
                "m_BoxSize": {"x": 1.0, "y": 1.0, "z": 1.0}
            }),
        });
        let probes = scene.reflection_probes();
        assert_eq!(probes.len(), 1);
        assert_eq!(probes[0].cubemap_path_id, 54);
    }

    #[test]
    fn local_lights_use_world_transforms_and_source_properties() {
        let mut scene = fixture();
        scene.nodes[1].components.push(SourceComponent {
            kind: "Light".into(),
            script: None,
            connected_node: None,
            data: serde_json::json!({
                "m_Enabled": 1,
                "m_Type": 0,
                "m_Intensity": 8.0,
                "m_Range": 12.0,
                "m_InnerSpotAngle": 15.0,
                "m_SpotAngle": 70.0,
                "m_Color": {"r": 1.0, "g": 0.9, "b": 0.7},
                "m_Shadows": {"m_Type": 2}
            }),
        });
        let lights = scene.local_lights();
        assert_eq!(lights.len(), 1);
        assert_eq!(lights[0].transform.translation, Vec3::new(12.0, 4.0, 6.0));
        assert_eq!(lights[0].color, [1.0, 0.9, 0.7]);
        assert_eq!(lights[0].range, 12.0);
        assert!(matches!(lights[0].kind, SceneLightKind::Spot));
        assert!(lights[0].casts_shadows);
    }

    #[test]
    fn invalid_hierarchy_is_rejected() {
        let mut scene = fixture();
        scene.nodes[0].parent = Some(1);
        assert!(scene.validate().unwrap_err().contains("parent"));
    }

    #[test]
    fn joint_must_resolve_to_a_rigidbody() {
        let mut scene = fixture();
        scene.nodes[1].components.push(SourceComponent {
            kind: "ConfigurableJoint".into(),
            script: None,
            connected_node: Some(1),
            data: Value::Null,
        });
        assert!(scene.validate().unwrap_err().contains("rigidbody"));
        scene.nodes[1].components.last_mut().unwrap().connected_node = Some(0);
        scene.validate().unwrap();
    }
}

/// Sidecar node transforms are already mirrored into Bevy space by the exporter (X negated,
/// quaternion (x, -y, -z, w)); only the light direction differs: Unity lights shine along their
/// +Z, Bevy lights along -Z.
pub fn unity_light_to_bevy(t: Transform) -> Transform {
    let forward = t.rotation * Vec3::Z;
    let up = t.rotation * Vec3::Y;
    Transform::from_translation(t.translation).looking_to(forward, up)
}

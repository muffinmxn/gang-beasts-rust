//! Gang Beasts' `OpaqueSurfaceFog` URP renderer feature (OpaqueSurfaceFogSceneSettings): a sky
//! gradient from the fog colour at the horizon to the sky colour above `skyFogElevation`, and
//! fog that fades surfaces toward the fog colour with distance (`skyFogMaxDepth`).
//!
//! The feature's shader isn't ported; this reproduces its serialized parameters with a
//! vertex-coloured sky dome plus Bevy's distance fog.
use crate::scene::SceneData;
use bevy::prelude::*;
use bevy::render::mesh::VertexAttributeValues;

/// Reads an `f32` env override (harness tuning without a rebuild).
fn env_f32(key: &str, default: f32) -> f32 {
    std::env::var(key).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

#[derive(Clone, Copy, Debug)]
pub struct SurfaceFog {    pub sky: Color,
    pub fog: Color,
    pub sky_max_depth: f32,
    pub sky_elevation: f32,
    pub ground_max_depth: f32,
    pub ground_elevation: f32,
    /// fogLightScatterIntensity: fog brightens toward the sun.
    pub scatter: f32,
}

impl SurfaceFog {
    pub fn from_scene(scene: &SceneData) -> Option<Self> {
        let s = scene
            .nodes
            .iter()
            .filter(|n| n.active_in_hierarchy)
            .find_map(|n| {
                n.components
                    .iter()
                    .find(|c| c.script.as_deref() == Some("OpaqueSurfaceFogSceneSettings"))
                    .map(|c| c.data["settings"].clone())
            })?;
        let color = |k: &str| {
            let c = &s[k];
            let f = |x: &str| c[x].as_f64().unwrap_or(0.0) as f32;
            Color::srgb(f("r"), f("g"), f("b"))
        };
        let num = |k: &str, d: f32| s[k].as_f64().map_or(d, |v| v as f32);
        Some(Self {
            sky: color("skyColor"),
            fog: color("fogColor"),
            sky_max_depth: num("skyFogMaxDepth", 350.0),
            sky_elevation: num("skyFogElevation", 25.0),
            ground_max_depth: num("groundFogMaxDepth", 60.0),
            ground_elevation: num("groundFogElevation", 20.0),
            scatter: num("fogLightScatterIntensity", 0.0),
        })
    }

    /// Sky colour for a view direction `elevation` degrees above the horizon.
    pub fn sky_color(&self, elevation: f32) -> LinearRgba {
        let (fog, sky) = (self.fog.to_linear(), self.sky.to_linear());
        // Mirrored below the horizon: looking down past the roof edge shows deep sky, not a
        // fog-coloured glare (the +1.4 EV post exposure turns the fog colour near-white).
        let t = (elevation.abs() / self.sky_elevation.max(1e-3)).clamp(0.0, 1.0);
        fog.mix(&sky, t)
    }

    pub fn distance_fog(&self) -> DistanceFog {
        // `fogLightScatterIntensity` is the source's own strength: Rooftop ships 1.0 and its
        // references show aerial perspective on nearby surfaces, while low-scatter stages only
        // want a faint tint toward the far city. The pink is the fog colour mixed toward white
        // (the +1.4 EV post exposure lifts it anyway) and starting further out, so surfaces up
        // close stay neutral instead of turning magenta.
        let white_mix: f32 = env_f32("GB_FOG_WHITE", 0.75).clamp(0.0, 1.0);
        let start_scale: f32 = env_f32("GB_FOG_START_SCALE", 0.9);
        let end_scale: f32 = env_f32("GB_FOG_END_SCALE", 3.0);
        let (color, start, end) = if self.scatter >= 0.5 {
            (
                self.fog.mix(&Color::WHITE, white_mix),
                self.ground_max_depth * start_scale,
                (self.ground_max_depth * end_scale).max(1.0),
            )
        } else {
            (self.fog.mix(&Color::WHITE, 0.5), self.ground_max_depth, self.sky_max_depth * 3.0)
        };
        DistanceFog {
            color,
            falloff: FogFalloff::Linear { start, end },
            ..default()
        }
    }
}

#[derive(Component)]
pub struct SkyDome;

pub fn spawn_dome(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    fog: &SurfaceFog,
) {
    let mut mesh = Sphere::new(1.0).mesh().uv(64, 32);
    let colors: Vec<[f32; 4]> = match mesh.attribute(Mesh::ATTRIBUTE_POSITION) {
        Some(VertexAttributeValues::Float32x3(p)) => p
            .iter()
            .map(|v| {
                let elevation = v[1].clamp(-1.0, 1.0).asin().to_degrees();
                fog.sky_color(elevation).to_f32_array()
            })
            .collect(),
        _ => vec![],
    };
    mesh.insert_attribute(Mesh::ATTRIBUTE_COLOR, colors);
    commands.spawn((
        Mesh3d(meshes.add(mesh)),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::WHITE,
            unlit: true,
            fog_enabled: false,
            cull_mode: None,
            ..default()
        })),
        Transform::from_scale(Vec3::splat(20_000.0)),
        bevy::pbr::NotShadowCaster,
        bevy::pbr::NotShadowReceiver,
        bevy::render::view::NoFrustumCulling,
        SkyDome,
    ));
}

/// Keep the dome centred on the camera.
pub fn follow_camera(
    cams: Query<&GlobalTransform, (With<Camera3d>, Without<SkyDome>)>,
    mut domes: Query<&mut Transform, With<SkyDome>>,
) {
    let Some(cam) = cams.iter().next() else {
        return;
    };
    for mut t in &mut domes {
        t.translation = cam.translation();
    }
}

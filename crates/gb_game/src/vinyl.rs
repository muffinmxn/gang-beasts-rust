//! Menu-scoped port of the source VinylOrMetal material graph.
//! Register provenance and remaining lighting work: re/shaders/VINYL_PORT.md.
#![allow(dead_code)] // ShaderType derive emits unused per-field helpers
use bevy::{gltf::GltfMaterialExtras, pbr::{ExtendedMaterial, MaterialExtension}, prelude::*,
    render::render_resource::{AsBindGroup, ShaderRef, ShaderType}, scene::{SceneInstance, SceneSpawner}};
use serde_json::Value;
use std::collections::HashMap;

const SHADER: Handle<Shader> = bevy::asset::weak_handle!("c3f8ac1e-9d81-4b14-9d39-f51927d968d8");
/// The menu/stage VinylOrMetal material: the converted StandardMaterial plus the ported graph.
pub type VinylMaterial = ExtendedMaterial<StandardMaterial, Vinyl>;

/// Scales the specular reflectance of every converted vinyl material. Used by the graphics
/// settings screen's Screen Space Reflection row (the ported stand-in: real SSR is not in Bevy
/// 0.16, so the row brightens/dulls the specular response; `1.0` restores the authored level).
pub fn scale_vinyl_reflectance(materials: &mut Assets<VinylMaterial>, scale: f32) {
    for (_, material) in materials.iter_mut() {
        material.base.reflectance = (material.base.reflectance * scale).clamp(0.0, 1.0);
    }
}

#[derive(Component)]
pub struct MenuScenery;

/// Whether the vinyl shader replaces Bevy's directional BRDF with the ported URP one.
///
/// The replacement is a subtract-then-add correction that assumes the manually built
/// `LightingInput` matches Bevy's internal one. On stages it does not: the correction comes
/// out negative and cancels the sun (measured on Rooftop: `shadow_fill 1.0` moved the floor
/// 0 pixels through the vinyl path, but 100 -> 155 through the plain StandardMaterial path).
/// The menu was tuned with the replacement ON, so this stays on for the menu and off for
/// stages until the correction is rewritten against Bevy's lighting struct.
#[derive(Resource, Clone, Copy, Debug)]
pub struct UrpDirect(pub bool);

impl Default for UrpDirect {
    fn default() -> Self {
        Self(false)
    }
}

/// The exported graphics JSON the vinyl SH ambient probe is read from. Set per stage
/// (`menu-graphics.json`, `rooftop-graphics.json`, ...) so every map evaluates its own
/// source ambient probe instead of the menu's.
#[derive(Resource, Clone, Debug, Default)]
pub struct ProbeSource(pub Option<String>);

/// The baked-lightmap exposure written into every vinyl material when it is created. Updating
/// `lightmap_exposure` on an existing material asset never reached the GPU uniform, so the lightmapped
/// walls were sampled with exposure 0 (black); the stage's exposure is therefore known up front.
#[derive(Resource, Clone, Copy, Debug, Default)]
pub struct LightmapExposure(pub f32);

#[derive(Clone, Copy, Debug, ShaderType)]
pub struct VinylSettings {
    normal_uv: Vec4,
    normal_strength: f32,
    /// Flip the normal map's Y channel (gbrender: Unity's V axis points up, so without this
    /// "brick highlights were on the bottom edge"). -1.0 = flipped, 1.0 = raw. GB_NORMAL_Y.
    normal_y: f32,
    use_vertex_color: u32,
    urp_direct: u32,
    /// Shader Graph Triplanar: x = enabled (>0.5), y = world scale, z = blend, w = normal strength.
    triplanar: Vec4,
    /// 1 = evaluate the source SH ambient probe per pixel instead of Bevy's flat ambient.
    sh_ambient: u32,
    /// Unity SphericalHarmonicsL2, planar per channel: [0..9) R, [9..18) G, [18..27) B.
    sh: [Vec4; 9],
    /// Flat ambient Bevy already applies (linear RGB), subtracted so the SH term replaces it.
    flat_ambient: Vec4,
    /// Scale from Unity SH radiance to Bevy's ambient units (tuned; see GB_SH_SCALE).
    sh_scale: f32,
    _pad: Vec3,
}

#[derive(Asset, AsBindGroup, TypePath, Debug, Clone)]
pub struct Vinyl {
    #[uniform(100)]
    settings: VinylSettings,
    /// Triplanar normal map (Shader Graphs/Triplanar). Binding 101/102 must exist to match the
    /// WGSL layout even when triplanar is disabled; the default image is never sampled then.
    #[texture(101)]
    #[sampler(102)]
    triplanar_normal: Handle<Image>,
}

impl MaterialExtension for Vinyl {
    fn fragment_shader() -> ShaderRef { SHADER.into() }
    fn specialize(
        _pipeline: &bevy::pbr::MaterialExtensionPipeline,
        descriptor: &mut bevy::render::render_resource::RenderPipelineDescriptor,
        _layout: &bevy::render::mesh::MeshVertexBufferLayoutRef,
        _key: bevy::pbr::MaterialExtensionKey<Self>,
    ) -> Result<(), bevy::render::render_resource::SpecializedMeshPipelineError> {
        // The SH ambient term is only meaningful when the source probe was exported.
        if std::env::var_os("GB_NO_SH_AMBIENT").is_none() {
            descriptor.vertex.shader_defs.push("SH_AMBIENT".into());
            if let Some(fragment) = descriptor.fragment.as_mut() {
                fragment.shader_defs.push("SH_AMBIENT".into());
            }
        }
        Ok(())
    }
}

pub fn plugin(app: &mut App) {
    bevy::asset::load_internal_asset!(app, SHADER, "shaders/vinyl.wgsl", Shader::from_wgsl);
    app.add_plugins(MaterialPlugin::<VinylMaterial>::default())
        .add_systems(Update, convert.after(crate::mips::vinyl_roughness));
}

fn graph_roughness(smoothness: f32) -> f32 {
    1.0 - (0.99 * smoothness).powi(2).clamp(0.0, 1.0)
}

fn color(value: &Value, fallback: Color) -> Color {
    let Some(v) = value.as_array().filter(|v| v.len() == 4) else { return fallback; };
    Color::srgba(v[0].as_f64().unwrap_or(1.0) as f32, v[1].as_f64().unwrap_or(1.0) as f32,
        v[2].as_f64().unwrap_or(1.0) as f32, v[3].as_f64().unwrap_or(1.0) as f32)
}

/// Unity SphericalHarmonicsL2 (27 floats, planar per channel) -> the shader's packed form.
fn pack_sh(values: &[f32]) -> ([Vec4; 9], bool) {
    if values.len() < 27 {
        return ([Vec4::ZERO; 9], false);
    }
    let mut sh = [Vec4::ZERO; 9];
    for i in 0..9 {
        let r = values.get(i).copied().unwrap_or(0.0);
        let g = values.get(9 + i).copied().unwrap_or(0.0);
        let b = values.get(18 + i).copied().unwrap_or(0.0);
        sh[i] = Vec4::new(r, g, b, 0.0);
    }
    (sh, true)
}

fn convert(
    mut commands: Commands,
    roots: Query<&SceneInstance, With<MenuScenery>>,
    spawner: Res<SceneSpawner>,
    entities: Query<(
        &MeshMaterial3d<StandardMaterial>,
        &GltfMaterialExtras,
        Option<&bevy::gltf::GltfMaterialName>,
    )>,
    materials: Res<Assets<StandardMaterial>>,
    mut extended: ResMut<Assets<VinylMaterial>>,
    mut converted: Local<HashMap<AssetId<StandardMaterial>, Handle<VinylMaterial>>>,
    mut sh_cache: Local<Option<([Vec4; 9], bool)>>,
    probe_source: Res<ProbeSource>,
    lightmap_exposure: Res<LightmapExposure>,
    urp_direct: Res<UrpDirect>,
    ambient: Res<AmbientLight>,
) {
    if std::env::var_os("GB_VINYL_LEGACY").is_some() { return; }
    let (sh, sh_ok) = *sh_cache.get_or_insert_with(|| {
        let values = probe_source
            .0
            .as_ref()
            .and_then(|path| std::fs::read(path).ok())
            .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
            .and_then(|g| {
                let probe = &g["render_settings"]["ambient_probe"];
                let mut out = Vec::with_capacity(27);
                for i in 0..27 {
                    out.push(probe[format!("sh[{i:2}]")].as_f64().unwrap_or(0.0) as f32);
                }
                Some(out)
            })
            .unwrap_or_default();
        pack_sh(&values)
    });
    // Global `_Smoothness` scale (GB_SMOOTHNESS_SCALE). The source graph's specular reads a
    // hair hotter than the reference captures on broadly-lit surfaces; until the exact
    // VinylOrMetal specular path is ported (GRAPHICS_PLAN Phase 2), this one knob brings
    // every vinyl surface down together. 1.0 = authored. Sweep to settle it, then remove
    // with the specular port.
    let smoothness_scale: f32 = crate::devgui::knob("GB_SMOOTHNESS_SCALE", 1.0).clamp(0.0, 2.0);
    for root in &roots {
        for entity in spawner.iter_instance_entities(**root) {
            let Ok((handle, extras, material_name)) = entities.get(entity) else { continue; };
            let Ok(data) = serde_json::from_str::<Value>(&extras.value) else { continue; };
            if !data["shader"].as_str().is_some_and(|s| s.starts_with("GangBeasts/Surface/VinylOrMetal")) { continue; }
            let id = handle.0.id();
            let new_handle = if let Some(handle) = converted.get(&id) { handle.clone() } else {
                let Some(original) = materials.get(id) else { continue; };
                let mut base = original.clone();
                // gbrender's wall calibration (`gbrender/look/menu.json`, measured against the
                // in-game screenshots): the Alley's brick walls are much more matte than the
                // authored smoothness, with a softer normal and a slight overall tint. Adopted
                // per material name so only the walls change.
                let wall = material_name.is_some_and(|name| {
                    matches!(
                        name.0.as_str(),
                        "wall_01_diffuse" | "wall_02_diffuse" | "Wall_crazynormals"
                    )
                });
                // gbrender's wall NORMAL scale (0.6) is transferable on its own: it softens the
                // authored bump to their measured level. Their albedo tint and smoothness are
                // NOT - they assume the rest of their look stack (gi_scale / white balance) and
                // cost us 1.3% match plus 1.7 L when applied alone.
                let wall_bump = if wall { 0.6 } else { 1.0 };
                let f = |key: &str, default: f32| data["floats"][key].as_f64().map_or(default, |v| v as f32);
                base.base_color = color(&data["colors"]["_BaseColor"], base.base_color);
                base.lightmap_exposure = lightmap_exposure.0;
                base.perceptual_roughness =
                    graph_roughness(f("_Smoothness", 0.2) * smoothness_scale);
                base.metallic = f("_Metallic", 0.0).clamp(0.0, 1.0);
                // HDR emission colors are already linear. Unity does not multiply them by
                // Bevy's physical-camera exposure, only by the later URP post exposure.
                if let Some(e) = data["colors"]["_EmissionColor"].as_array() {
                    base.emissive = LinearRgba::rgb(e[0].as_f64().unwrap_or(0.0) as f32,
                        e[1].as_f64().unwrap_or(0.0) as f32, e[2].as_f64().unwrap_or(0.0) as f32);
                }
                base.emissive_exposure_weight = 0.0;
                let clipped = data["shader"].as_str().is_some_and(|s| s.ends_with("AlphaClipped"));
                let (tiling, offset) = if clipped {
                    ("Vector2_c1d98f255e2b4b66a2442f28c9bb5167", "Vector2_0e9a27fc14924124956beb4adf60ee25")
                } else {
                    ("Vector2_6e77f4833d7d4468b553e075d1c97eca", "Vector2_e8e7b4d263ac413d836c3db50597fe12")
                };
                let c = |key: &str, i: usize, default: f32| data["colors"][key][i].as_f64().map_or(default, |x| x as f32);
                let sy = c(tiling, 1, 1.0);
                let extension = Vinyl { settings: VinylSettings {
                    normal_uv: Vec4::new(c(tiling, 0, 1.0), sy, c(offset, 0, 0.0), 1.0 - sy - c(offset, 1, 0.0)),
                    normal_strength: f("_BumpScale", 1.0) * wall_bump,
                    normal_y: std::env::var("GB_NORMAL_Y").ok().and_then(|v| v.parse().ok()).unwrap_or(-1.0),
                    use_vertex_color: u32::from(f("Boolean_53813866627c4202933cbfd81bb5ac80", 0.0) != 0.0),
                    urp_direct: u32::from(
                        urp_direct.0 && std::env::var_os("GB_VINYL_BEVY_BRDF").is_none(),
                    ),
                    // Disabled until the Shader Graphs/Triplanar source texture is exported; the
                    // binding must still exist to match the WGSL layout (see Vinyl::triplanar_normal).
                    triplanar: Vec4::ZERO,
                    sh_ambient: u32::from(sh_ok && std::env::var_os("GB_NO_SH_AMBIENT").is_none()),
                    sh,
                    // Bevy's flat ambient (linear RGB x brightness) that the SH term replaces.
                    flat_ambient: {
                        let c = ambient.color.to_linear();
                        Vec4::new(c.red, c.green, c.blue, 0.0) * ambient.brightness
                    },
                    sh_scale: crate::devgui::knob("GB_SH_SCALE", 0.45),
                    _pad: Vec3::ZERO,
                }, triplanar_normal: Handle::default() };
                let handle = extended.add(ExtendedMaterial { base, extension });
                converted.insert(id, handle.clone());
                handle
            };
            commands.entity(entity).remove::<MeshMaterial3d<StandardMaterial>>()
                .insert(MeshMaterial3d(new_handle));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_smoothness_is_squared_before_urp_brdf() {
        assert!((graph_roughness(0.4) - 0.843184).abs() < 1e-6);
        assert!((graph_roughness(0.5) - 0.754975).abs() < 1e-6);
        assert!((graph_roughness(1.0) - 0.0199).abs() < 1e-6);
    }
}

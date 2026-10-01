//! Stage material conversion for the gbrender engine path (`GB_ENGINE=1`).
//!
//! Walks the stage's glTF scene and swaps every `StandardMaterial` for `GbMaterial`, carrying
//! over the base/normal/emissive textures and alpha settings. Lightmap slices, shadowmasks,
//! cubemaps and the ambient probe are wired in the follow-up step (see
//! `handoff/GBRENDER_TRANSFER_PLAN.md` step 3); until then the gb path renders with realtime
//! lighting only, which is exactly the A/B the plan calls for.

use bevy::gltf::GltfMaterialExtras;
use bevy::pbr::MeshMaterial3d;
use bevy::prelude::*;
use bevy::scene::SceneInstance;

use crate::gb_engine::material::{GbMaterial, GbParams, F_ALPHACLIP, F_BASE_TEX, F_EMISSIVE_TEX,
    F_NORMALMAP, F_RECEIVE_SHADOWS};

pub struct GbConvertPlugin;

impl Plugin for GbConvertPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, convert_stage_materials);
    }
}

fn convert_stage_materials(
    mut commands: Commands,
    roots: Query<&SceneInstance, With<crate::play::PhysicsScene>>,
    spawner: Res<SceneSpawner>,
    entities: Query<(&MeshMaterial3d<StandardMaterial>, &GltfMaterialExtras)>,
    materials: Res<Assets<StandardMaterial>>,
    mut gb_materials: ResMut<Assets<GbMaterial>>,
    mut converted: Local<std::collections::HashMap<AssetId<StandardMaterial>, Handle<GbMaterial>>>,
) {
    for instance in &roots {
        for entity in spawner.iter_instance_entities(**instance) {
            let Ok((handle, extras)) = entities.get(entity) else { continue };
            let id = handle.0.id();
            let new_handle = if let Some(existing) = converted.get(&id) {
                existing.clone()
            } else {
                let Some(source) = materials.get(id) else { continue };
                // Always bind a texture at every slot (1x1 fallbacks), like the vinyl path: one
                // bind-group layout for every material, gated by flags.
                let mut params = GbParams::default();
                params.base_color = {
                    let c = source.base_color.to_linear();
                    Vec4::new(c.red, c.green, c.blue, c.alpha)
                };
                params.emissive = Vec4::new(
                    source.emissive.red,
                    source.emissive.green,
                    source.emissive.blue,
                    0.0,
                );
                // Their packing: surface = (metallic, smoothness, bump_scale, cutoff).
                // extra = (gi_scale, reflection, vertex_color, unused).
                // fog_sky = (gi_saturation, gi_warmth, normal_y, tex_bias).
                params.surface = Vec4::new(
                    source.metallic,
                    1.0 - source.perceptual_roughness,
                    1.0,
                    0.5,
                );
                params.extra = Vec4::new(1.0, 1.0, 1.0, 1.0);
                params.fog_sky = Vec4::new(1.0, 0.0, -1.0, 0.0);
                let mut flags = F_RECEIVE_SHADOWS;
                if source.base_color_texture.is_some() {
                    flags |= F_BASE_TEX;
                }
                if source.normal_map_texture.is_some() {
                    flags |= F_NORMALMAP;
                }
                if source.emissive_texture.is_some() {
                    flags |= F_EMISSIVE_TEX;
                }
                if !matches!(source.alpha_mode, AlphaMode::Opaque) {
                    flags |= F_ALPHACLIP;
                }
                params.flags = flags;
                let material = GbMaterial {
                    params,
                    base: source.base_color_texture.clone(),
                    normal: source.normal_map_texture.clone(),
                    lightmap: None,
                    shadowmask: None,
                    reflection: None,
                    emissive: source.emissive_texture.clone(),
                    alpha_mode: source.alpha_mode,
                    double_sided: source.double_sided,
                };
                let handle = gb_materials.add(material);
                converted.insert(id, handle.clone());
                handle
            };
            let _ = extras;
            commands
                .entity(entity)
                .remove::<MeshMaterial3d<StandardMaterial>>()
                .insert(MeshMaterial3d(new_handle));
        }
    }
}

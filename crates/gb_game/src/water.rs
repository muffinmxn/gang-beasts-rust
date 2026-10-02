//! Water4 seas: Gerstner waves + animated wave normals on top of the tuned sea `StandardMaterial`
//! (`ExtendedMaterial`). Parameters per sea come from `water-params.json` (tools/extract/water_params.py).
#![allow(dead_code)] // ShaderType derive emits unused per-field helpers
use bevy::{
    gltf::GltfMaterialName,
    image::{ImageAddressMode, ImageLoaderSettings, ImageSampler, ImageSamplerDescriptor},
    pbr::{ExtendedMaterial, MaterialExtension, MaterialPlugin},
    prelude::*,
    render::render_resource::{AsBindGroup, ShaderRef, ShaderType},
};
use std::{collections::HashMap, path::Path};

const SHADER: Handle<Shader> = bevy::asset::weak_handle!("5d0b3b2e-62a3-4d3c-9f7f-2f2d6f6e8a11");

#[derive(Clone, Copy, Debug, ShaderType)]
pub struct WaterParams {
    pub amp: Vec4,
    pub freq: Vec4,
    pub steep: Vec4,
    pub speed: Vec4,
    pub dir_ab: Vec4,
    pub dir_cd: Vec4,
    pub bump_tiling: Vec4,
    pub bump_dir: Vec4,
    /// x gerstner intensity, y time scale, z bump strength, w foam amount.
    pub misc: Vec4,
}

#[derive(Asset, AsBindGroup, TypePath, Debug, Clone)]
pub struct WaterExt {
    #[uniform(100)]
    pub params: WaterParams,
    #[texture(101)]
    #[sampler(102)]
    pub bump: Option<Handle<Image>>,
}

impl MaterialExtension for WaterExt {
    fn vertex_shader() -> ShaderRef {
        SHADER.into()
    }
    fn fragment_shader() -> ShaderRef {
        SHADER.into()
    }
}

pub type WaterMaterial = ExtendedMaterial<StandardMaterial, WaterExt>;

#[derive(Resource)]
struct WaterDb {
    seas: HashMap<String, serde_json::Value>,
    bump: Handle<Image>,
}

#[derive(Component)]
struct WaterDone;

/// The active sea's wave parameters, so floating bodies ride the same waves the shader draws.
#[derive(Resource, Clone, Copy)]
pub struct SeaWaves(pub WaterParams);

impl SeaWaves {
    /// Surface height offset at a Bevy-world (x, z) at `t` seconds (the shader's `gerstner(...).y * intensity`).
    pub fn height(&self, x: f32, z: f32, t: f32) -> f32 {
        let p = &self.0;
        let xz = Vec2::new(-x, z);
        let dots = p.freq
            * Vec4::new(
                Vec2::new(p.dir_ab.x, p.dir_ab.y).dot(xz),
                Vec2::new(p.dir_ab.z, p.dir_ab.w).dot(xz),
                Vec2::new(p.dir_cd.x, p.dir_cd.y).dot(xz),
                Vec2::new(p.dir_cd.z, p.dir_cd.w).dot(xz),
            );
        let ph = dots + p.speed * (t * p.misc.y);
        (Vec4::new(ph.x.sin(), ph.y.sin(), ph.z.sin(), ph.w.sin()).dot(p.amp)) * p.misc.x
    }
}

pub fn plugin(app: &mut App, root: &Path) {
    if std::env::var_os("GB_NO_WATER_WAVES").is_some() {
        return;
    }
    let seas: HashMap<String, serde_json::Value> = std::fs::read(root.join("water-params.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_default();
    if seas.is_empty() {
        return; // not extracted: keep the plain sea
    }
    bevy::asset::load_internal_asset!(app, SHADER, "shaders/water.wgsl", Shader::from_wgsl);
    let bump = app.world().resource::<AssetServer>().load_with_settings("water_bump.png", |s: &mut ImageLoaderSettings| {
        s.sampler = ImageSampler::Descriptor(ImageSamplerDescriptor {
            address_mode_u: ImageAddressMode::Repeat,
            address_mode_v: ImageAddressMode::Repeat,
            ..ImageSamplerDescriptor::linear()
        });
    });
    app.add_plugins(MaterialPlugin::<WaterMaterial>::default())
        .insert_resource(WaterDb { seas, bump })
        .add_systems(Update, convert_seas);
}

fn v4(sea: &serde_json::Value, key: &str, default: [f32; 4]) -> Vec4 {
    let a = &sea["colors"][key];
    let g = |i: usize, d: f32| a[i].as_f64().map_or(d, |v| v as f32);
    Vec4::new(g(0, default[0]), g(1, default[1]), g(2, default[2]), g(3, default[3]))
}

fn knob(name: &str, default: f32) -> f32 {
    std::env::var(name).ok().and_then(|v| v.parse().ok()).unwrap_or(default)
}

/// Swap each tuned sea tile's material for the wave material (after `play`'s sea tuning made it translucent).
fn convert_seas(
    mut commands: Commands,
    tiles: Query<(Entity, &MeshMaterial3d<StandardMaterial>, &GltfMaterialName), Without<WaterDone>>,
    standard: Res<Assets<StandardMaterial>>,
    mut water: ResMut<Assets<WaterMaterial>>,
    db: Res<WaterDb>,
    mut cache: Local<HashMap<AssetId<StandardMaterial>, Handle<WaterMaterial>>>,
) {
    for (e, handle, name) in &tiles {
        if !name.0.starts_with("Water - ") {
            continue;
        }
        let Some(base) = standard.get(&handle.0) else { continue };
        if !matches!(base.alpha_mode, AlphaMode::Blend) {
            continue; // play's sea tuning has not run yet
        }
        let Some(sea) = db.seas.iter().find(|(k, _)| k.eq_ignore_ascii_case(&name.0)).map(|(_, v)| v) else {
            commands.entity(e).insert(WaterDone);
            continue;
        };
        let new = cache
            .entry(handle.0.id())
            .or_insert_with(|| {
                let params = WaterParams {
                    amp: v4(sea, "_GAmplitude", [0.14, 0.1, 0.175, 0.225]),
                    freq: v4(sea, "_GFrequency", [3.0, 1.0, 0.5, 0.5]),
                    steep: v4(sea, "_GSteepness", [0.25; 4]),
                    speed: v4(sea, "_GSpeed", [1.0; 4]),
                    dir_ab: v4(sea, "_GDirectionAB", [0.4, 0.2, -0.2, 0.1]),
                    dir_cd: v4(sea, "_GDirectionCD", [0.5, 0.0, 0.0, -0.2]),
                    bump_tiling: v4(sea, "_BumpTiling", [0.15; 4]),
                    bump_dir: v4(sea, "_BumpDirection", [-20.0, -20.0, 10.0, 10.0]),
                    misc: Vec4::new(
                        sea["floats"]["_GerstnerIntensity"].as_f64().unwrap_or(1.0) as f32 * knob("GB_WAVE_HEIGHT", 0.3),
                        knob("GB_WAVE_TIME", 1.0),
                        knob("GB_WAVE_BUMP", 0.35),
                        knob("GB_WAVE_FOAM", 0.5),
                    ),
                };
                commands.insert_resource(SeaWaves(params));
                water.add(WaterMaterial {
                    base: base.clone(),
                    extension: WaterExt { params, bump: Some(db.bump.clone()) },
                })
            })
            .clone();
        commands
            .entity(e)
            .remove::<MeshMaterial3d<StandardMaterial>>()
            .insert((MeshMaterial3d(new), WaterDone));
    }
}

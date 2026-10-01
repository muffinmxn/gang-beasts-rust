//! Readers for the Gang Beasts export: `<level>-graphics.json` (URP render settings,
//! lightmaps, probes, post volume) and `<level>.json` (Unity scene graph with
//! component data). Also decodes Unity lightmap encodings and BC6H cubemaps.
//!
//! Coordinate note: the exporter converts Unity (left-handed) to glTF by negating X.
//! A Unity direction (x, y, z) is (-x, y, z) here, and a Unity object's local +Z
//! (its "forward") is still local +Z. Bevy cameras/lights look down local -Z, so we
//! rotate those by 180 degrees about Y.

use anyhow::{Context, Result};
use bevy::math::{Mat4, Quat, Vec3, Vec4};
use serde_json::Value;
use std::path::Path;

pub fn load_json(path: &Path) -> Result<Value> {
    let text = std::fs::read_to_string(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(serde_json::from_str(&text)?)
}

pub fn f(v: &Value) -> f32 {
    v.as_f64().unwrap_or(0.0) as f32
}

pub fn color(v: &Value) -> [f32; 4] {
    [f(&v["r"]), f(&v["g"]), f(&v["b"]), v.get("a").map(f).unwrap_or(1.0)]
}

pub fn srgb_to_linear(c: f32) -> f32 {
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

pub fn color_linear(v: &Value) -> Vec4 {
    let c = color(v);
    Vec4::new(srgb_to_linear(c[0]), srgb_to_linear(c[1]), srgb_to_linear(c[2]), c[3])
}

// ---------------------------------------------------------------- graphics.json

#[derive(Clone, Debug)]
pub struct LightmapEncoding {
    pub kind: String,
    pub exposure: f32,
    pub exponent: f32,
}

#[derive(Clone, Debug, Default)]
pub struct PostSettings {
    pub post_exposure_ev: f32,
    pub contrast: f32,
    pub saturation: f32,
    /// URP TonemappingMode: 0 none, 1 neutral, 2 ACES
    pub tonemapping: i32,
    pub bloom_threshold: f32,
    pub bloom_intensity: f32,
    pub bloom_scatter: f32,
}

#[derive(Clone, Debug)]
pub struct ReflectionCube {
    pub file: String,
    pub size: u32,
    pub mips: u32,
    pub format: i64,
    pub intensity: f32,
    pub center: Option<Vec3>,
    pub box_size: Option<Vec3>,
}

#[derive(Clone, Debug)]
pub struct Graphics {
    /// Unity SH packed as the 7 shader constants unity_SHAr..unity_SHC (Unity space).
    pub sh: [Vec4; 7],
    pub ambient_intensity: f32,
    pub lightmap_images: Vec<String>,
    pub shadowmask_images: Vec<String>,
    pub lightmap_encodings: Vec<LightmapEncoding>,
    pub post: PostSettings,
    pub sky: Option<ReflectionCube>,
    pub probes: Vec<ReflectionCube>,
    pub probe_positions: Vec<Vec3>,
    pub probe_sh: Vec<[Vec4; 7]>,
    pub fog_enabled: bool,
    pub fog_color: Vec4,
    pub fog_mode: i64,
    pub fog_density: f32,
    pub fog_start: f32,
    pub fog_end: f32,
}

/// Unity's GetShaderConstantsFromNormalizedSH: 27 coeffs (channel-major) -> 7 float4.
pub fn sh_constants(c: &[f32]) -> [Vec4; 7] {
    let s = |ch: usize, i: usize| c[ch * 9 + i];
    let mut out = [Vec4::ZERO; 7];
    for ch in 0..3 {
        out[ch] = Vec4::new(s(ch, 3), s(ch, 1), s(ch, 2), s(ch, 0) - s(ch, 6));
        out[ch + 3] = Vec4::new(s(ch, 4), s(ch, 5), s(ch, 6) * 3.0, s(ch, 7));
    }
    out[6] = Vec4::new(s(0, 8), s(1, 8), s(2, 8), 1.0);
    out
}

fn post_value(o: &Value, a: &str, b: &str) -> Option<f32> {
    let v = &o[a][b];
    if v["override"].as_bool().unwrap_or(false) {
        v["value"].as_f64().map(|x| x as f32)
    } else {
        None
    }
}

pub fn parse_graphics(g: &Value, level_center: Option<Vec3>) -> Graphics {
    let rs = &g["render_settings"];
    let mut coeffs = vec![0.0f32; 27];
    if let Some(m) = rs["ambient_probe"].as_object() {
        for i in 0..27 {
            coeffs[i] = m.get(&format!("sh[{i:2}]")).map(f).unwrap_or(0.0);
        }
    }
    let lm = &g["lightmaps"];
    let strings = |v: &Value| -> Vec<String> {
        v.as_array()
            .map(|a| a.iter().map(|x| x.as_str().unwrap_or("").to_string()).collect())
            .unwrap_or_default()
    };
    let encodings = lm["encodings"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|e| LightmapEncoding {
                    kind: e["kind"].as_str().unwrap_or("dldr").to_string(),
                    exposure: f(&e["exposure"]),
                    exponent: e.get("exponent").map(f).unwrap_or(1.0),
                })
                .collect()
        })
        .unwrap_or_default();
    let ov = &g["global_volume"]["overrides"];
    let post = PostSettings {
        post_exposure_ev: post_value(ov, "color_adjustments", "postExposure").unwrap_or(0.0),
        contrast: post_value(ov, "color_adjustments", "contrast").unwrap_or(0.0),
        saturation: post_value(ov, "color_adjustments", "saturation").unwrap_or(0.0),
        tonemapping: ov["tonemapping"]["value"].as_i64().unwrap_or(0) as i32,
        bloom_threshold: post_value(ov, "bloom", "threshold").unwrap_or(0.9),
        bloom_intensity: post_value(ov, "bloom", "intensity").unwrap_or(0.0),
        bloom_scatter: post_value(ov, "bloom", "scatter").unwrap_or(0.7),
    };
    let cube = |v: &Value| -> Option<ReflectionCube> {
        Some(ReflectionCube {
            file: v["data_file"].as_str()?.to_string(),
            size: v["width"].as_u64()? as u32,
            mips: v["mip_count"].as_u64()? as u32,
            format: v["format"].as_i64()?,
            intensity: v.get("intensity").map(f).unwrap_or(1.0),
            center: None,
            box_size: v["box_size"].as_array().map(|a| Vec3::new(f(&a[0]), f(&a[1]), f(&a[2]))),
        })
    };
    let _ = level_center;
    let probe_positions = lm["probe_positions"]
        .as_array()
        .map(|a| a.iter().map(|p| Vec3::new(f(&p[0]), f(&p[1]), f(&p[2]))).collect())
        .unwrap_or_default();
    let probe_sh = lm["probe_sh"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(|p| {
                    let c: Vec<f32> = p.as_array().unwrap().iter().map(f).collect();
                    sh_constants(&c)
                })
                .collect()
        })
        .unwrap_or_default();
    Graphics {
        sh: sh_constants(&coeffs),
        ambient_intensity: rs.get("ambient_intensity").map(f).unwrap_or(1.0),
        lightmap_images: strings(&lm["color_images"]),
        shadowmask_images: strings(&lm["shadowmask_images"]),
        lightmap_encodings: encodings,
        post,
        sky: cube(&g["sky_reflection"]),
        probes: g["reflection_probes"].as_array().map(|a| a.iter().filter_map(cube).collect()).unwrap_or_default(),
        probe_positions,
        probe_sh,
        fog_enabled: rs["fog"].as_bool().unwrap_or(false),
        fog_color: color_linear(&rs["fog_color"]),
        fog_mode: rs["fog_mode"].as_i64().unwrap_or(3),
        fog_density: f(&rs["fog_density"]),
        fog_start: f(&rs["fog_start"]),
        fog_end: f(&rs["fog_end"]),
    }
}

// ---------------------------------------------------------------- scene.json

pub fn local_matrix(t: &Value) -> Mat4 {
    let tr = &t["translation"];
    let r = &t["rotation"];
    let s = &t["scale"];
    Mat4::from_scale_rotation_translation(
        Vec3::new(f(&s[0]), f(&s[1]), f(&s[2])),
        Quat::from_xyzw(f(&r[0]), f(&r[1]), f(&r[2]), f(&r[3])).normalize(),
        Vec3::new(f(&tr[0]), f(&tr[1]), f(&tr[2])),
    )
}

pub struct SceneGraph {
    pub nodes: Vec<Value>,
    pub world: Vec<Mat4>,
}

impl SceneGraph {
    pub fn new(scene: Value) -> Self {
        let nodes = scene["nodes"].as_array().cloned().unwrap_or_default();
        let mut world: Vec<Option<Mat4>> = vec![None; nodes.len()];
        fn solve(i: usize, nodes: &[Value], world: &mut Vec<Option<Mat4>>) -> Mat4 {
            if let Some(m) = world[i] {
                return m;
            }
            let l = local_matrix(&nodes[i]["transform"]);
            let m = match nodes[i]["parent"].as_u64() {
                Some(p) => solve(p as usize, nodes, world) * l,
                None => l,
            };
            world[i] = Some(m);
            m
        }
        for i in 0..nodes.len() {
            solve(i, &nodes, &mut world);
        }
        SceneGraph { world: world.into_iter().map(|m| m.unwrap()).collect(), nodes }
    }

    pub fn find(&self, path_suffix: &str) -> Option<usize> {
        self.nodes.iter().position(|n| n["path"].as_str().map(|p| p.ends_with(path_suffix)).unwrap_or(false))
    }

    pub fn component<'a>(&'a self, i: usize, ty: &str) -> Option<&'a Value> {
        self.nodes[i]["components"]
            .as_array()?
            .iter()
            .find(|c| c["type"] == ty || c["script"] == ty)
            .map(|c| &c["data"])
    }
}

// ---------------------------------------------------------------- lightmaps

/// Decode a Unity-encoded lightmap PNG into linear RGB (f32), top row first.
pub fn decode_lightmap(path: &Path, enc: &LightmapEncoding) -> Result<(u32, u32, Vec<[f32; 4]>)> {
    let img = image::open(path).with_context(|| format!("opening {}", path.display()))?.to_rgba8();
    let (w, h) = img.dimensions();
    let lut: Vec<f32> = (0..256).map(|i| srgb_to_linear(i as f32 / 255.0)).collect();
    let mut out = Vec::with_capacity((w * h) as usize);
    for p in img.pixels() {
        let [r, g, b, a] = p.0;
        let (r, g, b) = (lut[r as usize], lut[g as usize], lut[b as usize]);
        let px = match enc.kind.as_str() {
            // Unity DecodeLightmapRGBM: (x * a^y) * rgb, rgb sampled as sRGB in linear projects
            "rgbm" => {
                let m = enc.exposure * (a as f32 / 255.0).powf(enc.exponent);
                [r * m, g * m, b * m, 1.0]
            }
            // DecodeLightmapDoubleLDR: 2^2.2 * rgb (sRGB-sampled)
            _ => [r * enc.exposure, g * enc.exposure, b * enc.exposure, 1.0],
        };
        out.push(px);
    }
    Ok((w, h, out))
}

// ---------------------------------------------------------------- BC6H cubemaps

/// Read a face-major BC6H cubemap blob into per-(face, mip) byte slices, reordered to
/// wgpu's layer-major-then-mip layout (which is also face-major), so it is a no-op copy.
pub fn load_cube_blob(path: &Path) -> Result<Vec<u8>> {
    Ok(std::fs::read(path)?)
}

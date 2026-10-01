//! Baked light-probe volume (Unity `LightProbes`): positions + per-probe SphericalHarmonicsL2.
//! Dynamic objects (beasts) sample the nearest probe so they pick up the baked GI the static
//! lightmaps give the scenery. Full tetrahedral interpolation is not implemented; the nearest
//! probe is a good approximation for the small lobby volume.
use bevy::prelude::*;
use serde_json::Value;

#[derive(Resource, Default)]
pub struct LightProbes {
    positions: Vec<Vec3>,
    /// 27 floats per probe (planar per channel: [0..9) R, [9..18) G, [18..27) B).
    sh: Vec<[f32; 27]>,
}

impl LightProbes {
    pub fn load(graphics: &Value) -> Self {
        let lightmaps = &graphics["lightmaps"];
        let positions = lightmaps["probe_positions"]
            .as_array()
            .map(|list| {
                list.iter()
                    .filter_map(|p| {
                        let a = p.as_array()?;
                        if a.len() != 3 {
                            return None;
                        }
                        Some(Vec3::new(
                            a[0].as_f64()? as f32,
                            a[1].as_f64()? as f32,
                            a[2].as_f64()? as f32,
                        ))
                    })
                    .collect()
            })
            .unwrap_or_default();
        let sh = lightmaps["probe_sh"]
            .as_array()
            .map(|list| {
                list.iter()
                    .filter_map(|entry| {
                        let a = entry.as_array()?;
                        if a.len() < 27 {
                            return None;
                        }
                        let mut out = [0.0f32; 27];
                        for (i, v) in a.iter().take(27).enumerate() {
                            out[i] = v.as_f64().unwrap_or(0.0) as f32;
                        }
                        Some(out)
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self { positions, sh }
    }

    pub fn is_empty(&self) -> bool {
        self.positions.is_empty() || self.sh.is_empty()
    }

    /// Nearest probe's SH coefficients to a world position (Unity space).
    pub fn nearest(&self, position: Vec3) -> Option<&[f32; 27]> {
        let index = self
            .positions
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                (**a - position)
                    .length_squared()
                    .total_cmp(&(**b - position).length_squared())
            })
            .map(|(i, _)| i)?;
        self.sh.get(index)
    }

    /// Unity `SHEvalLinearL0L1` for a direction, returning linear RGB irradiance.
    pub fn eval(sh: &[f32; 27], normal: Vec3) -> Vec3 {
        let (x, y, z) = (normal.x, normal.y, normal.z);
        let channel = |base: usize| {
            let c = |i: usize| sh[base + i];
            let l0l1 = c(3) * x + c(1) * y + c(2) * z + c(0);
            let l2 = c(4) * (x * y) + c(5) * (y * z) + c(6) * (z * z) + c(7) * (z * x)
                + c(8) * (x * x - y * y);
            (l0l1 + l2).max(0.0)
        };
        Vec3::new(channel(0), channel(9), channel(18))
    }
}

/// Applies the baked probe GI to the beast's materials as an added emissive term. Bevy's ambient
/// is global, so this is how a beast picks up the room's baked light like the real game. The
/// beast's materials are the exported `Primary_Color` / `White`; scenery keeps its own path.
///
/// DEFAULT OFF (`GB_PROBE_EMISSIVE=1` re-enables). The additive form double-counts light: the
/// beast already receives Bevy's global ambient (the same term every scenery material gets), and
/// stacking the probe irradiance (~0.39 linear, near-neutral) on top washed dark materials pale —
/// the beast's dark-red body rendered (216,199,195) against the reference's (92,73,66). It also
/// kept re-applying after every material clone (tint/costume systems clone materials, and the
/// entity keeps its `GltfMaterialName`, so each new asset id got the emissive again), which undid
/// the explicit `emissive = BLACK` clears.
pub fn apply_to_beast(
    probes: Res<LightProbes>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    named: Query<(&bevy::gltf::GltfMaterialName, &MeshMaterial3d<StandardMaterial>)>,
    mut applied: Local<std::collections::HashSet<AssetId<StandardMaterial>>>,
) {
    if probes.is_empty() || std::env::var_os("GB_PROBE_EMISSIVE").is_none() {
        return;
    }
    let scale: f32 = std::env::var("GB_PROBE_SCALE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1.0);
    // Sample the probe volume at the lobby's actor spawn (the beast stands there).
    let sample = probes.positions.first().copied().unwrap_or(Vec3::ZERO);
    let Some(sh) = probes.nearest(sample) else { return };
    let irradiance = LightProbes::eval(sh, Vec3::Y) * scale;
    if std::env::var_os("GB_PROBE_DEBUG").is_some() {
        info!(
            "probe at {:?}: L0 {:?} eval+Y {:?} scale {scale} -> {:?}",
            sample,
            [sh[0], sh[9], sh[18]],
            LightProbes::eval(sh, Vec3::Y),
            irradiance
        );
    }
    for (name, handle) in &named {
        if !matches!(name.0.as_str(), "Primary_Color" | "White") {
            continue;
        }
        let id = handle.0.id();
        if !applied.insert(id) {
            continue;
        }
        if let Some(material) = materials.get_mut(id) {
            material.emissive = LinearRgba::rgb(irradiance.x, irradiance.y, irradiance.z);
            material.emissive_exposure_weight = 0.0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearest_probe_and_sh_eval_are_finite() {
        let probes = LightProbes {
            positions: vec![Vec3::ZERO, Vec3::new(10.0, 0.0, 0.0)],
            sh: vec![[0.5; 27], [0.1; 27]],
        };
        let sh = probes.nearest(Vec3::new(9.0, 0.0, 0.0)).unwrap();
        assert!((sh[0] - 0.1).abs() < 1e-6);
        let up = LightProbes::eval(sh, Vec3::Y);
        assert!(up.is_finite() && up.x >= 0.0);
    }
}

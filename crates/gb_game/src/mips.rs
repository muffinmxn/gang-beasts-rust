//! Unity samples its textures with a full mip chain (trilinear); the exported PNGs arrive in Bevy
//! with a single level, so distant detail aliases into speckle and shimmer. Build the chain on the
//! CPU when a texture loads (box filter in linear space for sRGB images).
use bevy::image::{ImageAddressMode, ImageFilterMode, ImageSampler, ImageSamplerDescriptor};
use bevy::prelude::*;
use bevy::render::render_resource::TextureFormat;

fn srgb_to_linear(c: u8) -> f32 {
    let c = c as f32 / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

fn linear_to_srgb(c: f32) -> u8 {
    let c = if c <= 0.003_130_8 {
        c * 12.92
    } else {
        1.055 * c.powf(1.0 / 2.4) - 0.055
    };
    (c.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
}

/// RGBA8 mip chain (level 0 included). `srgb`: filter colour channels in linear space.
pub fn mip_chain(data: &[u8], width: u32, height: u32, srgb: bool) -> (Vec<u8>, u32) {
    let mut out = data.to_vec();
    let (mut w, mut h) = (width, height);
    let mut level: Vec<u8> = data.to_vec();
    let mut count = 1;
    while w > 1 || h > 1 {
        let (nw, nh) = ((w / 2).max(1), (h / 2).max(1));
        let mut next = vec![0u8; (nw * nh * 4) as usize];
        for y in 0..nh {
            for x in 0..nw {
                let mut acc = [0f32; 4];
                for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
                    let sx = (x * 2 + dx).min(w - 1);
                    let sy = (y * 2 + dy).min(h - 1);
                    let i = ((sy * w + sx) * 4) as usize;
                    for c in 0..4 {
                        acc[c] += if srgb && c < 3 {
                            srgb_to_linear(level[i + c])
                        } else {
                            level[i + c] as f32 / 255.0
                        };
                    }
                }
                let o = ((y * nw + x) * 4) as usize;
                for c in 0..4 {
                    let v = acc[c] / 4.0;
                    next[o + c] = if srgb && c < 3 {
                        linear_to_srgb(v)
                    } else {
                        (v.clamp(0.0, 1.0) * 255.0 + 0.5) as u8
                    };
                }
            }
        }
        out.extend_from_slice(&next);
        level = next;
        w = nw;
        h = nh;
        count += 1;
    }
    (out, count)
}

/// On every newly loaded RGBA8 texture without mips: add the chain and a trilinear sampler.
pub fn generate(mut events: EventReader<AssetEvent<Image>>, mut images: ResMut<Assets<Image>>) {
    let ids: Vec<AssetId<Image>> = events
        .read()
        .filter_map(|e| match e {
            AssetEvent::LoadedWithDependencies { id } | AssetEvent::Added { id } => Some(*id),
            _ => None,
        })
        .collect();
    for id in ids {
        let Some(image) = images.get_mut(id) else {
            continue;
        };
        let desc = &image.texture_descriptor;
        let srgb = match desc.format {
            TextureFormat::Rgba8UnormSrgb => true,
            TextureFormat::Rgba8Unorm => false,
            _ => continue,
        };
        if desc.mip_level_count > 1 || desc.size.depth_or_array_layers != 1 {
            continue;
        }
        let (w, h) = (desc.size.width, desc.size.height);
        let Some(data) = image.data.as_ref() else {
            continue;
        };
        if w < 2 || h < 2 || data.len() != (w * h * 4) as usize {
            continue;
        }
        let (chain, levels) = mip_chain(data, w, h, srgb);
        image.data = Some(chain);
        image.texture_descriptor.mip_level_count = levels;
        // Keep the glTF wrap modes; add trilinear + anisotropic filtering.
        let mut s = match &image.sampler {
            ImageSampler::Descriptor(d) => d.clone(),
            ImageSampler::Default => ImageSamplerDescriptor {
                address_mode_u: ImageAddressMode::Repeat,
                address_mode_v: ImageAddressMode::Repeat,
                ..default()
            },
        };
        s.mag_filter = ImageFilterMode::Linear;
        s.min_filter = ImageFilterMode::Linear;
        s.mipmap_filter = ImageFilterMode::Linear;
        s.anisotropy_clamp = 8;
        image.sampler = ImageSampler::Descriptor(s);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn chain_has_all_levels() {
        let data = vec![255u8; 8 * 4 * 4];
        let (chain, levels) = super::mip_chain(&data, 8, 4, true);
        assert_eq!(levels, 4); // 8x4, 4x2, 2x1, 1x1
        assert_eq!(chain.len(), (32 + 8 + 2 + 1) * 4);
        assert!(chain.iter().all(|&v| v == 255));
    }
}

/// Exported stage meshes arrive without tangents (the source report counts 561 for Rooftop),
/// but normal mapping needs a TBN basis. Generate the standard MikkTSpace-equivalent tangents
/// on the CPU for any mesh that has positions + normals + UV0 but no tangent attribute.
pub fn generate_tangents(
    mut events: EventReader<AssetEvent<Mesh>>,
    mut meshes: ResMut<Assets<Mesh>>,
) {
    let ids: Vec<_> = events
        .read()
        .filter_map(|event| match event {
            AssetEvent::Added { id } => Some(*id),
            _ => None,
        })
        .collect();
    for id in ids {
        let Some(mesh) = meshes.get_mut(id) else {
            continue;
        };
        if mesh.attribute(Mesh::ATTRIBUTE_TANGENT).is_some()
            || mesh.attribute(Mesh::ATTRIBUTE_POSITION).is_none()
            || mesh.attribute(Mesh::ATTRIBUTE_NORMAL).is_none()
            || mesh.attribute(Mesh::ATTRIBUTE_UV_0).is_none()
        {
            continue;
        }
        if let Err(error) = mesh.generate_tangents() {
            warn!("could not generate tangents: {error}");
        }
    }
}

/// The game's GangBeasts/Surface/VinylOrMetal shader never looks mirror-like (reference
/// screenshots show matte buildings and floors), but several of its materials carry smoothness
/// 0.9-1.0 which Bevy's PBR turns into a blinding specular sun reflection. Until the shader is
/// ported, clamp opaque materials to a vinyl-like minimum roughness (glass stays glossy).
pub fn vinyl_roughness(
    mut events: EventReader<AssetEvent<StandardMaterial>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    const MIN_ROUGHNESS: f32 = 0.6;
    const MAX_METALLIC: f32 = 0.2;
    let ids: Vec<_> = events
        .read()
        .filter_map(|e| match e {
            AssetEvent::Added { id } => Some(*id),
            _ => None,
        })
        .collect();
    for id in ids {
        let Some(m) = materials.get_mut(id) else {
            continue;
        };
        if m.unlit || !matches!(m.alpha_mode, AlphaMode::Opaque | AlphaMode::Mask(_)) {
            continue;
        }
        if m.perceptual_roughness < MIN_ROUGHNESS {
            m.perceptual_roughness = MIN_ROUGHNESS;
        }
        // Full metals render black without sky reflections (no environment map); VinylOrMetal's
        // metals read as grey painted steel in the references.
        m.metallic = m.metallic.min(MAX_METALLIC);
    }
}

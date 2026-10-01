// Source graph + URP direct BRDF. Bevy still supplies local-light attenuation,
// shadow maps and indirect lighting; no claim of complete URP lighting parity.
#import bevy_pbr::{
    forward_io::{VertexOutput, FragmentOutput},
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions,
    pbr_bindings,
    lighting,
    mesh_view_bindings as view_bindings,
    mesh_view_types,
    mesh_types::MESH_FLAGS_SHADOW_RECEIVER_BIT,
    shadows,
}

struct VinylSettings {
    normal_uv: vec4<f32>,
    normal_strength: f32,
    /// Flip the normal map's Y channel (Unity's V axis points up; without the flip the brick
    /// highlights sit on the bottom edge). -1.0 = flipped.
    normal_y: f32,
    use_vertex_color: u32,
    urp_direct: u32,
    triplanar: vec4<f32>,
    sh_ambient: u32,
    sh: array<vec4<f32>, 9>,
    flat_ambient: vec4<f32>,
    sh_scale: f32,
    _pad: vec3<f32>,
}
@group(2) @binding(100) var<uniform> settings: VinylSettings;
@group(2) @binding(101) var triplanar_normal_texture: texture_2d<f32>;
@group(2) @binding(102) var triplanar_normal_sampler: sampler;

fn unpack_normal(rgb: vec3<f32>, strength: f32) -> vec3<f32> {
    let xy = vec2((rgb.x * 2.0 - 1.0) * strength, (rgb.y * 2.0 - 1.0) * strength * settings.normal_y);
    return vec3(xy, sqrt(max(1.0 - dot(xy, xy), 0.0)));
}

// Shader Graph Triplanar: project the source noise normal on all three world axes and blend
// by the geometric normal. This preserves detail on roofs, walls and bevels without UV seams.
fn triplanar_world_normal(position: vec3<f32>, geometric_normal: vec3<f32>) -> vec3<f32> {
    let n = normalize(geometric_normal);
    let scale = max(settings.triplanar.y, 0.0001);
    let blend = max(settings.triplanar.z, 0.0001);
    var w = pow(abs(n), vec3(blend));
    w /= max(w.x + w.y + w.z, 0.0001);
    let tx = unpack_normal(textureSample(triplanar_normal_texture,
        triplanar_normal_sampler, position.zy * scale).xyz, settings.triplanar.w);
    let ty = unpack_normal(textureSample(triplanar_normal_texture,
        triplanar_normal_sampler, position.xz * scale).xyz, settings.triplanar.w);
    let tz = unpack_normal(textureSample(triplanar_normal_texture,
        triplanar_normal_sampler, position.xy * scale).xyz, settings.triplanar.w);
    let sx = select(-1.0, 1.0, n.x >= 0.0);
    let sy = select(-1.0, 1.0, n.y >= 0.0);
    let sz = select(-1.0, 1.0, n.z >= 0.0);
    let nx = vec3(tx.z * sx, tx.y, tx.x);
    let ny = vec3(ty.x, ty.z * sy, ty.y);
    let nz = vec3(tz.x, tz.y, tz.z * sz);
    return normalize(nx * w.x + ny * w.y + nz * w.z);
}

// Unity SphericalHarmonicsL2 evaluation (UnityCG.cginc SHEvalLinearL0L1). The probe is stored
// planar per channel; the shader packs (R,G,B) per coefficient. Returns linear irradiance.
fn unity_sh(n: vec3<f32>) -> vec3<f32> {
    let x = n.x; let y = n.y; let z = n.z;
    let c0 = settings.sh[0].xyz;
    let c1 = settings.sh[1].xyz;
    let c2 = settings.sh[2].xyz;
    let c3 = settings.sh[3].xyz;
    let c4 = settings.sh[4].xyz;
    let c5 = settings.sh[5].xyz;
    let c6 = settings.sh[6].xyz;
    let c7 = settings.sh[7].xyz;
    let c8 = settings.sh[8].xyz;
    // L0 + L1: dot((c3, c1, c2, c0), (x, y, z, 1)).
    let l0l1 = c3 * x + c1 * y + c2 * z + c0;
    // L2: dot((c4, c5, c6, c7), (xy, yz, zz, zx)) + c8 * (xx - yy).
    let l2 = c4 * (x * y) + c5 * (y * z) + c6 * (z * z) + c7 * (z * x)
        + c8 * (x * x - y * y);
    return max(l0l1 + l2, vec3(0.0));
}

// VinylOrMetal 0018_ps.asm: roughness floor, normalization, LoH floor and d term
// match URP DirectBRDFSpecular. 1/pi bridges Bevy irradiance to its Lambert units.
fn urp_brdf(albedo: vec3<f32>, metallic: f32, perceptual_roughness: f32,
    N: vec3<f32>, V: vec3<f32>, L: vec3<f32>) -> vec3<f32> {
    let H = normalize(L + V);
    let NoH = saturate(dot(N, H));
    let LoH = saturate(dot(L, H));
    let r = max(perceptual_roughness * perceptual_roughness, 0.0078125);
    let r2 = r * r;
    let d = NoH * NoH * (r2 - 1.0) + 1.00001;
    let specular_term = r2 / (d * d * max(0.1, LoH * LoH) * (r * 4.0 + 2.0));
    let diffuse = albedo * (0.96 * (1.0 - metallic));
    let specular = mix(vec3(0.04), albedo, metallic);
    return (diffuse + specular * specular_term) * saturate(dot(N, L)) / 3.14159265359;
}

// Unity Core's 5x5 tent filter, translated directly to WGSL. Nine hardware-PCF fetches
// reproduce URP's soft-shadow footprint instead of Bevy's default single 2x2 fetch.
fn tent3_areas(offset: f32) -> array<vec4<f32>, 2> {
    let edge = (offset + 0.5) * (offset + 0.5) * 0.5;
    var area = vec4(edge - offset, 1.0 - offset, 1.0 + offset, edge);
    let uncut = area;
    area.y -= min(offset, 0.0) * min(offset, 0.0);
    area.z -= max(offset, 0.0) * max(offset, 0.0);
    return array(area, uncut);
}

fn tent5_weights(offset: f32) -> array<vec3<f32>, 2> {
    let a = tent3_areas(offset);
    return array(
        0.16 * vec3(a[0].x, a[1].y, a[0].y + 1.0),
        0.16 * vec3(a[0].z + 1.0, a[1].z, a[0].w),
    );
}

fn urp_tent_cascade(light_id: u32, cascade_index: u32, position: vec4<f32>, normal: vec3<f32>) -> f32 {
    let light = view_bindings::lights.directional_lights[light_id];
    let cascade = light.cascades[cascade_index];
    let offset_position = vec4(position.xyz
        + light.shadow_normal_bias * cascade.texel_size * normal
        + light.shadow_depth_bias * light.direction_to_light.xyz, position.w);
    let clip = cascade.clip_from_world * offset_position;
    if clip.w <= 0.0 { return 1.0; }
    let ndc = clip.xyz / clip.w;
    if any(ndc.xy < vec2(-1.0)) || ndc.z < 0.0 || any(ndc > vec3(1.0)) { return 1.0; }
    let uv = ndc.xy * vec2(0.5, -0.5) + vec2(0.5);
    let size = vec2<f32>(textureDimensions(view_bindings::directional_shadow_textures));
    let center = floor(uv * size + 0.5);
    let subtexel = uv * size - center;
    let wu = tent5_weights(subtexel.x);
    let wv = tent5_weights(subtexel.y);
    let group_u = vec3(wu[0].x + wu[0].y, wu[0].z + wu[1].x, wu[1].y + wu[1].z);
    let group_v = vec3(wv[0].x + wv[0].y, wv[0].z + wv[1].x, wv[1].y + wv[1].z);
    let offset_u = (vec3(wu[0].y, wu[1].x, wu[1].z) / group_u + vec3(-2.5, -0.5, 1.5)) / size.x;
    let offset_v = (vec3(wv[0].y, wv[1].x, wv[1].z) / group_v + vec3(-2.5, -0.5, 1.5)) / size.y;
    let origin = center / size;
    let us = array(offset_u.x, offset_u.y, offset_u.z);
    let vs = array(offset_v.x, offset_v.y, offset_v.z);
    var result = 0.0;
    for (var y = 0u; y < 3u; y += 1u) {
        for (var x = 0u; x < 3u; x += 1u) {
#ifdef NO_ARRAY_TEXTURES_SUPPORT
            let sample = textureSampleCompare(view_bindings::directional_shadow_textures,
                view_bindings::directional_shadow_textures_comparison_sampler,
                origin + vec2(us[x], vs[y]), ndc.z);
#else
            let sample = textureSampleCompareLevel(view_bindings::directional_shadow_textures,
                view_bindings::directional_shadow_textures_comparison_sampler,
                origin + vec2(us[x], vs[y]), i32(light.depth_texture_base_index + cascade_index), ndc.z);
#endif
            result += sample * group_u[x] * group_v[y];
        }
    }
    return result;
}

fn urp_directional_shadow(light_id: u32, position: vec4<f32>, normal: vec3<f32>, view_z: f32) -> f32 {
    let light = view_bindings::lights.directional_lights[light_id];
    var index = light.num_cascades;
    for (var i = 0u; i < light.num_cascades; i += 1u) {
        if -view_z < light.cascades[i].far_bound { index = i; break; }
    }
    if index >= light.num_cascades { return 1.0; }
    var shadow = urp_tent_cascade(light_id, index, position, normal);
    let next = index + 1u;
    if next < light.num_cascades {
        let far = light.cascades[index].far_bound;
        let near = (1.0 - light.cascades_overlap_proportion) * far;
        if -view_z >= near {
            let next_shadow = urp_tent_cascade(light_id, next, position, normal);
            shadow = mix(shadow, next_shadow, (-view_z - near) / (far - near));
        }
    }
    return shadow;
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var vertex = in;
#ifdef VERTEX_COLORS
    // Shader Graph's Boolean property gates vertex colors; StandardMaterial always multiplies.
    if settings.use_vertex_color == 0u { vertex.color = vec4(1.0); }
#endif
    var pbr = pbr_input_from_standard_material(vertex, is_front);
    pbr.material.base_color = pbr_functions::alpha_discard(pbr.material, pbr.material.base_color);
    if settings.triplanar.x > 0.5 {
        pbr.N = triplanar_world_normal(pbr.world_position.xyz, pbr.world_normal);
    }
#ifdef VERTEX_UVS
#ifdef VERTEX_TANGENTS
#ifdef STANDARD_MATERIAL_NORMAL_MAP
    if settings.triplanar.x < 0.5 {
        // Normal UVs are independent of BaseMap UVs in both source graph variants.
        let uv = in.uv * settings.normal_uv.xy + settings.normal_uv.zw;
        let sample = textureSampleBias(pbr_bindings::normal_map_texture,
            pbr_bindings::normal_map_sampler, uv, view_bindings::view.mip_bias).xyz;
        let Nt = unpack_normal(sample, settings.normal_strength);
        let tbn = pbr_functions::calculate_tbn_mikktspace(pbr.world_normal, in.world_tangent);
        pbr.N = normalize(tbn * Nt);
    }
#endif
#endif
#endif
    var out: FragmentOutput;
    out.color = pbr_functions::apply_pbr_lighting(pbr);
// URP: lightmapped surfaces take their GI from the lightmap only (no SH ambient on top).
#ifndef LIGHTMAP
#ifdef SH_AMBIENT
    if settings.sh_ambient != 0u {
        // Add only the source probe's directional variation (SH minus its L0 average), so the
        // overall ambient level is unchanged but up-facing surfaces (the duct) gain the sky
        // light the flat ambient misses. Bevy's flat ambient stays as the average term.
        let albedo = pbr.material.base_color.rgb;
        let delta = unity_sh(pbr.N) - settings.sh[0].xyz;
        out.color = vec4(max(vec3(0.0), out.color.rgb + delta * albedo * settings.sh_scale), out.color.a);
    }
#endif
#endif
    if settings.urp_direct != 0u {
        // Replace the directional BRDF only; retain existing environment/probe/local-light
        // support while those Unity lighting inputs are ported in the following plan steps.
        var old: lighting::LightingInput;
        old.layers[0].NdotV = max(dot(pbr.N, pbr.V), 0.0001);
        old.layers[0].N = pbr.N;
        old.layers[0].R = reflect(-pbr.V, pbr.N);
        old.layers[0].perceptual_roughness = pbr.material.perceptual_roughness;
        old.layers[0].roughness = lighting::perceptualRoughnessToRoughness(pbr.material.perceptual_roughness);
        old.P = pbr.world_position.xyz;
        old.V = pbr.V;
        old.diffuse_color = pbr.material.base_color.rgb * (1.0 - pbr.material.metallic);
        old.F0_ = pbr_functions::calculate_F0(pbr.material.base_color.rgb,
            pbr.material.metallic, pbr.material.reflectance);
        old.F_ab = lighting::F_AB(pbr.material.perceptual_roughness, old.layers[0].NdotV);
        let view_z = (view_bindings::view.view_from_world * pbr.world_position).z;
        var correction = vec3(0.0);
        for (var i = 0u; i < view_bindings::lights.n_directional_lights; i += 1u) {
            let light = view_bindings::lights.directional_lights[i];
            var old_shadow = 1.0;
            var source_shadow = 1.0;
            if (pbr.flags & MESH_FLAGS_SHADOW_RECEIVER_BIT) != 0u
                && (light.flags & mesh_view_types::DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) != 0u {
                old_shadow = shadows::fetch_directional_shadow(i, pbr.world_position, pbr.world_normal, view_z);
                source_shadow = urp_directional_shadow(i, pbr.world_position, pbr.world_normal, view_z);
            }
            let before = lighting::directional_light(i, &old, true);
            let after = urp_brdf(pbr.material.base_color.rgb, pbr.material.metallic,
                pbr.material.perceptual_roughness, pbr.N, pbr.V, light.direction_to_light) * light.color.rgb;
            correction += after * source_shadow - before * old_shadow;
        }
        out.color = vec4(max(vec3(0.0), out.color.rgb + correction * view_bindings::view.exposure), out.color.a);
    }
    out.color = pbr_functions::main_pass_post_lighting_processing(pbr, out.color);
    return out;
}

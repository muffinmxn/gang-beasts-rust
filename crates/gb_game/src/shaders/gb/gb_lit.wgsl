// Gang Beasts surface shader (GangBeasts/Surface/VinylOrMetal[AlphaClipped], URP/Lit,
// Shader Graphs/Triplanar) translated from URP 12 "UniversalFragmentPBR" to WGSL.
//
// Lighting model = URP: BRDFData, GlobalIllumination (lightmap or SH + reflection probe),
// LightingPhysicallyBased for every realtime light, URP distance/spot attenuation,
// shadow strength + shadowmask mixing, then GangBeasts OpaqueSurfaceFog and URP's
// color pipeline (post exposure, contrast, saturation, ACES) applied per pixel.

#import bevy_pbr::{
    forward_io::VertexOutput,
    mesh_view_bindings as view_bindings,
    mesh_view_types,
    clustered_forward as clustering,
    shadows,
}
#import gb::urp_post::urp_color_pipeline
#import gb::types::GbParams
#ifdef SCREEN_SPACE_AMBIENT_OCCLUSION
#import bevy_pbr::mesh_view_bindings::screen_space_ambient_occlusion_texture
#import bevy_pbr::ssao_utils::ssao_multibounce
#endif



@group(2) @binding(0) var<uniform> m: GbParams;
@group(2) @binding(1) var base_tex: texture_2d<f32>;
@group(2) @binding(2) var base_smp: sampler;
@group(2) @binding(3) var normal_tex: texture_2d<f32>;
@group(2) @binding(4) var normal_smp: sampler;
@group(2) @binding(5) var lightmap_tex: texture_2d<f32>;
@group(2) @binding(6) var lightmap_smp: sampler;
@group(2) @binding(7) var shadowmask_tex: texture_2d<f32>;
@group(2) @binding(8) var shadowmask_smp: sampler;
@group(2) @binding(9) var refl_tex: texture_cube<f32>;
@group(2) @binding(10) var refl_smp: sampler;
@group(2) @binding(11) var emissive_tex: texture_2d<f32>;
@group(2) @binding(12) var emissive_smp: sampler;

const F_LIGHTMAP: u32 = 1u;
const F_SHADOWMASK: u32 = 2u;
const F_NORMALMAP: u32 = 4u;
const F_ALPHACLIP: u32 = 8u;
const F_UNLIT: u32 = 16u;
const F_SKY: u32 = 32u;
const F_RECEIVE_SHADOWS: u32 = 64u;
const F_TRIPLANAR: u32 = 128u;
const F_VERTEX_COLOR: u32 = 256u;
const F_REFLECTION: u32 = 512u;
const F_BASE_TEX: u32 = 1024u;
const F_EMISSIVE_TEX: u32 = 2048u;

const HALF_MIN: f32 = 6.103515625e-5;
const HALF_MIN_SQRT: f32 = 0.0078125;

// glTF space -> Unity space (exporter negated X)
fn to_unity(v: vec3<f32>) -> vec3<f32> { return vec3(-v.x, v.y, v.z); }

// URP SampleSH9 (L0L1 + L2), normal in Unity world space
fn sample_sh(n: vec3<f32>) -> vec3<f32> {
    let n4 = vec4<f32>(n, 1.0);
    var res = vec3(dot(m.sh[0], n4), dot(m.sh[1], n4), dot(m.sh[2], n4));
    let vb = n.xyzz * n.yzzx;
    res = res + vec3(dot(m.sh[3], vb), dot(m.sh[4], vb), dot(m.sh[5], vb));
    let vc = n.x * n.x - n.y * n.y;
    res = res + m.sh[6].rgb * vc;
    return max(res, vec3(0.0));
}

struct Brdf {
    diffuse: vec3<f32>,
    specular: vec3<f32>,
    perceptual_roughness: f32,
    roughness: f32,
    roughness2: f32,
    grazing: f32,
    normalization: f32,
    roughness2_minus_one: f32,
};

fn init_brdf(albedo: vec3<f32>, metallic: f32, smoothness: f32) -> Brdf {
    var b: Brdf;
    let one_minus_reflectivity = 0.96 - metallic * 0.96;
    let reflectivity = 1.0 - one_minus_reflectivity;
    b.diffuse = albedo * one_minus_reflectivity;
    b.specular = mix(vec3(0.04), albedo, metallic);
    b.perceptual_roughness = 1.0 - smoothness;
    b.roughness = max(b.perceptual_roughness * b.perceptual_roughness, HALF_MIN_SQRT);
    b.roughness2 = max(b.roughness * b.roughness, HALF_MIN);
    b.grazing = saturate(smoothness + reflectivity);
    b.normalization = b.roughness * 4.0 + 2.0;
    b.roughness2_minus_one = b.roughness2 - 1.0;
    return b;
}

fn direct_brdf_specular(b: Brdf, n: vec3<f32>, l: vec3<f32>, v: vec3<f32>) -> f32 {
    let h = normalize(l + v);
    let nh = saturate(dot(n, h));
    let lh = saturate(dot(l, h));
    let d = nh * nh * b.roughness2_minus_one + 1.00001;
    let lh2 = lh * lh;
    let spec = b.roughness2 / ((d * d) * max(0.1, lh2) * b.normalization);
    return clamp(spec, 0.0, 100.0);
}

fn lighting_pbr(b: Brdf, radiance_color: vec3<f32>, atten: f32, n: vec3<f32>, l: vec3<f32>, v: vec3<f32>) -> vec3<f32> {
    let ndl = saturate(dot(n, l));
    let radiance = radiance_color * (atten * ndl);
    var brdf = b.diffuse;
    brdf = brdf + b.specular * direct_brdf_specular(b, n, l, v);
    return brdf * radiance;
}

fn glossy_env(refl_u: vec3<f32>, perceptual_roughness: f32, occlusion: f32) -> vec3<f32> {
    if ((m.flags & F_REFLECTION) == 0u) {
        // Unity falls back to the skybox-derived default reflection; approximate with ambient SH
        // no baked cubemap exported: Unity's default reflection is the skybox; approximate with the ambient SH
        return sample_sh(refl_u) * m.extra.y * occlusion;
    }
    let pr = perceptual_roughness * (1.7 - 0.7 * perceptual_roughness);
    let mip = pr * 6.0;
    let c = textureSampleLevel(refl_tex, refl_smp, refl_u, mip).rgb;
    return c * m.extra.y * occlusion;
}

fn opaque_surface_fog(color: vec3<f32>, world_pos: vec3<f32>, cam: vec3<f32>) -> vec3<f32> {
    let mode = u32(m.fog_color.a + 0.5);
    let dist = distance(world_pos, cam);
    if (mode == 1u) {
        // GangBeasts OpaqueSurfaceFog: sky fog grows with distance (reaching full at skyFogMaxDepth),
        // ground fog fills everything below groundFogElevation within groundFogMaxDepth.
        let sky_depth = m.fog_params.x;
        let ground_depth = m.fog_params.z;
        let ground_elev = m.fog_params.w;
        let sky_f = pow(saturate(dist / max(sky_depth, 1e-3)), m.fog_extra.w);
        let below = saturate((ground_elev - world_pos.y) / max(m.fog_extra.x, 1e-3));
        let ground_f = saturate(dist / max(ground_depth, 1e-3)) * below;
        let f = saturate(max(ground_f, sky_f) * m.fog_extra.y);
        return mix(color, m.fog_color.rgb, f);
    } else if (mode == 2u) {
        let f = saturate((m.fog_extra.w - dist) / max(m.fog_extra.w - m.fog_extra.z, 1e-3));
        return mix(m.fog_color.rgb, color, f);
    } else if (mode == 3u) {
        let f = exp2(-m.fog_extra.y * dist * 1.442695);
        return mix(m.fog_color.rgb, color, saturate(f));
    } else if (mode == 4u) {
        let x = m.fog_extra.y * dist;
        let f = exp2(-x * x * 1.442695);
        return mix(m.fog_color.rgb, color, saturate(f));
    }
    return color;
}

fn finish(color: vec3<f32>, world_pos: vec3<f32>, alpha: f32) -> vec4<f32> {
    let cam = view_bindings::view.world_position;
    let fogged = opaque_surface_fog(color, world_pos, cam);
    return vec4(urp_color_pipeline(fogged * m.wb.rgb, m.post), alpha);
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> @location(0) vec4<f32> {
    let world_pos = in.world_position.xyz;
    let cam = view_bindings::view.world_position;
    let V = normalize(cam - world_pos);

    // ---------------------------------------------------------------- sky
    if ((m.flags & F_SKY) != 0u) {
        let dir_u = to_unity(-V);
        let sky = textureSampleLevel(refl_tex, refl_smp, dir_u, 0.0).rgb * m.extra.y;
        var c = sky;
        let mode = u32(m.fog_color.a + 0.5);
        if (mode == 1u) {
            // sky gets full sky-fog at the horizon, fading with elevation angle
            let up = saturate(-V.y);
            let f = saturate(1.0 - up * m.fog_extra.z);
            c = mix(c, m.fog_color.rgb, f * m.fog_extra.y);
        }
        return vec4(urp_color_pipeline(c * m.wb.rgb, m.post), 1.0);
    }

    // ---------------------------------------------------------------- surface inputs
#ifdef VERTEX_UVS_A
    let uv = in.uv * m.base_st.xy + m.base_st.zw;
#else
    let uv = vec2<f32>(0.0);
#endif
    var albedo_a = m.base_color;
    if ((m.flags & F_TRIPLANAR) != 0u) {
        // Shader Graphs/Triplanar: world-space triplanar projection of the base map
        let p = world_pos * m.extra.w;
        var w = abs(normalize(in.world_normal));
        w = pow(w, vec3(4.0));
        w = w / (w.x + w.y + w.z);
        let tx = textureSample(base_tex, base_smp, p.zy);
        let ty = textureSample(base_tex, base_smp, p.xz);
        let tz = textureSample(base_tex, base_smp, p.xy);
        albedo_a = albedo_a * (tx * w.x + ty * w.y + tz * w.z);
    } else if ((m.flags & F_BASE_TEX) != 0u) {
        albedo_a = albedo_a * textureSampleBias(base_tex, base_smp, uv, m.fog_sky.a);
    }
#ifdef VERTEX_COLORS
    if ((m.flags & F_VERTEX_COLOR) != 0u) {
        albedo_a = vec4(albedo_a.rgb * mix(vec3(1.0), in.color.rgb, m.extra.z), albedo_a.a);
    }
#endif
    if ((m.flags & F_ALPHACLIP) != 0u && albedo_a.a < m.surface.w) {
        discard;
    }
    let albedo = albedo_a.rgb;

    if ((m.flags & F_UNLIT) != 0u) {
        return finish(albedo + m.emissive.rgb * m.emissive.w, world_pos, albedo_a.a);
    }

    var N = normalize(in.world_normal);
    if (!is_front) {
        N = -N;
    }
#ifdef VERTEX_TANGENTS
#ifdef VERTEX_UVS_A
    if ((m.flags & F_NORMALMAP) != 0u) {
        var T = normalize(in.world_tangent.xyz - N * dot(in.world_tangent.xyz, N));
        let B = in.world_tangent.w * cross(N, T);
        var nt = textureSampleBias(normal_tex, normal_smp, uv, m.fog_sky.a).rgb * 2.0 - 1.0;
        // exporter flipped V (Unity V-up -> glTF V-down): the bitangent flips, so flip tangent-space Y
        nt.y = nt.y * m.fog_sky.b;
        nt = vec3(nt.xy * m.surface.z, max(nt.z, 1e-3));
        N = normalize(T * nt.x + B * nt.y + N * nt.z);
    }
#endif
#endif

    let metallic = m.surface.x;
    let smoothness = m.surface.y;
    let b = init_brdf(albedo, metallic, smoothness);

    // ---------------------------------------------------------------- baked GI
    var baked_gi: vec3<f32>;
    var shadowmask = vec4<f32>(1.0);
#ifdef VERTEX_UVS_B
    // Unity V-up lightmap UVs: exporter stored uv1 as glTF V-down, lightmaps are top-down PNGs
    let uv1_unity = vec2(in.uv_b.x, 1.0 - in.uv_b.y);
    let lm_unity = uv1_unity * m.lightmap_st.xy + m.lightmap_st.zw;
    let lm_uv = vec2(lm_unity.x, 1.0 - lm_unity.y);
    if ((m.flags & F_LIGHTMAP) != 0u) {
        baked_gi = textureSample(lightmap_tex, lightmap_smp, lm_uv).rgb;
    } else {
        baked_gi = sample_sh(to_unity(N));
    }
    if ((m.flags & F_SHADOWMASK) != 0u) {
        shadowmask = textureSample(shadowmask_tex, shadowmask_smp, lm_uv);
    }
#else
    baked_gi = sample_sh(to_unity(N));
#endif

    // ---------------------------------------------------------------- GlobalIllumination
    // Contact shadows: screen-space AO (strength in wb.a) darkens creases and corners
    var occlusion = 1.0;
    var ao_rgb = vec3<f32>(1.0);
#ifdef SCREEN_SPACE_AMBIENT_OCCLUSION
    // wb.a = AO power: >1 deepens creases/contact shadows
    let ssao = pow(textureLoad(screen_space_ambient_occlusion_texture, vec2<i32>(in.position.xy), 0i).r, m.wb.a);
    ao_rgb = ssao_multibounce(ssao, albedo);
    occlusion = ssao;
#endif
    let R = reflect(-V, N);
    let NoV = saturate(dot(N, V));
    let fresnel = pow(1.0 - NoV, 4.0);
    // look calibration: GI saturation (fog_sky.r) and warmth (fog_sky.g)
    let gi_l = dot(baked_gi, vec3(0.2126, 0.7152, 0.0722));
    let gi_graded = mix(vec3(gi_l), baked_gi, m.fog_sky.r) * (vec3(1.0) + m.fog_sky.g * vec3(0.10, 0.0, -0.15));
    // hemispheric shaping of baked light (surface.y of the uniform's lightmap_st is used as-is; strength in dir_shadow_strength.w):
    // up-facing faces get more sky, down-facing less, so bevels and corners read like the original
    let hemi = clamp(1.0 + m.dir_shadow_strength.w * N.y, 0.2, 2.0);
    let indirect_diffuse = gi_graded * ao_rgb * m.extra.x * hemi;
    let indirect_specular = glossy_env(to_unity(R), b.perceptual_roughness, occlusion);
    var color = indirect_diffuse * b.diffuse;
    let surface_reduction = 1.0 / (b.roughness2 + 1.0);
    let ind_spec = surface_reduction * indirect_specular * mix(b.specular, vec3(b.grazing), fresnel);
    color = color + ind_spec;
    let ind_total = color;

    // ---------------------------------------------------------------- realtime lights
    let receive = (m.flags & F_RECEIVE_SHADOWS) != 0u;
    let view_z = dot(vec4<f32>(
        view_bindings::view.view_from_world[0].z,
        view_bindings::view.view_from_world[1].z,
        view_bindings::view.view_from_world[2].z,
        view_bindings::view.view_from_world[3].z
    ), in.world_position);

    let n_dir = view_bindings::lights.n_directional_lights;
    for (var i: u32 = 0u; i < n_dir; i = i + 1u) {
        let light = &view_bindings::lights.directional_lights[i];
        let L = (*light).direction_to_light.xyz;
        var shadow = 1.0;
        let idx = min(i, 2u);
        let strength = m.dir_shadow_strength[idx];
        if (receive && ((*light).flags & mesh_view_types::DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) != 0u) {
            let s = shadows::fetch_directional_shadow(i, in.world_position, in.world_normal, view_z);
            shadow = mix(1.0, s, strength);
        }
        let ch = i32(m.dir_shadowmask_channel[idx]);
        if (ch >= 0 && ch < 4 && (m.flags & F_SHADOWMASK) != 0u) {
            let baked = mix(1.0, shadowmask[ch], strength);
            shadow = min(shadow, baked);
        }
        color = color + lighting_pbr(b, (*light).color.rgb, shadow * mix(1.0, occlusion, 0.5), N, L, V);
    }

    let cluster_index = clustering::fragment_cluster_index(in.position.xy, view_z, false);
    let ranges = clustering::unpack_clusterable_object_index_ranges(cluster_index);
    for (var i: u32 = ranges.first_point_light_index_offset; i < ranges.first_reflection_probe_index_offset; i = i + 1u) {
        let light_id = clustering::get_clusterable_object_id(i);
        let light = &view_bindings::clusterable_objects.data[light_id];
        let to_light = (*light).position_radius.xyz - world_pos;
        let d2 = max(dot(to_light, to_light), HALF_MIN);
        let L = to_light * inverseSqrt(d2);
        // URP DistanceAttenuation
        let factor = d2 * (*light).color_inverse_square_range.w;
        let smooth_f = saturate(1.0 - factor * factor);
        var atten = (1.0 / d2) * smooth_f * smooth_f;
        if (i >= ranges.first_spot_light_index_offset) {
            var spot_dir = vec3<f32>((*light).light_custom_data.x, 0.0, (*light).light_custom_data.y);
            spot_dir.y = sqrt(max(0.0, 1.0 - spot_dir.x * spot_dir.x - spot_dir.z * spot_dir.z));
            if (((*light).flags & mesh_view_types::POINT_LIGHT_FLAGS_SPOT_LIGHT_Y_NEGATIVE) != 0u) {
                spot_dir.y = -spot_dir.y;
            }
            let cd = dot(-spot_dir, L);
            let a = saturate(cd * (*light).light_custom_data.z + (*light).light_custom_data.w);
            atten = atten * a * a;
            if (receive && ((*light).flags & mesh_view_types::POINT_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) != 0u) {
                atten = atten * shadows::fetch_spot_shadow(light_id, in.world_position, in.world_normal, (*light).shadow_map_near_z);
            }
        } else if (receive && ((*light).flags & mesh_view_types::POINT_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) != 0u) {
            atten = atten * shadows::fetch_point_shadow(light_id, in.world_position, in.world_normal);
        }
        color = color + lighting_pbr(b, (*light).color_inverse_square_range.rgb, atten, N, L, V);
    }

    var emission = m.emissive.rgb;
    if ((m.flags & F_EMISSIVE_TEX) != 0u) {
        emission = emission * textureSample(emissive_tex, emissive_smp, uv).rgb;
    }
    color = color + emission * m.emissive.w;

    if (m.debug == 1u) { return vec4(baked_gi, 1.0); }
    if (m.debug == 7u) { return vec4(vec3(occlusion), 1.0); }
    if (m.debug == 4u) { return vec4(color - ind_total, 1.0); }
    if (m.debug == 5u) { return vec4(indirect_diffuse * b.diffuse, 1.0); }
    if (m.debug == 6u) { return vec4(ind_spec, 1.0); }
    if (m.debug == 2u) { return vec4(albedo, 1.0); }
    if (m.debug == 3u) { return vec4(N * 0.5 + 0.5, 1.0); }

    return finish(color, world_pos, albedo_a.a);
}

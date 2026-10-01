#define_import_path gb::types

struct GbParams {
    base_color: vec4<f32>,
    emissive: vec4<f32>,
    // Unity lightmapScaleOffset (xy scale, zw offset), in Unity V-up UV space
    lightmap_st: vec4<f32>,
    // x metallic, y smoothness, z normal scale, w alpha cutoff
    surface: vec4<f32>,
    // base map tiling (xy) and offset (zw)
    base_st: vec4<f32>,
    // x occlusion strength, y env reflection intensity, z vertex color strength, w triplanar scale
    extra: vec4<f32>,
    sh: array<vec4<f32>, 7>,
    // per directional light (up to 4): shadow strength
    dir_shadow_strength: vec4<f32>,
    // per directional light: shadowmask channel (-1 none) ; negative big = light is baked (skip)
    dir_shadowmask_channel: vec4<f32>,
    // fog: rgb color, a = mode (0 off, 1 GB opaque surface fog, 2 unity linear, 3 exp, 4 exp2)
    fog_color: vec4<f32>,
    // skyFogMaxDepth, skyFogElevation, groundFogMaxDepth, groundFogElevation
    fog_params: vec4<f32>,
    // rgb sky color, a scatter intensity
    fog_sky: vec4<f32>,
    // x ground height reference, y density, z start, w end
    fog_extra: vec4<f32>,
    post: vec4<f32>,
    wb: vec4<f32>,
    // bit 0 lightmap, 1 shadowmask, 2 normal map, 3 alpha clip, 4 unlit, 5 sky, 6 receive shadows,
    // 7 triplanar, 8 vertex color, 9 has reflection cube, 10 base texture
    flags: u32,
    debug: u32,
    _pad0: u32,
    _pad1: u32,
};

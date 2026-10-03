// "URP FX/Water4" sea: Gerstner wave displacement (GerstnerOffset4), scrolling wave normal map, crest foam.
// Parameters come from the game's Water4 material (water-params.json). Unity-space maths: x is mirrored.
#import bevy_pbr::{
    mesh_functions,
    forward_io::{Vertex, VertexOutput, FragmentOutput},
    view_transformations::position_world_to_clip,
    mesh_view_bindings::{globals, view},
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing},
    view_transformations::depth_ndc_to_view_z,
}
#ifdef DEPTH_PREPASS
#import bevy_pbr::prepass_utils::prepass_depth
#endif

struct WaterParams {
    amp: vec4<f32>,
    freq: vec4<f32>,
    steep: vec4<f32>,
    speed: vec4<f32>,
    dir_ab: vec4<f32>,
    dir_cd: vec4<f32>,
    bump_tiling: vec4<f32>,
    bump_dir: vec4<f32>,
    // x: gerstner intensity, y: time scale, z: bump strength, w: foam amount
    misc: vec4<f32>,
}

@group(2) @binding(100) var<uniform> water: WaterParams;
@group(2) @binding(101) var bump_tex: texture_2d<f32>;
@group(2) @binding(102) var bump_sampler: sampler;

// Offsets (x, y, z) in Unity space for the Unity-space xz position.
fn gerstner(xz: vec2<f32>, t: f32) -> vec3<f32> {
    let ab = water.steep.xxyy * water.amp.xxyy * water.dir_ab;
    let cd = water.steep.zzww * water.amp.zzww * water.dir_cd;
    let dots = water.freq * vec4<f32>(
        dot(water.dir_ab.xy, xz),
        dot(water.dir_ab.zw, xz),
        dot(water.dir_cd.xy, xz),
        dot(water.dir_cd.zw, xz),
    );
    let ph = dots + t * water.speed;
    let c = cos(ph);
    let s = sin(ph);
    var o: vec3<f32>;
    o.x = dot(c, vec4<f32>(ab.xz, cd.xz));
    o.z = dot(c, vec4<f32>(ab.yw, cd.yw));
    o.y = dot(s, water.amp);
    return o;
}

// Surface slope (dY/dx, dY/dz) in Unity space.
fn gerstner_slope(xz: vec2<f32>, t: f32) -> vec2<f32> {
    let dots = water.freq * vec4<f32>(
        dot(water.dir_ab.xy, xz),
        dot(water.dir_ab.zw, xz),
        dot(water.dir_cd.xy, xz),
        dot(water.dir_cd.zw, xz),
    );
    let c = cos(dots + t * water.speed) * water.amp * water.freq;
    return vec2<f32>(
        dot(c, vec4<f32>(water.dir_ab.x, water.dir_ab.z, water.dir_cd.x, water.dir_cd.z)),
        dot(c, vec4<f32>(water.dir_ab.y, water.dir_ab.w, water.dir_cd.y, water.dir_cd.w)),
    );
}

@vertex
fn vertex(vertex: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);
    var wp = mesh_functions::mesh_position_local_to_world(world_from_local, vec4<f32>(vertex.position, 1.0));
    let t = globals.time * water.misc.y;
    let xz_u = vec2<f32>(-wp.x, wp.z);
    // Fade the swell out with distance: far LOD tiles have T-junctions that would otherwise crack open.
    let fade = 1.0 - smoothstep(35.0, 90.0, distance(wp.xyz, view.world_position));
    let o = gerstner(xz_u, t) * water.misc.x * fade;
    // Vertical displacement only: the sea is built from LOD tiles with T-junctions, horizontal offsets crack them open.
    wp = vec4<f32>(wp.x, wp.y + o.y, wp.z, wp.w);
    out.world_position = wp;
    out.position = position_world_to_clip(wp.xyz);
#ifdef VERTEX_NORMALS
    let sl = gerstner_slope(xz_u, t) * water.misc.x;
    let n_u = normalize(vec3<f32>(-sl.x, 1.0, -sl.y));
    out.world_normal = normalize(vec3<f32>(-n_u.x, n_u.y, n_u.z));
#endif
#ifdef VERTEX_UVS_A
    out.uv = vertex.uv;
#endif
#ifdef VERTEX_UVS_B
    out.uv_b = vertex.uv_b;
#endif
#ifdef VERTEX_TANGENTS
    out.world_tangent = mesh_functions::mesh_tangent_local_to_world(world_from_local, vertex.tangent, vertex.instance_index);
#endif
#ifdef VERTEX_COLORS
    out.color = vertex.color;
#endif
#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    out.instance_index = vertex.instance_index;
#endif
#ifdef VISIBILITY_RANGE_DITHER
    out.visibility_range_dither = mesh_functions::get_visibility_range_dither_level(vertex.instance_index, world_from_local[3]);
#endif
    return out;
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool, @builtin(sample_index) sample_index: u32) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);
    let t = globals.time;
    // Two scrolling layers of the wave normal map (Water4: _BumpTiling / _BumpDirection * _Time.x).
    let xz = vec2<f32>(-in.world_position.x, in.world_position.z);
    let n0 = textureSample(bump_tex, bump_sampler, xz * water.bump_tiling.xy + t * 0.05 * water.bump_dir.xy).xyz * 2.0 - 1.0;
    let n1 = textureSample(bump_tex, bump_sampler, xz * water.bump_tiling.zw + t * 0.05 * water.bump_dir.zw).xyz * 2.0 - 1.0;
    let nm = (n0.xy + n1.xy) * 0.5 * water.misc.z;
    pbr_input.N = normalize(pbr_input.N + vec3<f32>(-nm.x, 0.0, nm.y));
    // Crest foam from the wave height.
    let h = gerstner(xz, t * water.misc.y).y * water.misc.x;
    let foam = smoothstep(0.55, 0.95, h / max(dot(water.amp, vec4<f32>(1.0)) * water.misc.x, 0.001)) * water.misc.w * 0.3;
    var shore = 0.0;
#ifdef DEPTH_PREPASS
    // Shoreline / contact foam where opaque geometry (hulls, ice, buoys) meets the surface (Water4 depth fade).
    let scene_z = depth_ndc_to_view_z(prepass_depth(in.position, sample_index));
    let surface_z = depth_ndc_to_view_z(in.position.z);
    let gap = surface_z - scene_z;
    if (gap > 0.0) {
        shore = (1.0 - smoothstep(0.0, 0.45, gap)) * water.misc.w;
    }
#endif
    let foam_all = clamp(foam + shore * 0.9, 0.0, 1.0);
    let base = pbr_input.material.base_color;
    pbr_input.material.base_color = vec4<f32>(mix(base.rgb, vec3<f32>(0.85, 0.95, 1.0), foam_all), base.a);
    pbr_input.material.base_color = alpha_discard(pbr_input.material, pbr_input.material.base_color);
    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}

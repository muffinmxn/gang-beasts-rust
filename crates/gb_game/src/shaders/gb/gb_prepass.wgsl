// Depth/normal pre-pass + shadow pass for GbMaterial: honours alpha-clipped (cut-out) materials.
#import bevy_pbr::prepass_io::{VertexOutput, FragmentOutput}
#import gb::types::GbParams

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

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var out: FragmentOutput;
#ifdef VERTEX_UVS_A
    if ((m.flags & 8u) != 0u) {
        let uv = in.uv * m.base_st.xy + m.base_st.zw;
        let a = textureSample(base_tex, base_smp, uv).a * m.base_color.a;
        if (a < m.surface.w) {
            discard;
        }
    }
#endif
#ifdef NORMAL_PREPASS
    var n = normalize(in.world_normal);
    if (!is_front) { n = -n; }
    out.normal = vec4(n * 0.5 + 0.5, 1.0);
#endif
#ifdef MOTION_VECTOR_PREPASS
    out.motion_vector = vec2(0.0);
#endif
#ifdef DEPTH_CLAMP_ORTHO
    out.frag_depth = in.clip_position_unclamped.z;
#endif
    return out;
}

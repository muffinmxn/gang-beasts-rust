// Port of Hidden/GangBeasts/OpaqueSurfaceFog (0001_ps.asm), register by register:
//   eyeDepth  = LinearEyeDepth(depth)              (cb0[20] _ZBufferParams)
//   offset    = ray * (eyeDepth + near)            (ray = view-z-normalised corner lerp, cb0[127..130])
//   colour    = fogColor + mainLightColor * scatter * ((1 - dot(toCamera, L)) / 2)^4
//   sky       = min(eyeDepth, skyElevation / rayDir.y) / skyMaxDepth
//   ground    = height fog below groundElevation over groundMaxDepth (camera inside/outside cases)
//   alpha     = 1 - (1 - f^2)^4, f = min(max(ground, sky), 1)
#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput
#import bevy_render::view::View

struct SurfaceFog {
    fog_color: vec4<f32>,   // linear, scene units
    light_color: vec4<f32>, // main light colour * intensity, scene units
    light_dir: vec4<f32>,   // direction TO the light (world)
    p: vec4<f32>,           // scatter, sky max depth, sky elevation, ground max depth
    q: vec4<f32>,           // ground elevation, near, far (sky pixels), enabled
}

@group(0) @binding(0) var screen: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
@group(0) @binding(2) var<uniform> fog: SurfaceFog;
@group(0) @binding(3) var<uniform> view: View;
#ifdef MULTISAMPLED
@group(0) @binding(4) var depth_tex: texture_depth_multisampled_2d;
#else
@group(0) @binding(4) var depth_tex: texture_depth_2d;
#endif

const BIG: f32 = 1e20;

@fragment
fn fragment(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    let src = textureSample(screen, samp, in.uv);
    if (fog.q.w < 0.5) { return src; }
    let pix = vec2<i32>(in.position.xy);
    let d = textureLoad(depth_tex, pix, 0);
    let ndc = vec2(in.uv.x * 2.0 - 1.0, 1.0 - in.uv.y * 2.0);
    // Reverse-Z: 0 is the far plane (sky). Unity's sky pixels sit at the camera far plane.
    let wh = view.world_from_clip * vec4(ndc, max(d, 1e-6), 1.0);
    let cam = view.world_position;
    var off = wh.xyz / wh.w - cam;
    let fwd = normalize(-view.world_from_view[2].xyz);
    var eye = dot(off, fwd);
    if (d <= 0.0) {
        eye = fog.q.z;
    }
    let ray = off / max(dot(off, fwd), 1e-6);          // view-z-normalised ray (Unity corners)
    let dn = eye + fog.q.y;                              // eyeDepth + _ProjectionParams.y
    let o = ray * dn;
    let to_cam = normalize(-o);
    let s = pow((1.0 - dot(to_cam, normalize(fog.light_dir.xyz))) * 0.5, 4.0) * fog.p.x;
    let colour = s * fog.light_color.rgb + fog.fog_color.rgb;
    let up = max(-to_cam.y, 0.0);                        // ray direction y
    let sky_dist = select(BIG, fog.p.z / up, up > 0.0);
    let sky = min(eye, sky_dist) / fog.p.y;
    let point_y = dn * ray.y + cam.y;
    let below = fog.q.x - point_y;
    let cam_below = fog.q.x - cam.y;
    let dens = max(below, cam_below) / fog.p.w;
    let gpath = max(below / (-o.y) * eye / fog.p.w, 0.0);
    let top_dist = select(BIG, abs(cam_below / up), up > 0.0);
    let gin = min(eye, top_dist) / fog.p.w;
    let inside = select(0.0, 1.0, cam_below > 0.0);
    let g = clamp((inside * (gin - gpath) + gpath) * dens, 0.0, 1.0);
    let f = min(max(g, sky), 1.0);
    let a = 1.0 - f * f;
    let alpha = 1.0 - (a * a) * (a * a);
    return vec4(mix(src.rgb, colour, alpha), src.a);
}

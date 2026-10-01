// URP 12 HDR colour pipeline, ported from Unity's LutBuilderHdr.shader + ACES.hlsl
// (_TONEMAP_ACES path): postExposure -> contrast in ACEScc -> saturation (AP1 luminance) ->
// AcesTonemap -> linear Rec.709 (the swapchain applies the sRGB curve).
#import bevy_core_pipeline::fullscreen_vertex_shader::FullscreenVertexOutput

struct UrpPost {
    exposure: f32,   // 2^postExposure
    contrast: f32,   // 1 + contrast / 100
    saturation: f32, // 1 + saturation / 100
    bypass: f32,     // 1 = passthrough
    shadow: f32,     // >1 darkens the low end (deepens shadows); 1 = off
    shadow_knee: f32, // upper luminance bound of the shadow falloff ramp (0.45 = Unity-ish)
    // URP screen-space effects (the graphics options). 0 = off, which is the state every
    // harness baseline was captured in.
    ca: f32,         // _ChromaticAberration intensity (radial RGB split)
    vignette: f32,   // _Vignette intensity
    grain: f32,      // _FilmGrain intensity
    grain_time: f32, // animation seed so the grain does not look like dirt on the lens
    _pad0: f32,
    _pad1: f32,
}

@group(0) @binding(0) var screen: texture_2d<f32>;
@group(0) @binding(1) var samp: sampler;
@group(0) @binding(2) var<uniform> settings: UrpPost;

// WGSL mat3x3 constructors are column-major: each group of three is a column, so these are the
// transposes of Unity's row-major tables, and `m * v` equals Unity's mul(M, v).
const sRGB_2_AP0 = mat3x3<f32>(
    0.4397010, 0.0897923, 0.0175440,
    0.3829780, 0.8134230, 0.1115440,
    0.1773350, 0.0967616, 0.8707040);
const AP0_2_AP1 = mat3x3<f32>(
    1.4514393161, -0.0765537734, 0.0083161484,
    -0.2365107469, 1.1762296998, -0.0060324498,
    -0.2149285693, -0.0996759264, 0.9977163014);
const AP1_2_AP0 = mat3x3<f32>(
    0.6954522414, 0.0447945634, -0.0055258826,
    0.1406786965, 0.8596711185, 0.0040252103,
    0.1638690622, 0.0955343182, 1.0015006723);
const AP1_2_XYZ = mat3x3<f32>(
    0.6624541811, 0.2722287168, -0.0055746495,
    0.1340042065, 0.6740817658, 0.0040607335,
    0.1561876870, 0.0536895174, 1.0103391003);
const XYZ_2_AP1 = mat3x3<f32>(
    1.6410233797, -0.6636628587, 0.0117218943,
    -0.3248032942, 1.6153315917, -0.0082844420,
    -0.2364246952, 0.0167563477, 0.9883948585);
const XYZ_2_REC709 = mat3x3<f32>(
    3.2409699419, -0.9692436363, 0.0556300797,
    -1.5373831776, 1.8759675015, -0.2039769589,
    -0.4986107603, 0.0415550574, 1.0569715142);
const D60_2_D65 = mat3x3<f32>(
    0.98722400, -0.00759836, 0.00307257,
    -0.00611327, 1.00186000, -0.00509595,
    0.01595330, 0.00533020, 1.08168000);
const AP1_RGB2Y = vec3<f32>(0.272229, 0.674082, 0.0536895);

const HALF_MAX: f32 = 65504.0;
const ACEScc_MIDGRAY: f32 = 0.4135884;
const PI: f32 = 3.14159265359;

fn aces_to_acescc(x: vec3<f32>) -> vec3<f32> {
    let c = clamp(x, vec3(0.0), vec3(HALF_MAX));
    let lo = (log2(0.00001525878 + c * 0.5) + 9.72) / 17.52;
    let hi = (log2(max(c, vec3(1e-10))) + 9.72) / 17.52;
    return select(hi, lo, c < vec3(0.00003051757));
}

fn acescc_to_aces(x: vec3<f32>) -> vec3<f32> {
    let lo = (exp2(x * 17.52 - 9.72) - 0.00001525878) * 2.0;
    let mid = exp2(x * 17.52 - 9.72);
    let top = (log2(HALF_MAX) + 9.72) / 17.52;
    return select(select(mid, vec3(HALF_MAX), x >= vec3(top)), lo, x < vec3(-0.3013698630));
}

fn rgb_2_saturation(rgb: vec3<f32>) -> f32 {
    let tiny = 1e-4;
    let mi = min(rgb.r, min(rgb.g, rgb.b));
    let ma = max(rgb.r, max(rgb.g, rgb.b));
    return (max(ma, tiny) - max(mi, tiny)) / max(ma, 1e-2);
}

fn rgb_2_yc(rgb: vec3<f32>) -> f32 {
    let k = max(rgb.b * (rgb.b - rgb.g) + rgb.g * (rgb.g - rgb.r) + rgb.r * (rgb.r - rgb.b), 0.0);
    let chroma = select(sqrt(k), 0.0, k == 0.0);
    return (rgb.b + rgb.g + rgb.r + 1.75 * chroma) / 3.0;
}

fn sigmoid_shaper(x: f32) -> f32 {
    let t = max(1.0 - abs(x / 2.0), 0.0);
    return (1.0 + sign(x) * (1.0 - t * t)) * 0.5;
}

fn glow_fwd(yc: f32, gain: f32, mid: f32) -> f32 {
    if (yc <= 2.0 / 3.0 * mid) { return gain; }
    if (yc >= 2.0 * mid) { return 0.0; }
    return gain * (mid / yc - 0.5);
}

fn rgb_2_hue(rgb: vec3<f32>) -> f32 {
    var hue = 0.0;
    if (!(rgb.x == rgb.y && rgb.y == rgb.z)) {
        hue = (180.0 / PI) * atan2(sqrt(3.0) * (rgb.y - rgb.z), 2.0 * rgb.x - rgb.y - rgb.z);
    }
    if (hue < 0.0) { hue += 360.0; }
    return hue;
}

fn center_hue(hue: f32, center: f32) -> f32 {
    var h = hue - center;
    if (h < -180.0) { h += 360.0; } else if (h > 180.0) { h -= 360.0; }
    return h;
}

fn dark_to_dim_surround(cv: vec3<f32>) -> vec3<f32> {
    let xyz = AP1_2_XYZ * cv;
    let div = max(xyz.x + xyz.y + xyz.z, 1e-4);
    var xyY = vec3(xyz.x / div, xyz.y / div, xyz.y);
    xyY.z = pow(clamp(xyY.z, 0.0, HALF_MAX), 0.9811);
    let m = xyY.z / max(xyY.y, 1e-4);
    let back = vec3(xyY.x * m, xyY.z, (1.0 - xyY.x - xyY.y) * m);
    return XYZ_2_AP1 * back;
}

fn aces_tonemap(aces_in: vec3<f32>) -> vec3<f32> {
    var aces = aces_in;
    // Glow module
    let sat = rgb_2_saturation(aces);
    let yc = rgb_2_yc(aces);
    let s = sigmoid_shaper((sat - 0.4) / 0.2);
    aces *= 1.0 + glow_fwd(yc, 0.05 * s, 0.08);
    // Red modifier
    let hue = center_hue(rgb_2_hue(aces), 0.0);
    var hw = smoothstep(0.0, 1.0, 1.0 - abs(2.0 * hue / 135.0));
    hw *= hw;
    aces.r += hw * sat * (0.03 - aces.r) * (1.0 - 0.82);
    // ACES -> ACEScg, global desaturation
    var cg = max(vec3(0.0), AP0_2_AP1 * aces);
    cg = mix(vec3(dot(cg, AP1_RGB2Y)), cg, 0.96);
    // RRT + ODT (RGBmonitor_100nits_dim) luminance fit
    let a = 2.785085; let b = 0.107772; let c = 2.936045; let d = 0.887122; let e = 0.806889;
    let post = (cg * (a * cg + b)) / (cg * (c * cg + d) + e);
    var cv = dark_to_dim_surround(post);
    cv = mix(vec3(dot(cv, AP1_RGB2Y)), cv, 0.93);
    return XYZ_2_REC709 * (D60_2_D65 * (AP1_2_XYZ * cv));
}

@fragment
fn fragment(in: FullscreenVertexOutput) -> @location(0) vec4<f32> {
    var src = textureSample(screen, samp, in.uv);
    if (settings.bypass > 0.5) { return src; }
    // Chromatic aberration (URP UberPost, applied to the HDR source before grading): split the
    // channels radially around the frame centre, growing with distance.
    if (settings.ca > 0.0001) {
        let offset = (in.uv - vec2(0.5, 0.5)) * settings.ca * 0.02;
        src = vec4<f32>(
            textureSample(screen, samp, in.uv - offset).r,
            src.g,
            textureSample(screen, samp, in.uv + offset).b,
            src.a,
        );
    }
    let linear = max(src.rgb * settings.exposure, vec3(0.0));
    // Contrast in ACEScc log space (LutBuilderHdr, _TONEMAP_ACES)
    var lg = aces_to_acescc(sRGB_2_AP0 * linear);
    lg = (lg - ACEScc_MIDGRAY) * settings.contrast + ACEScc_MIDGRAY;
    var cg = max(vec3(0.0), AP0_2_AP1 * acescc_to_aces(lg));
    // Global saturation around ACES luminance
    let luma = dot(cg, AP1_RGB2Y);
    cg = vec3(luma) + settings.saturation * (cg - vec3(luma));
    let out = aces_tonemap(AP1_2_AP0 * cg);
    // Shadow depth: darken only the low end (occluded areas) while leaving midtones and
    // highlights alone, so the image gains depth without going overall darker. shadow = 1 is a
    // no-op. This is a diagnostic grading control; source spatial shadows come from lighting.
    let out_luma = dot(out, AP1_RGB2Y);
    let weight = 1.0 - smoothstep(0.0, settings.shadow_knee, out_luma);
    var shaded = out * mix(1.0, 1.0 / max(settings.shadow, 0.001), weight);
    // Vignette (URP ApplyVignette, display-referred): radial falloff from the frame centre with
    // URP's default smoothness 0.2.
    if (settings.vignette > 0.0001) {
        let dist = length(in.uv - vec2(0.5, 0.5)) * 1.41421356;
        let fade = smoothstep(0.2, 1.0, dist);
        shaded *= mix(1.0, 1.0 - settings.vignette, fade);
    }
    // Film grain (URP ApplyGrain): display-referred noise weighted by a luminance response, so
    // mid-greys show the most grain and blacks/whites the least.
    if (settings.grain > 0.0001) {
        let n = fract(sin(dot(in.uv * 1024.0 + settings.grain_time, vec2(12.9898, 78.233))) * 43758.5453);
        let response = clamp(out_luma * 2.0 * (1.0 - out_luma * 0.6), 0.0, 1.0);
        shaded += (vec3(n) - 0.5) * settings.grain * 0.35 * response;
    }
    return vec4(clamp(shaded, vec3(0.0), vec3(1.0)), src.a);
}

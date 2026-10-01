// URP 12 (Unity 2021) color pipeline, translated from Unity's HLSL:
//   UberPost: color *= postExposure  -> LutBuilderHdr: contrast (ACEScc), saturation (ACEScg)
//   -> ACES tonemap (RRT+ODT fitted, from Core RP ACES.hlsl).
// Output is linear display-referred color; the sRGB swapchain/target encodes it.
#define_import_path gb::urp_post

const sRGB_2_AP0 = mat3x3<f32>(
    vec3<f32>(0.4397010, 0.0897923, 0.0175440),
    vec3<f32>(0.3829780, 0.8134230, 0.1115440),
    vec3<f32>(0.1773350, 0.0967616, 0.8707040));
const AP0_2_AP1 = mat3x3<f32>(
    vec3<f32>(1.4514393161, -0.0765537734, 0.0083161484),
    vec3<f32>(-0.2365107469, 1.1762296998, -0.0060324498),
    vec3<f32>(-0.2149285693, -0.0996759264, 0.9977163014));
const AP1_2_AP0 = mat3x3<f32>(
    vec3<f32>(0.6954522414, 0.0447945634, -0.0055258826),
    vec3<f32>(0.1406786965, 0.8596711185, 0.0040252103),
    vec3<f32>(0.1638690622, 0.0955343182, 1.0015006723));
const AP1_2_XYZ = mat3x3<f32>(
    vec3<f32>(0.6624541811, 0.2722287168, -0.0055746495),
    vec3<f32>(0.1340042065, 0.6740817658, 0.0040607335),
    vec3<f32>(0.1561876870, 0.0536895174, 1.0103391003));
const XYZ_2_AP1 = mat3x3<f32>(
    vec3<f32>(1.6410233797, -0.6636628587, 0.0117218943),
    vec3<f32>(-0.3248032942, 1.6153315917, -0.0082844420),
    vec3<f32>(-0.2364246952, 0.0167563477, 0.9883948585));
const D60_2_D65_CAT = mat3x3<f32>(
    vec3<f32>(0.98722400, -0.00759836, 0.00307257),
    vec3<f32>(-0.00611327, 1.00186000, -0.00509595),
    vec3<f32>(0.01595330, 0.00533020, 1.08168000));
const XYZ_2_REC709 = mat3x3<f32>(
    vec3<f32>(3.2409699419, -0.9692436363, 0.0556300797),
    vec3<f32>(-1.5373831776, 1.8759675015, -0.2039769589),
    vec3<f32>(-0.4986107603, 0.0415550574, 1.0569715142));
const AP1_RGB2Y = vec3<f32>(0.2722287168, 0.6740817658, 0.0536895174);
const ACEScc_MIDGRAY: f32 = 0.4135884;

fn aces_to_acescc_1(x: f32) -> f32 {
    if (x <= 0.0) { return -0.3584474886; }
    if (x < 3.0517578e-05) { return (log2(1.5258789e-05 + x * 0.5) + 9.72) / 17.52; }
    return (log2(x) + 9.72) / 17.52;
}
fn acescc_to_aces_1(x: f32) -> f32 {
    if (x < -0.3013698630) { return (exp2(x * 17.52 - 9.72) - 1.5258789e-05) * 2.0; }
    if (x < 1.4679963) { return exp2(x * 17.52 - 9.72); }
    return 65504.0;
}
fn aces_to_acescc(v: vec3<f32>) -> vec3<f32> { return vec3(aces_to_acescc_1(v.x), aces_to_acescc_1(v.y), aces_to_acescc_1(v.z)); }
fn acescc_to_aces(v: vec3<f32>) -> vec3<f32> { return vec3(acescc_to_aces_1(v.x), acescc_to_aces_1(v.y), acescc_to_aces_1(v.z)); }

fn rgb_2_saturation(rgb: vec3<f32>) -> f32 {
    let mx = max(max(rgb.r, rgb.g), rgb.b);
    let mn = min(min(rgb.r, rgb.g), rgb.b);
    return (max(mx, 1e-4) - max(mn, 1e-4)) / max(mx, 1e-2);
}
fn rgb_2_yc(rgb: vec3<f32>) -> f32 {
    let chroma = sqrt(max(rgb.b * (rgb.b - rgb.g) + rgb.g * (rgb.g - rgb.r) + rgb.r * (rgb.r - rgb.b), 0.0));
    return (rgb.b + rgb.g + rgb.r + 1.75 * chroma) / 3.0;
}
fn sigmoid_shaper(x: f32) -> f32 {
    let t = max(1.0 - abs(0.5 * x), 0.0);
    let y = 1.0 + sign(x) * (1.0 - t * t);
    return 0.5 * y;
}
fn glow_fwd(yc: f32, gain: f32, mid: f32) -> f32 {
    if (yc <= 2.0 / 3.0 * mid) { return gain; }
    if (yc >= 2.0 * mid) { return 0.0; }
    return gain * (mid / yc - 0.5);
}
fn rgb_2_hue(rgb: vec3<f32>) -> f32 {
    var hue = 0.0;
    if (!(rgb.r == rgb.g && rgb.g == rgb.b)) {
        hue = 57.2957795 * atan2(sqrt(3.0) * (rgb.g - rgb.b), 2.0 * rgb.r - rgb.g - rgb.b);
    }
    if (hue < 0.0) { hue = hue + 360.0; }
    return hue;
}
fn center_hue(hue: f32, c: f32) -> f32 {
    var h = hue - c;
    if (h < -180.0) { h = h + 360.0; } else if (h > 180.0) { h = h - 360.0; }
    return h;
}
fn dark_to_dim_surround(linear_cv: vec3<f32>) -> vec3<f32> {
    let XYZ = AP1_2_XYZ * linear_cv;
    let s = max(XYZ.x + XYZ.y + XYZ.z, 1e-10);
    var xyY = vec3<f32>(XYZ.x / s, XYZ.y / s, XYZ.y);
    xyY.z = pow(clamp(xyY.z, 0.0, 65504.0), 0.9811);
    let Y = xyY.z;
    let X = xyY.x * Y / max(xyY.y, 1e-10);
    let Z = (1.0 - xyY.x - xyY.y) * Y / max(xyY.y, 1e-10);
    return XYZ_2_AP1 * vec3(X, Y, Z);
}

// Core RP ACES.hlsl AcesTonemap (TONEMAPPING_USE_FULL_ACES 0), input in ACES2065-1 (AP0).
fn aces_tonemap(aces_in: vec3<f32>) -> vec3<f32> {
    var aces = aces_in;
    // Glow module
    let saturation = rgb_2_saturation(aces);
    let yc_in = rgb_2_yc(aces);
    let s = sigmoid_shaper((saturation - 0.4) / 0.2);
    let added_glow = 1.0 + glow_fwd(yc_in, 0.05 * s, 0.08);
    aces = aces * added_glow;
    // Red modifier
    let hue = rgb_2_hue(aces);
    let centered = center_hue(hue, 0.0);
    var hue_weight = smoothstep(0.0, 1.0, 1.0 - abs(2.0 * centered / 135.0));
    hue_weight = hue_weight * hue_weight;
    aces.r = aces.r + hue_weight * saturation * (0.03 - aces.r) * (1.0 - 0.82);
    // ACES -> rendering space
    var acescg = max(vec3(0.0), AP0_2_AP1 * aces);
    // Global desaturation
    acescg = mix(vec3(dot(acescg, AP1_RGB2Y)), acescg, 0.96);
    // Fitted RRT + ODT
    let a = 278.5085;
    let b = 10.7772;
    let c = 293.6045;
    let d = 88.7122;
    let e = 80.6889;
    let x = acescg;
    let rgb_post = (x * (a * x + b)) / (x * (c * x + d) + e);
    var linear_cv = dark_to_dim_surround(rgb_post);
    linear_cv = mix(vec3(dot(linear_cv, AP1_RGB2Y)), linear_cv, 0.93);
    var XYZ = AP1_2_XYZ * linear_cv;
    XYZ = D60_2_D65_CAT * XYZ;
    return XYZ_2_REC709 * XYZ;
}

fn neutral_curve(x: vec3<f32>, a: f32, b: f32, c: f32, d: f32, e: f32, f: f32) -> vec3<f32> {
    return ((x * (a * x + c * b) + d * e) / (x * (a * x + b) + d * f)) - e / f;
}
fn neutral_tonemap(x_in: vec3<f32>) -> vec3<f32> {
    let white_clip = 1.0;
    let x = min(x_in, vec3(435.18712));
    let white_scale = 1.0 / neutral_curve(vec3(5.3), 0.2, 0.29, 0.24, 0.272, 0.02, 0.3).x;
    return clamp(neutral_curve(x * white_scale, 0.2, 0.29, 0.24, 0.272, 0.02, 0.3) * white_scale / white_clip, vec3(0.0), vec3(1.0));
}

// post.x = 2^postExposure, post.y = contrast mult, post.z = saturation mult, post.w = tonemapper
fn urp_color_pipeline(color_in: vec3<f32>, post: vec4<f32>) -> vec3<f32> {
    var c = max(color_in, vec3(0.0)) * post.x;
    // LutBuilderHdr: contrast in ACEScc
    var log_c = aces_to_acescc(sRGB_2_AP0 * c);
    log_c = (log_c - ACEScc_MIDGRAY) * post.y + ACEScc_MIDGRAY;
    var cg = AP0_2_AP1 * acescc_to_aces(log_c);
    // saturation in ACEScg
    let luma = dot(cg, AP1_RGB2Y);
    cg = vec3(luma) + post.z * (cg - vec3(luma));
    cg = max(cg, vec3(0.0));
    if (post.w > 1.5) {
        return saturate(aces_tonemap(AP1_2_AP0 * cg));
    }
    // back to linear sRGB for other tonemappers
    let aces = AP1_2_AP0 * cg;
    let XYZ = D60_2_D65_CAT * (AP1_2_XYZ * cg);
    let lin = XYZ_2_REC709 * XYZ;
    if (post.w > 0.5) {
        return neutral_tonemap(lin);
    }
    return saturate(lin + 0.0 * aces);
}

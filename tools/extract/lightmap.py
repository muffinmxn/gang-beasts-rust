"""Convert Unity's encoded static lightmaps for Bevy's linear RGB lightmap sampler."""
import os
import numpy as np
from PIL import Image


def unity_lightmap_encoding(texture_format, color_space):
    """Return the legacy format policy and its explicit texture-sampling contract.

    The caller currently supplies Texture2D.m_ColorSpace. Its relationship to
    the project's shader color-space keyword still needs source validation;
    texture format alone is not a universal lightmap-encoding discriminator.
    """
    # Desktop static lightmaps are RGBM whatever the container: BC7 (25), DXT5 (12) or uncompressed RGBA32 (4) with the
    # multiplier in alpha. Treating RGBA32 as 'double LDR' ignored alpha and produced near-white garbage atlases.
    if int(texture_format) in (25, 12, 4) and not os.environ.get("GB_LEGACY_DLDR"):
        if int(color_space) == 1:  # Unity ColorSpace.Linear
            return {"kind": "rgbm", "exposure": 34.493242, "exponent": 2.2}
        return {"kind": "rgbm", "exposure": 5.0, "exponent": 1.0}
    # Low-quality lightmap encoding (RGBA32/RGB24): double-LDR. In these bundles the stored
    # dLDR RGB is sampled as linear encoded data. `m_ColorSpace == 1` selects Unity's linear-
    # project HDR multiplier; it does not make this texture an sRGB sampler. A Rooftop A/B
    # against the original renderer (58.2% -> 29.8%) rejects sRGB-linearising these texels.
    if int(texture_format) in (3, 4) and int(color_space) == 1:
        return {"kind": "dldr", "exposure": 4.594794, "exponent": 1.0,
                "source_srgb": False}
    if int(texture_format) in (3, 4):
        return {"kind": "dldr", "exposure": 2.0, "exponent": 1.0,
                "source_srgb": False}
    return {"kind": "direct", "exposure": 1.0, "exponent": 1.0}


def decode_lightmap(image, encoding):
    """Bake Unity's texture decode into sRGB PNG channels for linear GPU sampling.

    The HDR multiplier remains in encoding['exposure'] for the runtime material.
    source_srgb describes the input sampler, independently of the output PNG.
    """
    pixels = np.asarray(image.convert("RGBA"), dtype=np.float32) / 255.0
    rgb = pixels[:, :, :3]
    if encoding["kind"] == "rgbm":
        # Unity marks linear-project lightmaps sRGB; texture sampling linearizes RGB before
        # applying the RGBM multiplier in alpha. Preserve that order before re-encoding PNG.
        if encoding.get("source_srgb", True):
            rgb = srgb_to_linear(rgb)
        rgb = rgb * np.power(pixels[:, :, 3:4], encoding["exponent"])
    elif encoding["kind"] == "dldr":
        # Unity EntityLighting.hlsl multiplies the already-sampled RGB. Whether the
        # sampler decodes sRGB must come from the explicit texture contract.
        if encoding.get("source_srgb", False):
            rgb = srgb_to_linear(rgb)

    rgb = np.maximum(rgb, 0.0)
    srgb = np.where(rgb <= 0.0031308, rgb * 12.92, 1.055 * np.power(rgb, 1.0 / 2.4) - 0.055)
    rgba = np.empty_like(pixels)
    rgba[:, :, :3] = np.clip(srgb, 0.0, 1.0)
    rgba[:, :, 3] = 1.0
    return Image.fromarray(np.round(rgba * 255.0).astype(np.uint8), "RGBA")


def srgb_to_linear(rgb):
    return np.where(rgb <= 0.04045, rgb / 12.92, ((rgb + 0.055) / 1.055) ** 2.4)

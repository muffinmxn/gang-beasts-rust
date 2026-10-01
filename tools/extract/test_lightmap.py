import unittest

import numpy as np
from PIL import Image

from lightmap import decode_lightmap, unity_lightmap_encoding


class UnityLightmapTests(unittest.TestCase):
    def test_linear_bc7_uses_unity_linear_rgbm_decode(self):
        encoding = unity_lightmap_encoding(25, 1)
        self.assertEqual(encoding, {"kind": "rgbm", "exposure": 34.493242, "exponent": 2.2})
        source = np.array([[[128, 64, 32, 128]]], dtype=np.uint8)
        result = np.asarray(decode_lightmap(Image.fromarray(source, "RGBA"), encoding))[0, 0]
        linear = (result[:3] / 255.0)
        linear = np.where(linear <= 0.04045, linear / 12.92, ((linear + 0.055) / 1.055) ** 2.4)
        encoded_rgb = source[0, 0, :3] / 255.0
        source_linear = np.where(
            encoded_rgb <= 0.04045,
            encoded_rgb / 12.92,
            ((encoded_rgb + 0.055) / 1.055) ** 2.4,
        )
        expected = source_linear * (source[0, 0, 3] / 255.0) ** 2.2
        np.testing.assert_allclose(linear, expected, atol=0.004)
        self.assertEqual(result[3], 255)

    def test_gamma_bc7_keeps_unity_gamma_rgbm_decode(self):
        encoding = unity_lightmap_encoding(25, 0)
        self.assertEqual(encoding, {"kind": "rgbm", "exposure": 5.0, "exponent": 1.0})

    def test_non_bc7_lightmap_is_passed_through(self):
        self.assertEqual(unity_lightmap_encoding(17, 1), {"kind": "direct", "exposure": 1.0, "exponent": 1.0})

    def test_linear_project_dldr_preserves_source_linear_values(self):
        # The source bytes are linear encoded values. Encode those to the output PNG;
        # its sRGB GPU sampler recovers the same values before separate HDR exposure.
        ramp = np.arange(256, dtype=np.uint8)
        source = np.stack((ramp, ramp[::-1], ramp, ramp), axis=-1)[None, :, :]
        for texture_format in (3, 4):
            with self.subTest(texture_format=texture_format):
                encoding = unity_lightmap_encoding(texture_format, 1)
                self.assertFalse(encoding["source_srgb"])
                self.assertAlmostEqual(encoding["exposure"], 2 ** 2.2, places=5)
                result = np.asarray(decode_lightmap(Image.fromarray(source), encoding))
                # 128 linear becomes approximately 188 when stored as sRGB PNG.
                self.assertEqual(result[0, 128, 0], 188)
                np.testing.assert_array_equal(result[:, :, 3], 255)

    def test_gamma_dldr_retains_unlinearized_sample_values(self):
        encoding = unity_lightmap_encoding(4, 0)
        self.assertFalse(encoding["source_srgb"])
        self.assertEqual(encoding["exposure"], 2.0)
        source = Image.fromarray(np.array([[[128, 64, 0, 12]]], dtype=np.uint8))
        result = np.asarray(decode_lightmap(source, encoding))[0, 0]
        # sRGB encoding of 128/255 and 64/255 linear intensity, not a
        # second sRGB decode. Alpha is not a multiplier for double-LDR.
        np.testing.assert_array_equal(result, [188, 137, 0, 255])

    def test_legacy_dldr_contract_and_explicit_sampler_override(self):
        source = Image.fromarray(np.array([[[128, 64, 0, 0]]], dtype=np.uint8))
        legacy = {"kind": "dldr", "exposure": 4.594794, "exponent": 1.0}
        np.testing.assert_array_equal(
            np.asarray(decode_lightmap(source, legacy))[0, 0], [188, 137, 0, 255])
        # Explicit sampler metadata wins even when exposure is supplied by
        # a different shader decode contract.
        explicit = {**legacy, "source_srgb": True}
        np.testing.assert_array_equal(
            np.asarray(decode_lightmap(source, explicit))[0, 0], [128, 64, 0, 255])


if __name__ == "__main__":
    unittest.main()

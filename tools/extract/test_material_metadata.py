"""Small metadata/GLB declaration regressions; no Unity installs, bundles or pixels required."""
import ast
import json
from pathlib import Path
import struct
import tempfile
from types import SimpleNamespace as NS
import unittest

from material_metadata import material_metadata, texture_metadata


class Reader:
    def __init__(self, file, path_id, data, tree=None):
        self.assets_file, self.path_id, self.data, self.tree = file, path_id, data, tree
        self.reads = 0

    def read(self):
        self.reads += 1
        return self.data

    def read_typetree(self):
        return self.tree


class Pointer:
    def __init__(self, file, path_id, reader=None, file_id=0):
        self.assetsfile, self.m_PathID = file, path_id
        self.m_FileID, self.reader = file_id, reader

    def deref(self):
        if self.reader is None:
            raise LookupError("external file missing")
        return self.reader


class NoPixels(NS):
    @property
    def image(self):
        raise AssertionError("Metadata must never decode pixels")


def source_file(name):
    return NS(name=name, parent=NS(name="fixture.bundle"), externals=[NS(path="shared.assets")])


class MetadataTests(unittest.TestCase):
    def test_full_source_slots_and_state_without_pixels(self):
        owner, shared = source_file("scene.assets"), source_file("shared.assets")
        texture = Reader(shared, 9007199254740993, NoPixels(
            m_Name="Shared", m_TextureFormat=12, m_ColorSpace=0,
            m_Width=4, m_Height=4, m_MipCount=2,
            m_TextureSettings=NS(m_FilterMode=2, m_Aniso=4, m_MipBias=-0.5,
                                 m_WrapU=0, m_WrapV=1, m_WrapW=2)))
        ptr = Pointer(owner, texture.path_id, texture, file_id=1)
        slot = NS(m_Texture=ptr, m_Scale=NS(x=2.0, y=-3.0), m_Offset=NS(x=0.1, y=0.2))
        shader = Reader(shared, 7, NS(m_ParsedForm=NS(m_Name="Shader Graphs/Custom")))
        raw = {"m_ShaderKeywords": "A B", "m_ValidKeywords": ["A"],
               "m_InvalidKeywords": ["B"], "m_CustomRenderQueue": 2450,
               "stringTagMap": [("RenderType", "TransparentCutout")],
               "disabledShaderPasses": ["ShadowCaster"],
               "m_SavedProperties": {"m_Ints": [("_Cull", 0)],
                                      "m_Floats": [("_ZWrite", 1.0)]}}
        mat = NS(m_Shader=Pointer(owner, 7, shader, 1),
                 m_SavedProperties=NS(m_TexEnvs=[("_BaseMap", slot), ("_CustomMask", slot)]))
        mat.object_reader = Reader(owner, 9, mat, raw)
        result = material_metadata(mat, {})
        self.assertEqual(result["shader"]["name"], "Shader Graphs/Custom")
        self.assertEqual(result["state"]["disabledShaderPasses"], ["ShadowCaster"])
        self.assertEqual(result["properties"]["m_Ints"], [("_Cull", 0)])
        self.assertEqual(result["texture_slots"]["_CustomMask"]["scale"], [2.0, -3.0])
        self.assertEqual(result["texture_slots"]["_CustomMask"]["offset"], [0.1, 0.2])
        info = result["texture_slots"]["_BaseMap"]["texture"]
        self.assertEqual(info["reference"]["target_file"], "shared.assets")
        self.assertEqual(info["source"]["path_id"], "9007199254740993")
        self.assertEqual(info["sampler"]["m_WrapV"], 1)
        self.assertEqual(info["m_ColorSpace"], 0)
        self.assertEqual(texture.reads, 1)
        json.dumps(result, allow_nan=False)

    def test_missing_and_null_references_are_retained(self):
        file = source_file("source.assets")
        result = texture_metadata(Pointer(file, 17, file_id=1), {})
        self.assertEqual(result["status"], "unresolved")
        self.assertEqual(result["reference"]["path_id"], "17")
        self.assertEqual(texture_metadata(Pointer(file, 0), {})["status"], "null")

    def test_same_path_id_in_different_files_not_aliased(self):
        cache = {}
        for name in ("a.assets", "b.assets"):
            file = source_file(name)
            reader = Reader(file, 42, NoPixels(m_Name=name))
            result = texture_metadata(Pointer(file, 42, reader), cache)
            self.assertEqual(result["m_Name"], name)
        self.assertEqual(len(cache), 2)

    def test_glb_declares_both_used_extensions(self):
        # Execute the actual writer without importing UnityPy/numpy or loading game assets.
        path = Path(__file__).with_name("export.py")
        tree = ast.parse(path.read_text(encoding="utf-8"))
        cls = next(n for n in tree.body if isinstance(n, ast.ClassDef) and n.name == "Gltf")
        cls.body = [n for n in cls.body if isinstance(n, ast.FunctionDef) and n.name in ("__init__", "write")]
        safe = next(n for n in tree.body if isinstance(n, ast.FunctionDef) and n.name == "jsonable")
        namespace = {"json": json, "struct": struct}
        exec(compile(ast.Module(body=[safe, cls], type_ignores=[]), str(path), "exec"), namespace)
        writer = namespace["Gltf"]()
        writer.materials = [{"extensions": {"KHR_materials_emissive_strength": {"emissiveStrength": 4.0}}}]
        writer.uses_texture_transform = True
        with tempfile.TemporaryDirectory() as tmp:
            dest = Path(tmp) / "tiny.glb"
            writer.write(dest, [])
            data = dest.read_bytes()
        size = struct.unpack_from("<I", data, 12)[0]
        document = json.loads(data[20:20 + size])
        self.assertEqual(set(document["extensionsUsed"]),
                         {"KHR_texture_transform", "KHR_materials_emissive_strength"})
        self.assertNotIn("images", document)


if __name__ == "__main__":
    unittest.main()

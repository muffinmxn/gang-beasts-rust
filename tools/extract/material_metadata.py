"""Source material metadata only: never decode images or copy texture/shader blobs."""


def identity(reader):
    file = reader.assets_file
    return {"file": file.name, "path_id": str(reader.path_id),
            "bundle": getattr(getattr(file, "parent", None), "name", None)}


def pointer_identity(ptr):
    file = ptr.assetsfile
    file_id = int(ptr.m_FileID)
    target = file.name if file_id == 0 else None
    if 0 < file_id <= len(file.externals):
        target = file.externals[file_id - 1].path
    return {"owner_file": file.name, "target_file": target,
            "file_id": file_id, "path_id": str(ptr.m_PathID)}


def texture_metadata(ptr, cache):
    result = {"reference": pointer_identity(ptr)}
    if not ptr.m_PathID:
        result["status"] = "null"
        return result
    try:
        reader = ptr.deref()
        source = identity(reader)
        key = (source["bundle"], source["file"], source["path_id"])
        if key not in cache:
            texture = reader.read()
            fields = ("m_Name", "m_Width", "m_Height", "m_TextureFormat", "m_ColorSpace",
                      "m_MipCount", "m_ImageCount", "m_LightmapFormat", "m_IsReadable")
            data = {k: getattr(texture, k) for k in fields if hasattr(texture, k)}
            sampler = getattr(texture, "m_TextureSettings", None)
            data["sampler"] = {k: getattr(sampler, k) for k in
                               ("m_FilterMode", "m_Aniso", "m_MipBias", "m_WrapMode",
                                "m_WrapU", "m_WrapV", "m_WrapW") if hasattr(sampler, k)}
            cache[key] = {"source": source, **data}
        result.update(cache[key])
        result["status"] = "resolved"
    except Exception as error:
        result.update(status="unresolved", error=str(error)[:240])
    return result


def material_metadata(mat, texture_cache):
    """Preserve raw Unity names/UV convention, even for unrecognized shader slots.

    Material values are source state, not a claim that a shader pass is reproduced.
    Shader-derived pass defaults remain in the referenced source shader.
    """
    tree = mat.object_reader.read_typetree()
    fields = ("m_ValidKeywords", "m_InvalidKeywords", "m_ShaderKeywords",
              "m_CustomRenderQueue", "m_LightmapFlags", "m_EnableInstancingVariants",
              "m_DoubleSidedGI", "stringTagMap", "disabledShaderPasses")
    result = {"version": 1, "source": identity(mat.object_reader),
              "state": {k: tree[k] for k in fields if k in tree},
              "shader": {"reference": pointer_identity(mat.m_Shader)},
              "texture_slots": {}, "uv_convention": "Unity source (V-up)"}
    # Includes integer properties that the legacy float/color extras cannot represent.
    saved = tree.get("m_SavedProperties", {})
    result["properties"] = {k: v for k, v in saved.items() if k != "m_TexEnvs"}
    try:
        shader_reader = mat.m_Shader.deref()
        result["shader"]["source"] = identity(shader_reader)
        shader = shader_reader.read()
        parsed = getattr(shader, "m_ParsedForm", None)
        result["shader"]["name"] = getattr(parsed, "m_Name", getattr(shader, "m_Name", None))
    except Exception as error:
        result["shader"]["error"] = str(error)[:240]
    for name, slot in mat.m_SavedProperties.m_TexEnvs:
        result["texture_slots"][name] = {
            "scale": [slot.m_Scale.x, slot.m_Scale.y],
            "offset": [slot.m_Offset.x, slot.m_Offset.y],
            "texture": texture_metadata(slot.m_Texture, texture_cache),
        }
    return result

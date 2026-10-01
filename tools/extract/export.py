"""Export a Unity prefab or scene from the Gang Beasts bundles to .glb + sidecar .json.

  python export.py prefab "Core/Beasts/actor_humanoidMediumEctomorph.prefab" beast
  python export.py costume-prefab original-costumes "Assets/Costumes/BundleAssets/Prefabs/Part.prefab" costume_part
  python export.py costume-data costume-databases CostumePresetDatabase costume_presets
  python export.py scene stages-rooftop_scenes rooftop
  python export.py settings physics

.glb: full transform hierarchy (node names = GameObject names), meshes, skins, base-color textures.
      Converted to glTF space (right-handed): x is mirrored, winding flipped, v flipped.
.json: per node (by glTF node index): Unity path, layer, active, and every non-render component
       as its raw Unity typetree (Rigidbody, *Collider, ConfigurableJoint, MonoBehaviour + script name).
       Values in the sidecar stay in raw Unity space; the engine converts with unity_to_gltf.
"""
import sys, os, io, json, struct, glob
import UnityPy
import numpy as np
from geometry import geometry, trs
from lightmap import decode_lightmap, unity_lightmap_encoding
from material_metadata import material_metadata
from UnityPy.helpers.MeshHelper import MeshHandler

UnityPy.config.FALLBACK_UNITY_VERSION = "2021.3.33f1"
ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
# Local copy of the game (read-only). Override with GB_GAME_DIR.
GAME = os.environ.get("GB_GAME_DIR", os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "Base Game (Microsoft Store Version)", "Gang Beasts", "Content"))
AA = os.path.join(GAME, "Gang Beasts_Data/StreamingAssets/aa/StandaloneWindows64-LocalBundles")
OUT = os.path.join(ROOT, "assets", "export")

RENDER_TYPES = {"Transform", "RectTransform", "MeshFilter", "MeshRenderer", "SkinnedMeshRenderer", "GameObject"}


def load_env(extra_pattern):
    files = sorted(glob.glob(os.path.join(AA, "core-*.bundle")))
    files.append(os.path.join(AA, "5bc3b817c341dd2d856f685e7d48240d.bundle"))  # MonoScripts (script class names)
    if extra_pattern:
        pats = [x for x in extra_pattern.split("|") if x]
        pats += [x for x in os.environ.get("GB_EXTRA_BUNDLES", "").split(",") if x]
        files += [f for f in sorted(glob.glob(os.path.join(AA, "*.bundle"))) if any(x in os.path.basename(f) for x in pats) and f not in files]
    data = os.path.dirname(os.path.dirname(os.path.dirname(AA)))
    files += [os.path.join(data, "Resources", "unity default resources"), os.path.join(data, "Resources", "unity_builtin_extra")]
    return UnityPy.load(*files)


def unity_normal_map(img):
    """Unity tangent-space normal map -> standard RGB (glTF) normal map. PC Unity packs normals as
    DXT5nm (x in alpha, y in green, red/blue unused and usually 1) or BC5 (x red, y green)."""
    import numpy as np
    from PIL import Image
    a = np.asarray(img.convert("RGBA"), dtype=np.float32) / 255.0
    # DXT5nm fills red with 1.0 (x lives in alpha). A flat map (e.g. Rooftop's Floor) has an
    # almost constant alpha too, so the red channel alone decides; reading red there made every
    # floor normal point sideways (x = 1) and killed the sun on the roof.
    dxt5nm = a[..., 0].mean() > 0.95 and a[..., 0].std() < 0.02
    x = a[..., 3] if dxt5nm else a[..., 0]
    y = a[..., 1]
    x, y = x * 2 - 1, y * 2 - 1
    z = np.sqrt(np.clip(1 - x * x - y * y, 0, 1))
    rgb = np.stack([x, y, z], -1) * 0.5 + 0.5
    return Image.fromarray((rgb * 255 + 0.5).astype(np.uint8), "RGB")


def jsonable(v):
    """Typetree dict -> JSON-safe (bytes to hex, drop huge arrays)."""
    if isinstance(v, dict):
        return {k: jsonable(x) for k, x in v.items()}
    if isinstance(v, (list, tuple)):
        return [jsonable(x) for x in v]
    if isinstance(v, (bytes, bytearray)):
        return v[:256].hex()
    if isinstance(v, float) and (v != v or v in (float("inf"), float("-inf"))):
        return None
    return v


def link_pptrs(v, file, known):
    """Tag same-file PPtrs to exported GameObjects/components with "node": the owning glTF node."""
    if isinstance(v, dict):
        if v.keys() == {"m_FileID", "m_PathID"}:
            if v["m_FileID"] == 0 and v["m_PathID"]:
                node = known.get((file, v["m_PathID"]))
                if node is not None: v["node"] = node
            return
        for x in v.values(): link_pptrs(x, file, known)
    elif isinstance(v, list):
        for x in v: link_pptrs(x, file, known)


COMBINE = ["average", "minimum", "multiply", "maximum"]  # Unity PhysicMaterialCombine order


def physic_material(ptr):
    """Resolved PhysicMaterial, or None for Unity's default (0.6/0.6/0, average/average)."""
    if not ptr or not ptr.m_PathID: return None
    try:
        m = ptr.read()
    except Exception:
        return None  # PhysicMaterial lives in a bundle that is not loaded: use Unity's default
    return {"name": m.m_Name, "dynamic_friction": m.dynamicFriction, "static_friction": m.staticFriction,
            "bounciness": m.bounciness, "friction_combine": COMBINE[m.frictionCombine],
            "bounce_combine": COMBINE[m.bounceCombine]}


def export_settings(name):
    """Project-wide physics/time settings from globalgamemanagers -> <name>.json."""
    data = os.path.dirname(os.path.dirname(os.path.dirname(AA)))
    env = UnityPy.load(os.path.join(data, "globalgamemanagers"))
    found = {obj.type.name: obj.read_typetree() for obj in env.objects
             if obj.type.name in ("PhysicsManager", "TimeManager", "TagManager")}
    pm, tm, tags = found["PhysicsManager"], found["TimeManager"], found["TagManager"]
    matrix = pm["m_LayerCollisionMatrix"]
    out = {"version": 1,
           "gravity": [pm["m_Gravity"][k] for k in "xyz"],  # Unity space; engine mirrors x
           "fixed_timestep": tm["Fixed Timestep"],
           "maximum_timestep": tm["Maximum Allowed Timestep"],
           "time_scale": tm["m_TimeScale"],
           "solver_iterations": pm["m_DefaultSolverIterations"],
           "solver_velocity_iterations": pm["m_DefaultSolverVelocityIterations"],
           "bounce_threshold": pm["m_BounceThreshold"],
           "sleep_threshold": pm["m_SleepThreshold"],
           "contact_offset": pm["m_DefaultContactOffset"],
           "max_depenetration_velocity": pm["m_DefaultMaxDepenetrationVelocity"],
           "max_angular_speed": pm["m_DefaultMaxAngularSpeed"],
           "adaptive_force": bool(pm["m_EnableAdaptiveForce"]),
           "enhanced_determinism": bool(pm["m_EnableEnhancedDeterminism"]),
           "improved_patch_friction": bool(pm["m_ImprovedPatchFriction"]),
           "friction_type": ["patch", "one_directional", "two_directional"][pm["m_FrictionType"]],
           "solver_type": ["pgs", "tgs"][pm["m_SolverType"]],
           "broadphase": ["sap", "mbp", "abp"][pm["m_BroadphaseType"]],
           "contacts_generation": ["legacy", "pcm"][pm["m_ContactsGeneration"]],
           # Colliders without a material use this; null = Unity built-in (0.6/0.6/0, average/average).
           "default_material": None if not pm["m_DefaultMaterial"]["m_PathID"] else pm["m_DefaultMaterial"],
           "layers": tags["layers"],
           "tags": tags["tags"],
           # collides[i][j]: layer i contacts layer j (Unity stores one bit per pair, symmetric).
           "layer_collision_matrix": [[bool(matrix[i] >> j & 1) for j in range(32)] for i in range(32)],
           "raw": {"PhysicsManager": pm, "TimeManager": tm}}
    os.makedirs(OUT, exist_ok=True)
    with open(os.path.join(OUT, name + ".json"), "w") as f:
        json.dump(jsonable(out), f, indent=1, allow_nan=False)
    print(f"wrote {name}: gravity {out['gravity']}, dt {out['fixed_timestep']}, "
          f"iterations {out['solver_iterations']}/{out['solver_velocity_iterations']}")


def export_graphics(name, scene_pattern):
    """Export scene lighting data and the active global URP volume's key overrides."""
    env = load_env(scene_pattern)
    render = next((obj.read_typetree() for obj in env.objects
                   if obj.type.name == "RenderSettings"), None)
    if render is None:
        raise ValueError(f"no RenderSettings object in scene matching {scene_pattern!r}")

    profile = None
    for obj in env.objects:
        if obj.type.name != "MonoBehaviour":
            continue
        try:
            component = obj.read()
            if component.m_Script.read().m_ClassName != "Volume":
                continue
            if not component.m_IsGlobal or not component.sharedProfile.m_PathID:
                continue
            profile = component.sharedProfile.read()
            break
        except Exception:
            continue

    def field(tree, key):
        item = tree.get(key, {})
        return {"override": bool(item.get("m_OverrideState", 0)), "value": item.get("m_Value")}

    volume = {}
    if profile:
        for ptr in profile.components:
            try:
                component = ptr.read_typetree()
            except Exception:
                continue
            component_name = component.get("m_Name")
            if component_name == "Bloom":
                volume["bloom"] = {k: field(component, k) for k in ("threshold", "intensity", "scatter")}
            elif component_name == "Tonemapping":
                volume["tonemapping"] = field(component, "mode")
            elif component_name == "ColorAdjustments":
                volume["color_adjustments"] = {
                    k: field(component, k) for k in ("postExposure", "contrast", "saturation")
                }

    out = {
        "version": 1,
        "render_settings": {
            "fog": render["m_Fog"], "fog_color": render["m_FogColor"],
            "fog_mode": render["m_FogMode"], "fog_density": render["m_FogDensity"],
            "fog_start": render["m_LinearFogStart"], "fog_end": render["m_LinearFogEnd"],
            "ambient_mode": render["m_AmbientMode"],
            "ambient_sky_color": render["m_AmbientSkyColor"],
            "ambient_equator_color": render["m_AmbientEquatorColor"],
            "ambient_ground_color": render["m_AmbientGroundColor"],
            "ambient_intensity": render["m_AmbientIntensity"],
            "ambient_probe": render.get("m_AmbientProbe"),
            "skybox_material": render.get("m_SkyboxMaterial"),
        },
        "global_volume": {"profile": getattr(profile, "m_Name", None), "overrides": volume},
    }
    lightmap_settings_obj = next((obj for obj in env.objects
                                  if obj.type.name == "LightmapSettings"), None)
    if lightmap_settings_obj:
        lightmap_settings = lightmap_settings_obj.read_typetree()
        color_images = []
        lightmap_encodings = []
        for index, lightmap in enumerate(lightmap_settings_obj.read().m_Lightmaps):
            pointer = lightmap.m_Lightmap
            if not pointer.m_PathID:
                color_images.append(None)
                lightmap_encodings.append(None)
                continue
            try:
                texture = pointer.read()
                texture_data = pointer.deref().read_typetree()
                encoding = unity_lightmap_encoding(
                    texture.m_TextureFormat, texture_data.get("m_ColorSpace", 1)
                )
                filename = f"{name}-lightmap-{index}.png"
                decode_lightmap(texture.image, encoding).save(os.path.join(OUT, filename), "PNG")
                color_images.append(filename)
                lightmap_encodings.append({
                    "texture_format": int(texture.m_TextureFormat),
                    "color_space": int(texture_data.get("m_ColorSpace", 1)),
                    **encoding,
                })
            except Exception as error:
                print(f"  lightmap {index} export failed: {error}")
                color_images.append(None)
                lightmap_encodings.append(None)
        # Shadowmask (Mixed lights): per-texel baked occlusion of each mixed light, one light per
        # RGBA channel (Light.m_BakingOutput.occlusionMaskChannel). Stored linear, no decode.
        shadowmask_images = []
        for index, lightmap in enumerate(lightmap_settings_obj.read().m_Lightmaps):
            pointer = getattr(lightmap, "m_ShadowMask", None)
            if pointer is None or not pointer.m_PathID:
                shadowmask_images.append(None)
                continue
            try:
                filename = f"{name}-shadowmask-{index}.png"
                pointer.read().image.save(os.path.join(OUT, filename), "PNG")
                shadowmask_images.append(filename)
            except Exception as error:
                print(f"  shadowmask {index} export failed: {error}")
                shadowmask_images.append(None)
        # Light probes: positions + per-probe SphericalHarmonicsL2, so dynamic objects (beasts)
        # can be lit by the baked probe volume instead of only the flat ambient.
        probes = {"positions": [], "sh": []}
        probe_pointer = lightmap_settings.get("m_LightProbes")
        if probe_pointer and probe_pointer.get("m_PathID"):
            try:
                # Several bundles can share a path_id; prefer the one from the scene bundle.
                candidates = [obj for obj in env.objects
                              if obj.path_id == probe_pointer["m_PathID"]]
                probe_obj = next(
                    (obj for obj in candidates
                     if scene_pattern in (getattr(obj.assets_file.parent, "name", "") or "")),
                    candidates[0] if candidates else None,
                )
                if probe_obj is not None:
                    probe_data = probe_obj.read_typetree()
                    # LightProbes: m_Data.m_Positions is float3 per probe; m_BakedCoefficients is
                    # one SphericalHarmonicsL2 (27 floats) per probe.
                    positions = (probe_data.get("m_Data") or {}).get("m_Positions") or []
                    print(f"  light probes: {len(positions)} positions, "
                          f"{len(probe_data.get('m_BakedCoefficients') or [])} coefficients")
                    coefficients = probe_data.get("m_BakedCoefficients") or []
                    for position in positions:
                        probes["positions"].append(
                            [position.get("x", 0.0), position.get("y", 0.0), position.get("z", 0.0)]
                        )
                    for sh in coefficients:
                        probes["sh"].append([sh.get(f"sh[{i:2}]", 0.0) for i in range(27)])
            except Exception as error:
                print(f"  light probe export failed: {error}")
        out["lightmaps"] = {
            "shadowmask_images": shadowmask_images,
            "mode": lightmap_settings.get("m_LightmapsMode"),
            "maps": lightmap_settings.get("m_Lightmaps", []),
            "color_images": color_images,
            "encodings": lightmap_encodings,
            "light_probes": lightmap_settings.get("m_LightProbes"),
            "probe_positions": probes["positions"],
            "probe_sh": probes["sh"],
        }
    os.makedirs(OUT, exist_ok=True)
    reflection_probes = []
    for obj in env.objects:
        if obj.type.name != "ReflectionProbe":
            continue
        try:
            probe = obj.read()
            # Realtime/custom probes serialize m_CustomBakedTexture; baked probes use
            # m_BakedTexture. Rooftop is the latter, and silently skipping it removed the
            # mauve local reflection that ties the floor and metalwork to the source scene.
            texture = probe.m_CustomBakedTexture
            if not texture or not texture.m_PathID:
                texture = getattr(probe, "m_BakedTexture", None)
            if not texture or not texture.m_PathID:
                continue
            cubemap = texture.read()
            payload = bytes(cubemap.get_image_data())
            filename = f"{name}-reflection-probe-{obj.path_id}.bin"
            with open(os.path.join(OUT, filename), "wb") as stream:
                stream.write(payload)
            vector = lambda value: [value.x, value.y, value.z]
            reflection_probes.append({
                "component_path_id": obj.path_id,
                "game_object_path_id": probe.m_GameObject.m_PathID,
                "mode": probe.m_Mode,
                "resolution": probe.m_Resolution,
                "importance": probe.m_Importance,
                "blend_distance": probe.m_BlendDistance,
                "intensity": getattr(probe, "m_IntensityMultiplier", 1.0),
                "box_size": vector(probe.m_BoxSize),
                "box_offset": vector(probe.m_BoxOffset),
                "cubemap_path_id": texture.m_PathID,
                "cubemap_name": cubemap.m_Name,
                "data_file": filename,
                "format": int(cubemap.m_TextureFormat),
                "width": cubemap.m_Width,
                "height": cubemap.m_Height,
                "mip_count": cubemap.m_MipCount,
                "face_count": cubemap.m_ImageCount,
                "face_mip_bytes": cubemap.m_CompleteImageSize,
                "data_bytes": len(payload),
                "data_layout": "face-major",
            })
        except Exception as error:
            print(f"  reflection probe {obj.path_id} export failed: {error}")
    out["reflection_probes"] = reflection_probes
    # RenderSettings.m_GeneratedSkyboxReflection: the baked skybox cubemap URP uses as
    # unity_SpecCube0 wherever no reflection probe covers a surface (reflections + sky specular).
    render_obj = next((obj for obj in env.objects if obj.type.name == "RenderSettings"), None)
    if render_obj is not None:
        try:
            rs = render_obj.read()
            ptr = rs.m_GeneratedSkyboxReflection
            if ptr and ptr.m_PathID:
                cubemap = ptr.read()
                payload = bytes(cubemap.get_image_data())
                filename = f"{name}-sky-reflection.bin"
                with open(os.path.join(OUT, filename), "wb") as stream:
                    stream.write(payload)
                out["sky_reflection"] = {
                    "data_file": filename, "format": int(cubemap.m_TextureFormat),
                    "width": cubemap.m_Width, "height": cubemap.m_Height,
                    "mip_count": cubemap.m_MipCount, "face_count": cubemap.m_ImageCount,
                    "face_mip_bytes": cubemap.m_CompleteImageSize, "data_bytes": len(payload),
                    "intensity": float(getattr(rs, "m_ReflectionIntensity", 1.0)),
                }
        except Exception as error:
            print(f"  sky reflection export failed: {error}")
    with open(os.path.join(OUT, name + ".json"), "w") as f:
        json.dump(jsonable(out), f, indent=1, allow_nan=False)
    print(f"wrote {name}: ambient mode {out['render_settings']['ambient_mode']}, "
          f"fog {out['render_settings']['fog']}, {len(reflection_probes)} reflection probes, "
          f"volume overrides {', '.join(volume)}")


def reader_key(reader):
    # Path IDs are only unique inside a serialized file, not across bundles.
    return (reader.assets_file.name, reader.path_id)


def object_key(ptr):
    return reader_key(ptr.deref()) if ptr and ptr.m_PathID else None


class Gltf:
    def __init__(self):
        self.nodes, self.meshes, self.skins, self.materials = [], [], [], []
        self.textures, self.images, self.accessors, self.views = [], [], [], []
        self.bin = bytearray()
        self.mat_cache, self.tex_cache, self.mesh_cache = {}, {}, {}
        self.metadata_texture_cache = {}
        self.source_mesh_cache = {}

    def view(self, data, target=None):
        while len(self.bin) % 4: self.bin.append(0)
        v = {"buffer": 0, "byteOffset": len(self.bin), "byteLength": len(data)}
        if target: v["target"] = target
        self.bin += data
        self.views.append(v)
        return len(self.views) - 1

    def accessor(self, fmt, comps, ctype, typ, rows, target=34962, minmax=False):
        flat = [c for r in rows for c in (r if isinstance(r, (list, tuple)) else (r,))]
        data = struct.pack("<%d%s" % (len(flat), fmt), *flat)
        a = {"bufferView": self.view(data, target), "componentType": ctype, "count": len(rows), "type": typ}
        if minmax:
            a["min"] = [min(r[i] for r in rows) for i in range(comps)]
            a["max"] = [max(r[i] for r in rows) for i in range(comps)]
        self.accessors.append(a)
        return len(self.accessors) - 1

    def texture(self, tex_ptr, normal=False):
        if tex_ptr is None or not tex_ptr.m_PathID: return None
        key = (object_key(tex_ptr), normal)
        if key in self.tex_cache: return self.tex_cache[key]
        try:
            tex = tex_ptr.read()
            img = tex.image
            if normal:
                img = unity_normal_map(img)
            buf = io.BytesIO(); img.save(buf, "PNG")
        except Exception as e:
            print("  tex fail", e); self.tex_cache[key] = None; return None
        self.images.append({"bufferView": self.view(bytes(buf.getvalue())), "mimeType": "image/png", "name": tex.m_Name})
        self.textures.append({"source": len(self.images) - 1})
        self.tex_cache[key] = len(self.textures) - 1
        return self.tex_cache[key]

    def vinyl_orm_texture(self, texs, flts):
        """Pack the VinylOrMetal graph's three masks into glTF ORM.

        The compiled forward pass reads metallic from _MetallicMap.r, smoothness from
        _MetallicGlossMap.a and occlusion from _OcclusionMap.r.  Packing the *final* values
        keeps the graph's non-linear `1 - texture * (0.99*smoothness)^2` roughness exactly;
        glTF's scalar roughness factor cannot express that operation.
        """
        names = ("_OcclusionMap", "_MetallicGlossMap", "_MetallicMap")
        ptrs = [texs.get(name).m_Texture if name in texs else None for name in names]
        if not any(ptr is not None and ptr.m_PathID for ptr in ptrs):
            return None
        key = ("vinyl_orm", tuple(object_key(ptr) if ptr is not None and ptr.m_PathID else None
                                  for ptr in ptrs), float(flts.get("_OcclusionStrength", 1.0)),
               float(flts.get("_Smoothness", 0.0)), float(flts.get("_Metallic", 0.0)))
        if key in self.tex_cache:
            return self.tex_cache[key]
        try:
            from PIL import Image
            loaded = [ptr.read().image.convert("RGBA") if ptr is not None and ptr.m_PathID else None
                      for ptr in ptrs]
            size = max((img.size for img in loaded if img is not None), key=lambda s: s[0] * s[1])
            arrays = []
            for img in loaded:
                if img is None:
                    arrays.append(None)
                else:
                    if img.size != size:
                        img = img.resize(size, Image.Resampling.BILINEAR)
                    arrays.append(np.asarray(img, dtype=np.float32) / 255.0)
            occlusion = np.ones(size[::-1], dtype=np.float32) if arrays[0] is None else arrays[0][..., 0]
            smooth = np.ones(size[::-1], dtype=np.float32) if arrays[1] is None else arrays[1][..., 3]
            metallic = np.ones(size[::-1], dtype=np.float32) if arrays[2] is None else arrays[2][..., 0]
            occlusion = 1.0 + (occlusion - 1.0) * float(flts.get("_OcclusionStrength", 1.0))
            roughness = 1.0 - smooth * (0.99 * float(flts.get("_Smoothness", 0.0))) ** 2
            metallic *= float(flts.get("_Metallic", 0.0))
            rgba = np.stack((occlusion, roughness, metallic, np.ones_like(occlusion)), axis=-1)
            image = Image.fromarray((np.clip(rgba, 0, 1) * 255 + 0.5).astype(np.uint8), "RGBA")
            buf = io.BytesIO(); image.save(buf, "PNG")
            self.images.append({"bufferView": self.view(bytes(buf.getvalue())), "mimeType": "image/png",
                                "name": "VinylOrMetal ORM"})
            self.textures.append({"source": len(self.images) - 1})
            self.tex_cache[key] = len(self.textures) - 1
        except Exception as error:
            print("  vinyl ORM fail", error); self.tex_cache[key] = None
        return self.tex_cache[key]

    def material(self, mat_ptr):  # noqa: C901
        key = object_key(mat_ptr)
        if key in self.mat_cache: return self.mat_cache[key]
        m = {"name": "default", "pbrMetallicRoughness": {"metallicFactor": 0.0, "roughnessFactor": 0.8}}
        extras = {}
        try:
            mat = mat_ptr.read()
            m["name"] = mat.m_Name
            sp = mat.m_SavedProperties
            texs = {k: v for k, v in sp.m_TexEnvs}
            cols = {k: v for k, v in sp.m_Colors}
            flts = {k: v for k, v in sp.m_Floats}
            try: extras["shader"] = mat.m_Shader.read().m_ParsedForm.m_Name
            except Exception: pass
            extras["unity_material"] = material_metadata(mat, self.metadata_texture_cache)
            extras["floats"] = flts
            extras["colors"] = {k: [c.r, c.g, c.b, c.a] for k, c in cols.items()}
            if "_EmissionColor" in cols:
                emission = cols["_EmissionColor"]
                rgb = [emission.r, emission.g, emission.b]
                if any(v > 0 for v in rgb):
                    strength = max(1.0, *rgb)
                    m["emissiveFactor"] = [v / strength for v in rgb]
                    if strength > 1.0:
                        m.setdefault("extensions", {})["KHR_materials_emissive_strength"] = {"emissiveStrength": strength}
                    if "_EmissionMap" in texs and texs["_EmissionMap"].m_Texture.m_PathID:
                        emission_tex = self.texture(texs["_EmissionMap"].m_Texture)
                        if emission_tex is not None:
                            m["emissiveTexture"] = {"index": emission_tex}
            # Shader Graph / custom shaders name their albedo differently: known names first, then the
            # first plain Texture2D_* slot of a Shader Graph that is not a normal map.
            base_names = ["_BaseMap", "_MainTex", "_Albedo", "_Diffuse", "_BASE_COLOR_MAP", "_DiffuseTex"]
            if str(extras.get("shader", "")).startswith("Shader Graphs/") and extras.get("shader") != "Shader Graphs/Triplanar":
                skip = set()
                base_names += [k for k in texs if k.startswith("Texture2D_") and k not in skip
                               and texs[k].m_Texture.m_PathID]
            for name in base_names:
                if name in texs and texs[name].m_Texture.m_PathID:
                    t = self.texture(texs[name].m_Texture)
                    if t is not None:
                        m["pbrMetallicRoughness"]["baseColorTexture"] = {"index": t}
                        extras["texture_prop"] = name
                        break
            # VinylOrMetal Shader Graph tiling/offset (base map; the normal map pair matches on every
            # material seen). Unity UVs are V-up, glTF V-down: offset_v = 1 - scale_v - offset_v.
            tiling = extras["colors"].get("Vector2_0cc7e2872297450e88cdba7bec667802")
            offset = extras["colors"].get("Vector2_548d1e46deb54e63a95a3c904e1285a2")
            if tiling and offset and (tiling[:2] != [1.0, 1.0] or offset[:2] != [0.0, 0.0])                     and "baseColorTexture" in m["pbrMetallicRoughness"]:
                xf = {"scale": [tiling[0], tiling[1]], "offset": [offset[0], 1.0 - tiling[1] - offset[1]]}
                m["pbrMetallicRoughness"]["baseColorTexture"]["extensions"] = {"KHR_texture_transform": xf}
                self.uses_texture_transform = True
            normal_prop = next((n for n in ("_BumpMap", "_NormalMap") if n in texs and texs[n].m_Texture.m_PathID), None)
            if normal_prop:
                t = self.texture(texs[normal_prop].m_Texture, normal=True)
                if t is not None:
                    m["normalTexture"] = {"index": t, "scale": float(flts.get("_BumpScale", 1.0))}
            elif (extras.get("shader") == "Shader Graphs/Triplanar"
                  and "Texture2D_4B06A00B" in texs
                  and texs["Texture2D_4B06A00B"].m_Texture.m_PathID):
                t = self.texture(texs["Texture2D_4B06A00B"].m_Texture, normal=True)
                if t is not None:
                    m["normalTexture"] = {"index": t, "scale": 1.0}
                    extras["triplanar_normal_prop"] = "Texture2D_4B06A00B"
            for name in ("_BaseColor", "_Color"):
                if name in cols:
                    c = cols[name]; m["pbrMetallicRoughness"]["baseColorFactor"] = [c.r, c.g, c.b, c.a]; break
            if "_Smoothness" in flts: m["pbrMetallicRoughness"]["roughnessFactor"] = 1.0 - flts["_Smoothness"]
            if "_Metallic" in flts: m["pbrMetallicRoughness"]["metallicFactor"] = flts["_Metallic"]
            if extras.get("shader", "").startswith("GangBeasts/Surface/VinylOrMetal"):
                orm = self.vinyl_orm_texture(texs, flts)
                if orm is not None:
                    m["pbrMetallicRoughness"]["metallicRoughnessTexture"] = {"index": orm}
                    m["pbrMetallicRoughness"]["roughnessFactor"] = 1.0
                    m["pbrMetallicRoughness"]["metallicFactor"] = 1.0
                    m["occlusionTexture"] = {"index": orm}
                    extras["vinyl_orm_packed"] = True
            if extras.get("shader", "").endswith("AlphaClipped"):
                m["alphaMode"] = "MASK"
                m["alphaCutoff"] = float(flts.get("_Cutoff", 0.5))
            elif (getattr(mat, "m_CustomRenderQueue", -1) >= 3000 or flts.get("_Surface") == 1.0)                     and m["pbrMetallicRoughness"].get("baseColorFactor", [1, 1, 1, 1])[3] < 1.0:
                # Only Unity-transparent materials blend; opaque shaders ignore colour alpha
                # (Windows_ParallaxMapping / Wall_crazynormals carry alpha 0 but draw opaque).
                m["alphaMode"] = "BLEND"
        except Exception as e:
            print("  mat fail", e)
        m["extras"] = extras
        self.materials.append(m)
        self.mat_cache[key] = len(self.materials) - 1
        return self.mat_cache[key]

    def mesh(self, mesh_ptr, mat_ptrs, skinned, submeshes=None, to_local=None):
        key = (object_key(mesh_ptr), tuple(object_key(p) for p in mat_ptrs), skinned,
               tuple(submeshes) if submeshes is not None else None,
               tuple(to_local.flatten()) if to_local is not None else None)
        if key in self.mesh_cache: return self.mesh_cache[key]
        source_key = object_key(mesh_ptr)
        if source_key not in self.source_mesh_cache:
            mesh = mesh_ptr.read()
            h = MeshHandler(mesh); h.process()
            self.source_mesh_cache[source_key] = (mesh, h)
        mesh, h = self.source_mesh_cache[source_key]
        data = geometry(h, submeshes, to_local, len(mesh.m_BindPose or []) if skinned else 0)
        if data is None:
            self.mesh_cache[key] = (None, None); return self.mesh_cache[key]
        values, groups = data
        attrs = {}
        for name, rows in values.items():
            comps = len(rows[0])
            integer = name == "JOINTS_0"
            attrs[name] = self.accessor("H" if integer else "f", comps, 5123 if integer else 5126,
                                       "VEC" + str(comps), rows, minmax=name == "POSITION")
        prims = []
        for i, tris in enumerate(groups):
            if not tris: continue
            p = {"attributes": attrs, "indices": self.accessor("I", 1, 5125, "SCALAR", [x for t in tris for x in t], target=34963)}
            if mat_ptrs:
                # Unity draws extra material slots onto the last submesh; a null slot draws
                # nothing, so use the first real material among this submesh's slots.
                slots = [mat_ptrs[min(i, len(mat_ptrs)-1)]]
                if i == len(groups) - 1:
                    slots += mat_ptrs[i + 1:]
                real = [m for m in slots if m.m_PathID]
                p["material"] = self.material(real[0] if real else slots[0])
            prims.append(p)
        self.meshes.append({"name": mesh.m_Name, "primitives": prims})
        bindposes = None
        if skinned and mesh.m_BindPose and "JOINTS_0" in attrs:
            bindposes = []
            for bp in mesh.m_BindPose:
                M = [[bp.e00, bp.e01, bp.e02, bp.e03], [bp.e10, bp.e11, bp.e12, bp.e13],
                     [bp.e20, bp.e21, bp.e22, bp.e23], [bp.e30, bp.e31, bp.e32, bp.e33]]
                S = [-1, 1, 1, 1]
                bindposes.append(tuple(M[r][c] * S[r] * S[c] for c in range(4) for r in range(4)))
        self.mesh_cache[key] = (len(self.meshes)-1, bindposes)
        return self.mesh_cache[key]

    def write(self, path, scene_roots, extras=None):
        while len(self.bin) % 4: self.bin.append(0)
        doc = {"asset": {"version": "2.0", "generator": "gb-rust export.py"}, "scene": 0,
               "scenes": [{"nodes": scene_roots, **({"extras": extras} if extras else {})}], "nodes": self.nodes,
               "buffers": [{"byteLength": len(self.bin)}], "bufferViews": self.views, "accessors": self.accessors}
        for k in ("meshes", "skins", "materials", "textures", "images"):
            if getattr(self, k): doc[k] = getattr(self, k)
        if self.textures: doc["samplers"] = [{}]; [t.setdefault("sampler", 0) for t in self.textures]
        extensions = []
        if getattr(self, "uses_texture_transform", False):
            extensions.append("KHR_texture_transform")
        if any("KHR_materials_emissive_strength" in m.get("extensions", {}) for m in self.materials):
            extensions.append("KHR_materials_emissive_strength")
        if extensions:
            doc["extensionsUsed"] = extensions
        js = json.dumps(jsonable(doc), separators=(",", ":"), allow_nan=False).encode()
        js += b" " * (-len(js) % 4)
        with open(path, "wb") as f:
            f.write(struct.pack("<III", 0x46546C67, 2, 12 + 8 + len(js) + 8 + len(self.bin)))
            f.write(struct.pack("<II", len(js), 0x4E4F534A)); f.write(js)
            f.write(struct.pack("<II", len(self.bin), 0x004E4942)); f.write(self.bin)


def has_network_identity(go):
    """A NetworkServer-spawned scene object (critter pools stay off: CritterEscalationManager
    releases those birds over time)."""
    names = set()
    for c in go.m_Components:
        ptr = c.component if hasattr(c, "component") else c[1] if isinstance(c, tuple) else c
        if ptr.type.name != "MonoBehaviour":
            continue
        try:
            names.add(ptr.read().m_Script.read().m_ClassName)
        except Exception:
            pass
    return "NetworkIdentity" in names and "BirdActor" not in names


def export(roots, name, prefab=False, include_inactive_prefix=None, inactive_rest_y=None):
    """Export visible geometry and retain all physics nodes with resolvable identities.

    GB_INCLUDE_INACTIVE_PREFIX can retain a source-controlled inactive visual while preserving
    its inactive physics state. GB_INACTIVE_REST_Y optionally places its direct child root at
    the source controller's rest height for a render-only preview.
    """
    g = Gltf()
    sidecar = {"version": 3, "kind": "prefab" if prefab else "scene", "nodes": [], "collision_meshes": []}
    collision_cache = {}
    render_prefixes = tuple(
        prefix.strip()
        for prefix in (include_inactive_prefix or "").split(";")
        if prefix.strip()
    )

    def collision_mesh(ptr):
        """Raw Unity-space mesh for a MeshCollider (PhysX cooks it); index into collision_meshes."""
        if not ptr or not ptr.m_PathID: return None
        key = object_key(ptr)
        if key not in collision_cache:
            mesh = ptr.read()
            h = MeshHandler(mesh); h.process()
            tris = [int(i) for group in h.get_triangles() for tri in group for i in tri]
            sidecar["collision_meshes"].append({"name": mesh.m_Name,
                "vertices": [round(float(c), 6) for v in h.m_Vertices for c in v[:3]], "triangles": tris})
            collision_cache[key] = len(sidecar["collision_meshes"]) - 1
        return collision_cache[key]
    tf_to_node, component_to_node, world_cache = {}, {}, {}
    pending_skins, pending_joints = [], []

    def comp_list(go):
        return [c.component if hasattr(c, "component") else c[1] for c in go.m_Component]

    def world(tf):
        key = reader_key(tf.object_reader)
        if key not in world_cache:
            p, r, s = tf.m_LocalPosition, tf.m_LocalRotation, tf.m_LocalScale
            m = trs([p.x, p.y, p.z], [r.x, r.y, r.z, r.w], [s.x, s.y, s.z])
            if tf.m_Father.m_PathID: m = world(tf.m_Father.read()) @ m
            world_cache[key] = m
        return world_cache[key]

    def visit(tf, path, parent_active=True, parent=None):
        go = tf.m_GameObject.read()
        idx = len(g.nodes)
        tf_to_node[reader_key(tf.object_reader)] = idx
        component_to_node[reader_key(go.object_reader)] = idx
        p, r, s = tf.m_LocalPosition, tf.m_LocalRotation, tf.m_LocalScale
        node = {"name": go.m_Name, "translation": [-p.x, p.y, p.z],
                "rotation": [r.x, -r.y, -r.z, r.w], "scale": [s.x, s.y, s.z],
                "extras": {"gb_node": idx}}
        if prefab and parent is None:
            node["translation"] = [0.0, 0.0, 0.0]
            node["rotation"] = [0.0, 0.0, 0.0, 1.0]
        g.nodes.append(node)
        path = f"{path}/{go.m_Name}" if path else go.m_Name
        # UNet NetworkServer.SpawnObjects() activates every scene object with a NetworkIdentity
        # when the match starts (the rooftop billboards and CCTV camera are saved inactive).
        active = parent_active and (bool(go.m_IsActive) or (not prefab and has_network_identity(go)))
        info = {"path": path, "parent": parent, "layer": go.m_Layer, "active": bool(go.m_IsActive),
                "active_in_hierarchy": active, "tag": go.m_Tag, "components": [],
                "transform": {k: node[k] for k in ("translation", "rotation", "scale")},
                "unity_transform": {"file": reader_key(tf.object_reader)[0], "path_id": str(tf.object_reader.path_id)}}
        if tf.object_reader.type.name == "RectTransform":
            v2 = lambda p: [p.x, p.y]
            info["rect"] = {"anchor_min": v2(tf.m_AnchorMin), "anchor_max": v2(tf.m_AnchorMax),
                            "anchored_position": v2(tf.m_AnchoredPosition), "size_delta": v2(tf.m_SizeDelta),
                            "pivot": v2(tf.m_Pivot)}
        sidecar["nodes"].append(info)
        mesh_ptr = None; mats = []; skinned = None; renderer = None
        for cptr in comp_list(go):
            c = cptr.deref()
            component_to_node[reader_key(c)] = idx
            t = c.type.name
            if t == "MeshFilter":
                mesh_ptr = c.read().m_Mesh
            elif t == "MeshRenderer":
                renderer = c.read(); mats = list(renderer.m_Materials)
                info["renderer"] = {
                    "enabled": bool(renderer.m_Enabled),
                    # ShadowCastingMode: 0 Off, 1 On, 2 TwoSided, 3 ShadowsOnly.
                    "cast_shadows": int(renderer.m_CastShadows),
                    "receive_shadows": int(renderer.m_ReceiveShadows),
                    "lightmap_index": int(renderer.m_LightmapIndex),
                    "lightmap_scale_offset": [
                        renderer.m_LightmapTilingOffset.x, renderer.m_LightmapTilingOffset.y,
                        renderer.m_LightmapTilingOffset.z, renderer.m_LightmapTilingOffset.w,
                    ],
                }
            elif t == "SkinnedMeshRenderer":
                skinned = c.read(); mats = list(skinned.m_Materials)
                info["renderer"] = {
                    "enabled": bool(skinned.m_Enabled), "skinned": True,
                    "cast_shadows": int(skinned.m_CastShadows),
                    "receive_shadows": int(skinned.m_ReceiveShadows),
                    "lightmap_index": int(skinned.m_LightmapIndex),
                    "lightmap_scale_offset": [
                        skinned.m_LightmapTilingOffset.x, skinned.m_LightmapTilingOffset.y,
                        skinned.m_LightmapTilingOffset.z, skinned.m_LightmapTilingOffset.w,
                    ],
                }
            if t in RENDER_TYPES: continue
            tt = c.read_typetree()
            entry = {"type": t, "data": jsonable(tt),
                     "unity_id": {"file": reader_key(c)[0], "path_id": str(c.path_id)}}
            if t == "MonoBehaviour":
                try: entry["script"] = c.read().m_Script.read().m_ClassName
                except Exception: entry["script"] = None
            if t.endswith("Collider"):
                entry["material"] = physic_material(c.read().m_Material)
            if t == "MeshCollider":
                try:
                    entry["collision_mesh"] = collision_mesh(c.read().m_Mesh)
                except FileNotFoundError as e:  # mesh lives in a CAB the shipped bundles don't include
                    print("skip missing collision mesh:", path, str(e)[:50]); entry["collision_mesh"] = None
            if t.endswith("Joint"):
                body = c.read().m_ConnectedBody
                pending_joints.append((entry, object_key(body) if body.m_PathID else None))
            info["components"].append(entry)
        renderer_enabled = info.get("renderer", {}).get("enabled", False)
        render_override = bool(
            not active and any(path.startswith(prefix) for prefix in render_prefixes)
            and renderer_enabled
        )
        if render_override:
            info["render_override"] = True
            if (inactive_rest_y is not None and parent is not None
                    and sidecar["nodes"][parent]["path"] == "Tentacles"):
                node["translation"][1] = inactive_rest_y
                info["transform"]["translation"][1] = inactive_rest_y
        visible = (active or render_override) and renderer_enabled
        if visible and skinned is not None and skinned.m_Mesh.m_PathID:
            try:
                mi, bps = g.mesh(skinned.m_Mesh, mats, True)
            except (FileNotFoundError, ValueError) as e:  # mesh lives in a CAB the shipped bundles don't include
                print("skip missing skinned mesh:", path, str(e)[:60]); mi, bps = None, None
            if mi is not None:
                node["mesh"] = mi
                pending_skins.append((idx, list(skinned.m_Bones), bps))
        elif visible and mesh_ptr is not None and mesh_ptr.m_PathID:
            batch = renderer.m_StaticBatchInfo if renderer else None
            count = batch.subMeshCount if batch else 0
            selection, to_local = None, None
            if count:
                selection = range(batch.firstSubMesh, batch.firstSubMesh+count)
                root = renderer.m_StaticBatchRoot
                batch_world = world(root.read()) if root and root.m_PathID else np.eye(4)
                to_local = np.linalg.inv(world(tf)) @ batch_world
                info["static_batch"] = {"first_submesh": batch.firstSubMesh, "submesh_count": count}
            try:
                mi, _ = g.mesh(mesh_ptr, mats, False, selection, to_local)
            except FileNotFoundError as e:
                print("skip missing mesh:", path, str(e)[:60]); mi = None
            if mi is not None: node["mesh"] = mi
        elif "renderer" in info:
            info["hidden_mesh"] = True
        kids = [visit(ch.read(), path, active, idx) for ch in tf.m_Children]
        if kids: node["children"] = kids
        return idx

    root_nodes = [visit(t, "") for t in roots]
    for node_idx, bones, bps in pending_skins:
        joints = [tf_to_node.get(object_key(b)) for b in bones]
        if not bps:
            continue  # Blend-shape-only renderer, no skeleton.
        if not joints or None in joints or len(joints) != len(bps):
            raise ValueError(f"unresolved skin bones on {sidecar['nodes'][node_idx]['path']}")
        ibm = g.accessor("f", 16, 5126, "MAT4", bps, target=None)
        g.skins.append({"joints": joints, "inverseBindMatrices": ibm})
        g.nodes[node_idx]["skin"] = len(g.skins)-1
    for entry, body in pending_joints:
        entry["connected_node"] = component_to_node.get(body) if body else None
        if body and entry["connected_node"] is None:
            raise ValueError(f"unresolved connected rigidbody: {body}")
    for info in sidecar["nodes"]:
        for entry in info["components"]:
            link_pptrs(entry["data"], entry["unity_id"]["file"], component_to_node)
    os.makedirs(OUT, exist_ok=True)
    g.write(os.path.join(OUT, name + ".glb"), root_nodes)
    with open(os.path.join(OUT, name + ".json"), "w") as f:
        json.dump(jsonable(sidecar), f, separators=(",", ":"), allow_nan=False)
    print(f"wrote {name}: {len(g.nodes)} nodes, {len(g.meshes)} meshes, {len(g.materials)} mats, {len(g.textures)} textures, {len(g.skins)} skins")


def export_prefab(env, key, name):
    """Export a GameObject prefab selected from an already loaded UnityPy environment."""
    for path, ptr in env.container.items():
        if key not in path or ptr.type.name != "GameObject":
            continue
        go = ptr.read()
        tf = [
            c.component if hasattr(c, "component") else c[1]
            for c in go.m_Component
            if (c.component if hasattr(c, "component") else c[1]).type.name
            in ("Transform", "RectTransform")
        ]
        if tf:
            return export([tf[0].read()], name, prefab=True)
    sys.exit("prefab not found: " + key)


def export_script_asset(env, script_class, name):
    """Export a MonoBehaviour/ScriptableObject typetree selected by its Unity script class."""
    for obj in env.objects:
        if obj.type.name != "MonoBehaviour":
            continue
        try:
            component = obj.read()
            if component.m_Script.read().m_ClassName != script_class:
                continue
            data = jsonable(obj.read_typetree())
        except Exception:
            continue
        os.makedirs(OUT, exist_ok=True)
        path = os.path.join(OUT, name + ".json")
        with open(path, "w", encoding="utf-8") as stream:
            json.dump(data, stream, indent=2, allow_nan=False)
            stream.write("\n")
        print(f"wrote {path}: {script_class}")
        return
    sys.exit("script asset not found: " + script_class)


def main():
    mode, name = sys.argv[1], sys.argv[-1]
    key = sys.argv[2] if len(sys.argv) > 3 else None
    if mode == "settings":
        return export_settings(name)
    if mode == "graphics":
        return export_graphics(name, key)
    if mode == "prefab":
        env = load_env(None)
        return export_prefab(env, key, name)
    if mode == "costume-prefab":
        if len(sys.argv) < 5:
            sys.exit(
                'usage: export.py costume-prefab <bundle-name-fragment> '
                '<asset-path-fragment> <output-name>'
            )
        bundle_pattern, asset_key, output_name = sys.argv[2:5]
        return export_prefab(load_env(bundle_pattern), asset_key, output_name)
    if mode == "costume-data":
        if len(sys.argv) < 5:
            sys.exit(
                "usage: export.py costume-data <bundle-name-fragment> "
                "<script-class> <output-name>"
            )
        bundle_pattern, script_class, output_name = sys.argv[2:5]
        return export_script_asset(load_env(bundle_pattern), script_class, output_name)
    elif mode == "scene":
        env = load_env(key)
        roots = []
        for obj in env.objects:
            bundle = getattr(obj.assets_file.parent, "name", "") or ""
            # Only the scene file itself (bundles named *_scenes*, not their .sharedAssets). Content
            # bundles matching the key hold source model prefabs (sceneBlockout, props) that the
            # scene already instances - exporting them too drew every wall twice.
            if obj.type.name == "Transform" and key in bundle and "_scenes" in bundle                     and not obj.assets_file.name.endswith(".sharedAssets"):
                tf = obj.read()
                if not tf.m_Father.m_PathID: roots.append(tf)
        print("scene roots:", len(roots))
        render_prefix = os.environ.get("GB_INCLUDE_INACTIVE_PREFIX")
        rest_y = os.environ.get("GB_INACTIVE_REST_Y")
        if name == "aquarium":
            # TentacleMechanics references roots 0–5, 7, and 8 (root 6 is unused).
            # Show their inactive meshes at the source idle height; the sidecar keeps them
            # inactive so the physics world still excludes these render-only previews.
            render_prefix = render_prefix or ";".join(
                f"Tentacles/tentacle ({i})" for i in (*range(6), 7, 8)
            )
            if rest_y is None:
                rest_y = "-4.0"
        export(
            roots,
            name,
            include_inactive_prefix=render_prefix,
            inactive_rest_y=float(rest_y) if rest_y is not None else None,
        )


if __name__ == "__main__":
    main()

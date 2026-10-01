"""Export Unity UI prefabs (uGUI + TextMeshPro) for the Bevy UI port.

    python export_ui.py <container substring> <name>     e.g. "Overlay/ActorNameBar.prefab" ui-namebar

Writes assets/export/ui/<name>.json:
  nodes: [{name, path, parent, active, rect: {anchor_min, anchor_max, anchored_position,
           size_delta, pivot, scale, rotation}, components: [{type, script, data}]}]
Cross-file references in component data get a "ref" entry: {type, name} plus, for exported
media, "file" (sprites/textures as PNG with sprite rect/border, TMP font assets as the source
font file under assets/export/ui/fonts).
"""
import json
import os
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import export  # noqa: E402  (shared loader + paths)
from UnityPy.classes import PPtr  # noqa: E402

OUT = os.path.join(export.OUT, "ui")
MEDIA = os.path.join(OUT, "media")
FONTS = os.path.join(OUT, "fonts")


def safe(name):
    return "".join(c if c.isalnum() or c in "-_ ." else "_" for c in name).strip() or "unnamed"


class Exporter:
    def __init__(self, env):
        self.env = env
        self.done = {}

    def deref(self, reader, file_id, path_id):
        if not path_id:
            return None
        try:
            p = PPtr(m_FileID=file_id, m_PathID=path_id, assetsfile=reader.assets_file)
            return p.deref()
        except Exception:
            try:
                p = PPtr(file_id, path_id)
                p.assetsfile = reader.assets_file
                return p.deref()
            except Exception:
                return None

    def media(self, target):
        key = (target.assets_file.name, target.path_id)
        if key in self.done:
            return self.done[key]
        info = {"type": target.type.name}
        try:
            obj = target.read()
            info["name"] = getattr(obj, "m_Name", "")
            if target.type.name == "Sprite":
                os.makedirs(MEDIA, exist_ok=True)
                f = safe(info["name"]) + f"-{target.path_id & 0xffffffff:08x}.png"
                obj.image.save(os.path.join(MEDIA, f))
                r = obj.m_Rect
                info.update(file="media/" + f, rect=[r.x, r.y, r.width, r.height],
                            border=[obj.m_Border.x, obj.m_Border.y, obj.m_Border.z, obj.m_Border.w],
                            pixels_per_unit=obj.m_PixelsToUnits)
            elif target.type.name == "Texture2D":
                os.makedirs(MEDIA, exist_ok=True)
                f = safe(info["name"]) + f"-{target.path_id & 0xffffffff:08x}.png"
                obj.image.save(os.path.join(MEDIA, f))
                info.update(file="media/" + f, size=[obj.m_Width, obj.m_Height])
            elif target.type.name == "Font":
                os.makedirs(FONTS, exist_ok=True)
                data = bytes(obj.m_FontData)
                ext = ".otf" if data[:4] == b"OTTO" else ".ttf"
                f = safe(info["name"]) + ext
                with open(os.path.join(FONTS, f), "wb") as fh:
                    fh.write(data)
                info.update(file="fonts/" + f)
            elif target.type.name == "MonoBehaviour":
                t = target.read_typetree()
                info["name"] = t.get("m_Name", "")
                try:
                    info["script"] = obj.m_Script.read().m_ClassName
                except Exception:
                    pass
                # TMP_FontAsset: the source font file it was generated from.
                src = t.get("m_SourceFontFile") or t.get("sourceFontFile")
                if isinstance(src, dict) and src.get("m_PathID"):
                    font = self.deref(target, src["m_FileID"], src["m_PathID"])
                    if font is not None:
                        info["source_font"] = self.media(font)
                fi = t.get("m_FaceInfo") or {}
                if fi:
                    info["face"] = {k: fi.get(k) for k in ("m_FamilyName", "m_StyleName", "m_PointSize", "m_Scale", "m_LineHeight", "m_AscentLine", "m_DescentLine")}
                # TMP material: SDF outline/underlay/face settings.
            elif target.type.name == "Material":
                floats = {k: v for k, v in obj.m_SavedProperties.m_Floats}
                cols = {k: [c.r, c.g, c.b, c.a] for k, c in obj.m_SavedProperties.m_Colors}
                keep = ("_OutlineWidth", "_OutlineSoftness", "_FaceDilate", "_UnderlayOffsetX", "_UnderlayOffsetY", "_UnderlayDilate", "_UnderlaySoftness", "_GlowPower")
                info["floats"] = {k: floats[k] for k in keep if k in floats}
                info["colors"] = {k: cols[k] for k in ("_FaceColor", "_OutlineColor", "_UnderlayColor", "_GlowColor", "_Color") if k in cols}
                try:
                    info["shader"] = obj.m_Shader.read().m_ParsedForm.m_Name
                except Exception:
                    pass
        except Exception as e:
            info["error"] = str(e)[:200]
        self.done[key] = info
        return info

    def resolve(self, v, reader):
        """Annotate PPtrs in component data with what they point at."""
        if isinstance(v, dict):
            if "m_FileID" in v and "m_PathID" in v and len(v) <= 3:
                if v["m_PathID"]:
                    target = self.deref(reader, v["m_FileID"], v["m_PathID"])
                    if target is not None and target.type.name in ("Sprite", "Texture2D", "Font", "Material", "MonoBehaviour") and (v["m_FileID"] != 0 or target.type.name != "MonoBehaviour"):
                        v["ref"] = self.media(target)
                return
            for x in v.values():
                self.resolve(x, reader)
        elif isinstance(v, list):
            for x in v:
                self.resolve(x, reader)

    def export(self, root_tf, name):
        nodes = []
        tf_index = {}

        def comp_ptr(c):
            return c.component if hasattr(c, "component") else c[1] if isinstance(c, tuple) else c

        def visit(tf, parent, path):
            go = tf.m_GameObject.read()
            idx = len(nodes)
            tf_index[tf.object_reader.path_id] = idx
            path = f"{path}/{go.m_Name}" if path else go.m_Name
            node = {"name": go.m_Name, "path": path, "parent": parent, "active": bool(go.m_IsActive),
                    "layer": go.m_Layer, "components": []}
            if tf.object_reader.type.name == "RectTransform":
                v2 = lambda p: [p.x, p.y]
                node["rect"] = {"anchor_min": v2(tf.m_AnchorMin), "anchor_max": v2(tf.m_AnchorMax),
                                "anchored_position": v2(tf.m_AnchoredPosition), "size_delta": v2(tf.m_SizeDelta),
                                "pivot": v2(tf.m_Pivot)}
            s, r, p = tf.m_LocalScale, tf.m_LocalRotation, tf.m_LocalPosition
            node["scale"] = [s.x, s.y, s.z]
            node["rotation"] = [r.x, r.y, r.z, r.w]
            node["position"] = [p.x, p.y, p.z]
            nodes.append(node)
            for c in go.m_Components:
                ptr = comp_ptr(c)
                t = ptr.type.name
                if t in ("Transform", "RectTransform"):
                    continue
                entry = {"type": t}
                try:
                    reader = ptr.deref() if hasattr(ptr, "deref") else ptr
                    data = reader.read_typetree()
                    if t == "MonoBehaviour":
                        try:
                            entry["script"] = reader.read().m_Script.read().m_ClassName
                        except Exception:
                            pass
                    for k in ("m_GameObject", "m_Script", "m_EditorHideFlags", "m_EditorClassIdentifier", "m_Name"):
                        data.pop(k, None)
                    self.resolve(data, reader)
                    entry["data"] = data
                except Exception as e:
                    entry["error"] = str(e)[:200]
                node["components"].append(entry)
            for child in tf.m_Children:
                visit(child.read(), idx, path)

        visit(root_tf, None, "")
        # Same-file references to nodes (e.g. NameBarHandler.CachedNameText) -> node index.
        comp_nodes = {}
        for n_i, n in enumerate(nodes):
            pass
        os.makedirs(OUT, exist_ok=True)
        with open(os.path.join(OUT, name + ".json"), "w") as f:
            json.dump({"version": 1, "nodes": nodes}, f, indent=1, default=str)
        print(f"wrote ui/{name}.json: {len(nodes)} nodes, {len(self.done)} referenced assets")


def main():
    key, name = sys.argv[1], sys.argv[2]
    env = export.load_env(None)
    for path, ptr in env.container.items():
        if key in path:
            go = ptr.read()
            comps = [c.component if hasattr(c, "component") else c[1] for c in go.m_Component]
            tf = [c for c in comps if c.type.name in ("Transform", "RectTransform")][0]
            Exporter(env).export(tf.read(), name)
            return
    sys.exit("prefab not found: " + key)


if __name__ == "__main__":
    main()

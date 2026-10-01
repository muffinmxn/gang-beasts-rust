"""Export every costume piece used by the preset database to assets/export/costumes/<uid>.glb
(+ .json sidecar), and write costumes/index.json: presets -> item uids, item uid -> slot/file.
Needs costume-presets.json and costume-items.json (export.py costume-data / export_costume_items.py)."""
import json, os, sys
sys.path.insert(0, os.path.dirname(__file__))
import export as E
import addressables as A

presets = json.load(open(os.path.join(E.OUT, "costume-presets.json")))["CostumePresets"]
items = {i["uid"]: i for i in json.load(open(os.path.join(E.OUT, "costume-items.json")))}
catalog = A.load()
used = sorted({s["ItemId"] for p in presets for s in p["CostumeSaveEntry"]["CostumeSaveItemList"]})
env = E.load_env("costumes-")
by_path = {path: ptr for path, ptr in env.container.items() if ptr.type.name == "GameObject"}
os.makedirs(os.path.join(E.OUT, "costumes"), exist_ok=True)
index = {"presets": [], "items": {}}
for uid in used:
    item = items.get(uid)
    if not item or not item["assets"]:
        continue
    path = catalog.get(item["assets"][0], [{}])[0].get("path")
    ptr = by_path.get(path)
    if ptr is None:
        print("  missing prefab", uid, path)
        continue
    go = ptr.read()
    tf = next(c.component if hasattr(c, "component") else c[1] for c in go.m_Component
              if (c.component if hasattr(c, "component") else c[1]).type.name == "Transform")
    try:
        E.export([tf.read()], f"costumes/{uid}", prefab=True)
    except Exception as e:
        print("  export failed", uid, path, e)
        continue
    index["items"][str(uid)] = {"slot": item["slot"], "disable": item["disable"],
                               "name": os.path.basename(path)}
for p in presets:
    e = p["CostumeSaveEntry"]
    index["presets"].append({"name": e["Name"], "unlocked": bool(p["Unlocked"]),
                             "items": [[s["ItemId"], s["ColorId"]] for s in e["CostumeSaveItemList"]]})
json.dump(index, open(os.path.join(E.OUT, "costumes", "index.json"), "w"), indent=1)
print("exported", len(index["items"]), "items,", len(index["presets"]), "presets")

"""Export every CostumeObject (costume item: uid, slot, hidden parts, prefab path) to
assets/export/costume-items.json. Presets (costume-presets.json) reference these by `_uid`."""
import json, os, sys
sys.path.insert(0, os.path.dirname(__file__))
import export as E

env = E.load_env("costumes-catalog")
items = []
for o in env.objects:
    if o.type.name != "MonoBehaviour":
        continue
    try:
        if o.read().m_Script.read().m_ClassName != "CostumeObject":
            continue
        t = o.read_typetree()
    except Exception:
        continue
    items.append({
        "uid": t["_uid"], "name": t["PartName"], "prefab": t["PrefabPath"],
        "slot": t["PrimaryPart"], "disable": t["DisableParts"], "enabled": bool(t["Enabled"]),
        "unlocked": bool(t["Unlocked"]), "tags": t["Tags"], "sort": t["SortOrder"],
        "assets": [r.get("m_AssetGUID") for r in t["CostumeItems"]],
    })
items.sort(key=lambda i: i["uid"])
out = os.path.join(E.OUT, "costume-items.json")
with open(out, "w", encoding="utf-8") as f:
    json.dump(items, f, indent=1)
print(f"wrote {out}: {len(items)} items")

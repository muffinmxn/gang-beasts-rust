"""Export the Unity Localization English string table as {key: text} to assets/export/ui/strings_en.json."""
import json, os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import export, UnityPy

BUNDLES = {"0f09814c88c159695fef6c46507354cb.bundle": "table", "46c941530c6f45cd51b8e149487c8160.bundle": "shared"}
files = [os.path.join(export.AA, b) for b in BUNDLES] + [os.path.join(export.AA, "5bc3b817c341dd2d856f685e7d48240d.bundle")]
env = UnityPy.load(*files)
keys, texts = {}, {}
for obj in env.objects:
    if obj.type.name != "MonoBehaviour":
        continue
    try:
        t = obj.read_typetree()
    except Exception:
        continue
    if "m_Entries" in t and t.get("m_TableCollectionName") is not None and "StringTable" in t.get("m_TableCollectionName", "") and t.get("m_Name", "").startswith("StringTable"):
        for e in t["m_Entries"]:
            keys[e["m_Id"]] = e["m_Key"]
    if "m_TableData" in t and t.get("m_Name") == "StringTable_en-US":
        for e in t["m_TableData"]:
            texts[e["m_Id"]] = e["m_Localized"]
out = {keys[i]: s for i, s in texts.items() if i in keys}
os.makedirs(os.path.join(export.OUT, "ui"), exist_ok=True)
with open(os.path.join(export.OUT, "ui", "strings_en.json"), "w", encoding="utf-8") as f:
    json.dump(out, f, indent=1, ensure_ascii=False, sort_keys=True)
print(f"{len(keys)} keys, {len(texts)} en-US texts, {len(out)} exported")

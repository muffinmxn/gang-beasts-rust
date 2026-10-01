"""Export the UI font files (Font objects) from the fonts bundle to assets/export/ui/fonts."""
import os, sys
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import export

OUT = os.path.join(export.OUT, "ui", "fonts")
WANT = ("liberation", "snasm", "handgot", "handel", "pexico", "lift", "code", "notosans-black")

env = export.load_env(None)
os.makedirs(OUT, exist_ok=True)
for obj in env.objects:
    if obj.type.name != "Font":
        continue
    f = obj.read()
    name = f.m_Name
    if not any(w in name.lower() for w in WANT):
        continue
    data = bytes(f.m_FontData)
    if not data:
        continue
    ext = ".otf" if data[:4] == b"OTTO" else ".ttf"
    path = os.path.join(OUT, name + ext)
    with open(path, "wb") as fh:
        fh.write(data)
    print(f"{name}{ext}: {len(data)} bytes")

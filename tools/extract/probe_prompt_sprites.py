"""Find the UI bind-prompt sprites in the source bundles.

The exported scene JSON only carries text cues, so the button glyphs (SPACE/ESC/Xbox/PS)
have to come from the Sprite assets themselves. This scans every bundle near the menu for
Sprite / Texture2D objects whose names look like prompts and prints them, so the right ones
can be exported to assets/export/ui/media/.
"""
import glob
import os
import sys

import UnityPy

UnityPy.config.FALLBACK_UNITY_VERSION = "2021.3.33f1"

GAME = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..",
    "Base Game (Microsoft Store Version)", "Gang Beasts", "Content"))
AA = os.path.join(GAME, "Gang Beasts_Data/StreamingAssets/aa/StandaloneWindows64-LocalBundles")

KEYISH = ("space", "escape", "esc", "back", "submit", "accept", "cancel", "xbox", "ps4",
          "playstation", "button", "key", "pad", "glyph", "icon", "prompt", "binding")


def main() -> None:
    files = sorted(glob.glob(os.path.join(AA, "*.bundle")))
    print(f"{len(files)} bundles")
    found = []
    for i, path in enumerate(files):
        name = os.path.basename(path).lower()
        # The UI sprites live in the core/ui/menu bundles; skip stage geometry bundles.
        if not any(k in name for k in ("core", "menu", "ui", "sharedassets")):
            continue
        try:
            env = UnityPy.load(path)
        except Exception:
            continue
        for obj in env.objects:
            if obj.type.name not in ("Sprite", "Texture2D"):
                continue
            try:
                data = obj.read()
                obj_name = getattr(data, "m_Name", "") or ""
            except Exception:
                continue
            low = obj_name.lower()
            if any(k in low for k in KEYISH):
                found.append((os.path.basename(path), obj.type.name, obj_name))
        if i % 25 == 0:
            print(f"  scanned {i}/{len(files)}")
    print(f"--- {len(found)} candidates ---")
    for bundle, kind, name in found[:80]:
        print(f"{kind:10s} {name[:60]:60s} {bundle[:50]}")


if __name__ == "__main__":
    main()

"""Export the UI bind-prompt sprites (keyboard + gamepad glyphs) from core-ui_assets.

These are the icons the reference UI shows on the tooltips. The scene export only carries
text cues, so the glyphs have to come from the Sprite assets.
"""
import glob
import os

import UnityPy

UnityPy.config.FALLBACK_UNITY_VERSION = "2021.3.33f1"

GAME = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..",
    "Base Game (Microsoft Store Version)", "Gang Beasts", "Content"))
AA = os.path.join(GAME, "Gang Beasts_Data/StreamingAssets/aa/StandaloneWindows64-LocalBundles")
OUT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", "assets", "export", "ui", "media"))

WANTED = [
    "Keyboard_Space", "Keyboard_ESC", "Keyboard_QE", "Keyboard_M", "Keyboard_I", "Keyboard_O",
    "Keyboard_Crocs",
    "XboxOne_A", "XboxOne_B", "XboxOne_X", "XboxOne_Y", "XboxOne_View", "XboxOne_Bumpers",
    "XboxOne_RS_H",
    "PS4 - Icons- CROSS", "PS4 - Icons- CIRCLE", "PS4 - Icons- TRIANGLE", "PS4 - Icons- SQUARE",
    "PS4 - Icons - Options", "PS4 - Icons- L1 R1-12", "PS4 - Icons- R ANALOGUE", "TouchpadSprite",
]


def main() -> None:
    os.makedirs(OUT, exist_ok=True)
    bundle = next(iter(glob.glob(os.path.join(AA, "core-ui_assets_*.bundle"))), None)
    if bundle is None:
        raise SystemExit("core-ui_assets bundle not found")
    env = UnityPy.load(bundle)
    saved = 0
    for obj in env.objects:
        if obj.type.name not in ("Sprite", "Texture2D"):
            continue
        try:
            data = obj.read()
            name = getattr(data, "m_Name", "") or ""
        except Exception:
            continue
        if name not in WANTED:
            continue
        image = getattr(data, "image", None)
        if image is None:
            continue
        safe = name.replace(" ", "_").replace("-", "_")
        path = os.path.join(OUT, f"{safe}.png")
        image.save(path, "PNG")
        saved += 1
        print(f"saved {safe}.png  ({image.size[0]}x{image.size[1]})")
    print(f"{saved} sprites exported to {OUT}")


if __name__ == "__main__":
    main()

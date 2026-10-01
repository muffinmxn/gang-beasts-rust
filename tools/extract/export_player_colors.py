"""Export the installed Gang Beasts PlayerColors database for local actor rendering."""
import json
import os

from export import OUT, load_env


def player_palette(color_database):
    if color_database.get("m_Name") != "PlayerColors":
        raise ValueError("expected the shipped PlayerColors database")
    colors = []
    for entry in color_database.get("_colorObjects", []):
        swatches = entry.get("Colors", [])
        if not swatches or not entry.get("Unlocked", 0):
            continue
        color = swatches[0]
        colors.append(
            {
                "uid": int(entry["_uid"]),
                "loc_code": entry.get("LocCode", ""),
                "rgba": [float(color[k]) for k in ("r", "g", "b", "a")],
            }
        )
    if not colors:
        raise ValueError("PlayerColors has no unlocked swatches")
    return {"palette": "PlayerColors", "colors": colors}


def export_player_colors():
    env = load_env("costumesassets")
    for obj in env.objects:
        if obj.type.name != "MonoBehaviour":
            continue
        try:
            component = obj.read()
            if component.m_Script.read().m_ClassName != "ColorDatabase":
                continue
            data = obj.read_typetree()
        except Exception:
            continue
        if data.get("m_Name") != "PlayerColors":
            continue
        result = player_palette(data)
        os.makedirs(OUT, exist_ok=True)
        path = os.path.join(OUT, "player-colors.json")
        with open(path, "w", encoding="utf-8") as stream:
            json.dump(result, stream, indent=2)
            stream.write("\n")
        print(f"wrote {path}: {len(result['colors'])} unlocked player colors")
        return
    raise RuntimeError("installed costume bundle does not contain PlayerColors")


if __name__ == "__main__":
    export_player_colors()

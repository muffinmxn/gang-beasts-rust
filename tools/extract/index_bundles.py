"""Inventory every addressable bundle: object types + named meshes/prefabs/scenes -> assets/index.json"""
import json, os, sys, collections, UnityPy
UnityPy.config.FALLBACK_UNITY_VERSION = "2021.3.33f1"
ROOT = os.path.join(os.path.dirname(__file__), "..", "..")
GAME = os.environ.get("GB_GAME_DIR", os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "Base Game (Microsoft Store Version)", "Gang Beasts", "Content"))
AA = os.path.join(GAME, "Gang Beasts_Data/StreamingAssets/aa/StandaloneWindows64-LocalBundles")
out = {}
for fn in sorted(os.listdir(AA)):
    env = UnityPy.load(os.path.join(AA, fn))
    types = collections.Counter(); names = collections.defaultdict(list)
    for obj in env.objects:
        t = obj.type.name; types[t] += 1
        if t in ("Mesh", "GameObject", "Texture2D", "AudioClip", "Material"):
            try: n = obj.peek_name()
            except Exception: n = None
            if n and (t != "GameObject" or len(names[t]) < 40): names[t].append(n)
    containers = list(env.container.keys())
    out[fn] = {"size": os.path.getsize(os.path.join(AA, fn)), "types": types, "containers": containers[:200], "names": names}
    print(fn, dict(types.most_common(6)), containers[:2], flush=True)
os.makedirs(os.path.join(ROOT, "assets"), exist_ok=True)
json.dump(out, open(os.path.join(ROOT, "assets/index.json"), "w"), indent=1)

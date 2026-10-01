"""Print a costume item's node transforms (rest pose), to see how a hat/hair piece is attached.

  python tools/harness/item_nodes.py 107 89 237
"""
import json
import sys

for uid in sys.argv[1:]:
    d = json.load(open(f"assets/export/costumes/{uid}.json"))
    print(f"===== {uid}")
    for n in d["nodes"]:
        name = n["path"].rsplit("/", 1)[-1]
        t = n["transform"]
        tr = [round(v, 3) for v in t["translation"]]
        rot = [round(v, 2) for v in t["rotation"]]
        sc = [round(v, 2) for v in t["scale"]]
        comps = [c["type"] for c in n.get("components", [])]
        print(f"  {name:30s} parent {str(n.get('parent')):>4} t {tr} r {rot} s {sc} "
              f"active {n['active_in_hierarchy']} {comps}")

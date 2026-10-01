"""Show a costume preset's items and the assets each item exports.

  python tools/harness/preset_items.py "OWL CAP" ASTRONOUGHT ...
"""
import json
import sys

d = json.load(open("assets/export/costumes/index.json"))
items = {int(k): v for k, v in d["items"].items()}
names = sys.argv[1:]
for preset in d["presets"]:
    if names and preset["name"] not in names:
        continue
    print(f"== {preset['name']}  items: {preset['items']}")
    for uid, colour in preset["items"]:
        item = items.get(uid, {})
        print(f"   uid {uid:5d} slot {item.get('slot')} colour {colour} "
              f"name {item.get('name')!r} assets {item.get('assets')}")

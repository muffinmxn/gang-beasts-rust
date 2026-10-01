"""Decode the Addressables content catalog (StreamingAssets/aa/catalog.json): key -> asset path +
bundle list. Used to resolve AssetReference GUIDs (costume items) to prefabs."""
import base64, json, os, struct

CATALOG = os.path.join(os.environ.get("GB_GAME_DIR", os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "..", "Base Game (Microsoft Store Version)", "Gang Beasts", "Content")),
                       "Gang Beasts_Data/StreamingAssets/aa/catalog.json")


def _keys(data):
    out = []
    (n,) = struct.unpack_from("<i", data, 0)
    return n


def load(path=CATALOG):
    c = json.load(open(path, encoding="utf-8"))
    ids = c["m_InternalIds"]
    kd = base64.b64decode(c["m_KeyDataString"])
    bd = base64.b64decode(c["m_BucketDataString"])
    ed = base64.b64decode(c["m_EntryDataString"])

    def key_at(off):
        t = kd[off]; off += 1
        if t in (0, 1):  # ascii / unicode string with int length
            (ln,) = struct.unpack_from("<i", kd, off); off += 4
            raw = kd[off:off + ln]
            return raw.decode("ascii" if t == 0 else "utf-16-le")
        if t == 4:
            return struct.unpack_from("<i", kd, off)[0]
        return None

    (nb,) = struct.unpack_from("<i", bd, 0); o = 4
    buckets = []
    for _ in range(nb):
        koff, ne = struct.unpack_from("<ii", bd, o); o += 8
        entries = list(struct.unpack_from("<%di" % ne, bd, o)); o += 4 * ne
        buckets.append((key_at(koff), entries))
    (ne,) = struct.unpack_from("<i", ed, 0)
    entries = [struct.unpack_from("<7i", ed, 4 + 28 * i) for i in range(ne)]
    result = {}
    for key, ents in buckets:
        for e in ents:
            internal, provider, dep_key = entries[e][0], entries[e][1], entries[e][2]
            deps = []
            if dep_key >= 0:
                deps = [ids[entries[x][0]] for x in buckets[dep_key][1]]
            result.setdefault(key, []).append({"path": ids[internal], "deps": deps})
    return result


if __name__ == "__main__":
    r = load()
    print(len(r), "keys")
    for k in list(r)[:3]:
        print(k, r[k])

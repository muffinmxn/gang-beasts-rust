"""Export the game's audio from YOUR copy of the game (read-only).

  python audio.py clips            # every AudioClip in every bundle -> assets/export/audio/<name>.wav + audio-index.json
  python audio.py config           # AudioConfig / SceneAudioConfig / PhysicsAudioData / GeneralAudioData -> audio-config.json
  python audio.py stage rooftop    # per-stage emitters and ambient clips -> audio-<stage>.json
  python audio.py all              # clips + config + every stage

Outputs go to assets/export (git-ignored). The engine (`crates/gb_game/src/audio.rs`) only loads clips by name, on demand.
"""
import glob
import json
import os
import re
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import export as E  # noqa: E402
import UnityPy  # noqa: E402

OUT = os.path.join(E.OUT, "audio")
SCRIPT_CLASSES = {"AudioConfig", "SceneAudioConfig", "PhysicsAudioData", "GeneralAudioData", "SoundEffectScriptable",
                  "AudioDatabase"}


def safe(name):
    return re.sub(r"[^A-Za-z0-9 _.\-]", "_", name).strip()


def resolve(v):
    """Typetree -> JSON, replacing PPtrs to AudioClips (and other named assets) by their names."""
    if isinstance(v, dict):
        if v.keys() == {"m_FileID", "m_PathID"}:
            return None  # replaced by the caller via resolve_ptr
        return {k: resolve(x) for k, x in v.items()}
    if isinstance(v, (list, tuple)):
        return [resolve(x) for x in v]
    if isinstance(v, (bytes, bytearray)):
        return None
    if isinstance(v, float) and (v != v or v in (float("inf"), float("-inf"))):
        return None
    return v


def named(env, ptr_dict, file):
    """Name of the asset a {m_FileID, m_PathID} points at (clips, ScriptableObjects), or None."""
    if not ptr_dict.get("m_PathID"):
        return None
    try:
        pptr = UnityPy.classes.PPtr(None)  # not used: fall back to manual lookup below
    except Exception:
        pptr = None
    return None


def deref_name(obj_reader, ptr):
    """ptr is a UnityPy PPtr."""
    try:
        if not ptr or not ptr.m_PathID:
            return None
        o = ptr.read()
        return getattr(o, "m_Name", None)
    except Exception:
        return None


def walk(node, reader):
    """Typetree node + the object's parsed PPtr fields -> JSON with PPtrs resolved to names."""
    return resolve(node)


def _names(val, depth=0):
    """Recursively map PPtrs inside parsed values (objects, dicts, lists) to asset names."""
    if depth > 6:
        return None
    if hasattr(val, "m_PathID") and hasattr(val, "read"):
        return deref_name(None, val)
    if isinstance(val, (list, tuple)):
        return [_names(x, depth + 1) for x in val]
    if isinstance(val, dict):
        return {k: _names(x, depth + 1) for k, x in val.items() if not str(k).startswith("_")}
    if hasattr(val, "__dict__") and not isinstance(val, (str, bytes, int, float, bool)):
        inner = {k: _names(x, depth + 1) for k, x in vars(val).items()
                 if not k.startswith("_") and k not in ("assets_file", "object_reader", "reader")}
        return {k: x for k, x in inner.items() if x is not None and x != [] and x != {}} or None
    return None


def pptr_names(obj):
    """Return {field: name | [names] | nested dict} for every PPtr reachable from a MonoBehaviour."""
    data = obj.read()
    out = {}
    for field, val in vars(data).items():
        if field.startswith("_") or field in ("assets_file", "object_reader", "reader"):
            continue
        n = _names(val)
        if n is not None and n != [] and n != {}:
            out[field] = n
    return out


def script_name(obj):
    try:
        return obj.read().m_Script.read().m_ClassName
    except Exception:
        return None


def export_clips(env, seen, index):
    os.makedirs(OUT, exist_ok=True)
    for obj in env.objects:
        if obj.type.name != "AudioClip":
            continue
        try:
            clip = obj.read()
            name = clip.m_Name
            if name in seen:
                continue
            samples = clip.samples
        except Exception as e:
            print("skip clip", getattr(obj, "path_id", "?"), type(e).__name__)
            continue
        for sample_name, data in samples.items():
            fname = safe(name) + ".wav"
            with open(os.path.join(OUT, fname), "wb") as f:
                f.write(data)
            seen.add(name)
            index[name] = {"file": fname, "frequency": clip.m_Frequency, "channels": clip.m_Channels,
                           "length": round(float(clip.m_Length), 3)}
            break


def cmd_clips():
    files = sorted(glob.glob(os.path.join(E.AA, "*.bundle")))
    seen, index = set(), {}
    for i, f in enumerate(files):
        try:
            env = UnityPy.load(f)
        except Exception as e:
            print("skip bundle", os.path.basename(f), e)
            continue
        if not any(o.type.name == "AudioClip" for o in env.objects):
            continue
        before = len(index)
        export_clips(env, seen, index)
        print(f"[{i + 1}/{len(files)}] {os.path.basename(f)[:48]}: +{len(index) - before} clips")
    with open(os.path.join(E.OUT, "audio-index.json"), "w") as f:
        json.dump(index, f, indent=0)
    print("clips:", len(index))


def cmd_config():
    env = E.load_env("audio|stages-menu")
    out = {k: {} for k in SCRIPT_CLASSES}
    for obj in env.objects:
        if obj.type.name != "MonoBehaviour":
            continue
        cls = script_name(obj)
        if cls not in SCRIPT_CLASSES:
            continue
        try:
            tt = obj.read_typetree()
            name = tt.get("m_Name") or f"{cls}_{obj.path_id}"
            entry = resolve(tt)
            entry["_ptrs"] = pptr_names(obj)
            out[cls][name] = entry
        except Exception as e:
            print("config", cls, type(e).__name__, e)
    with open(os.path.join(E.OUT, "audio-config.json"), "w") as f:
        json.dump(out, f, indent=1)
    print({k: len(v) for k, v in out.items()})


def cmd_stage(stage):
    env = E.load_env(f"stages-{stage}")
    out = {"emitters": {}, "clips": []}
    for obj in env.objects:
        if obj.type.name != "MonoBehaviour":
            continue
        cls = script_name(obj)
        if cls == "SceneAudioConfig":
            try:
                out.setdefault("music", {})[obj.read().m_Name] = pptr_names(obj).get("musicData")
            except Exception as e:
                print("music", e)
            continue
        if cls not in ("PhysicAudioEmitter", "SceneAudioClip", "PlaySoundOnJointBreak", "SoundOnTriggerEnter"):
            continue
        try:
            tt = obj.read_typetree()
            go = tt["m_GameObject"]["m_PathID"]
            names = pptr_names(obj)
            if cls == "PhysicAudioEmitter":
                out["emitters"][str(go)] = names.get("audioData")
            else:
                entry = {"class": cls, "go": go, "ptrs": names}
                for k in ("loop", "play2D", "playOnAwake", "volume", "pitch", "minDistance", "maxDistance"):
                    if k in tt:
                        entry[k] = tt[k]
                out["clips"].append(entry)
        except Exception as e:
            print("stage", stage, cls, type(e).__name__, e)
    with open(os.path.join(E.OUT, f"audio-{stage}.json"), "w") as f:
        json.dump(out, f, indent=0)
    print(stage, len(out["emitters"]), "emitters,", len(out["clips"]), "clip players")


STAGES = ["alley", "aquarium", "billboard", "blimp", "buoy", "chute", "containers", "crane", "elevators", "girders",
          "gondola", "grind", "incinerator", "lighthouse", "ring", "rooftop", "subway", "towers", "train", "trawler",
          "trucks", "vents", "wheel"]

if __name__ == "__main__":
    what = sys.argv[1] if len(sys.argv) > 1 else "all"
    if what == "clips":
        cmd_clips()
    elif what == "config":
        cmd_config()
    elif what == "stage":
        cmd_stage(sys.argv[2])
    else:
        cmd_clips()
        cmd_config()
        for s in STAGES:
            try:
                cmd_stage(s)
            except Exception as e:
                print("stage failed", s, e)

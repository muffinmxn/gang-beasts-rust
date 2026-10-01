"""Extract the game's compiled D3D11 shaders and disassemble them.

  python export_shaders.py [shader-name-substring ...]      (default: GangBeasts/)

For each Shader asset: LZ4-decompress every blob chunk (Shader.compressedBlob), carve out each
DXBC container (magic "DXBC", total size at +24), dedupe, and disassemble with Windows'
d3dcompiler_47.dll (D3DDisassemble). Output: re/shaders/<shader>/<index>_<stage>.asm plus the raw
.dxbc, and index.json (per program: stage, size, the cbuffer variables it reads from the RDEF chunk).
Read-only on the game files.
"""
import ctypes, hashlib, json, os, re, struct, sys
sys.path.insert(0, os.path.dirname(__file__))
import export as E
import lz4.block

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
OUT = os.path.join(ROOT, "re", "shaders")

d3d = ctypes.WinDLL("d3dcompiler_47.dll")
d3d.D3DDisassemble.argtypes = [ctypes.c_void_p, ctypes.c_size_t, ctypes.c_uint, ctypes.c_char_p,
                               ctypes.POINTER(ctypes.c_void_p)]


class Blob(ctypes.Structure):
    pass


def disassemble(code: bytes) -> str:
    out = ctypes.c_void_p()
    buf = ctypes.create_string_buffer(code, len(code))
    hr = d3d.D3DDisassemble(buf, len(code), 0, None, ctypes.byref(out))
    if hr != 0 or not out:
        return f"; D3DDisassemble failed hr=0x{hr & 0xffffffff:08x}\n"
    # ID3DBlob vtable: QueryInterface, AddRef, Release, GetBufferPointer, GetBufferSize
    vt = ctypes.cast(ctypes.cast(out, ctypes.POINTER(ctypes.c_void_p))[0], ctypes.POINTER(ctypes.c_void_p))
    get_ptr = ctypes.WINFUNCTYPE(ctypes.c_void_p, ctypes.c_void_p)(vt[3])
    get_size = ctypes.WINFUNCTYPE(ctypes.c_size_t, ctypes.c_void_p)(vt[4])
    release = ctypes.WINFUNCTYPE(ctypes.c_ulong, ctypes.c_void_p)(vt[2])
    text = ctypes.string_at(get_ptr(out), get_size(out)).decode("utf-8", "replace")
    release(out)
    return text


def carve_dxbc(data: bytes):
    i = 0
    while True:
        i = data.find(b"DXBC", i)
        if i < 0 or i + 32 > len(data):
            return
        (size,) = struct.unpack_from("<I", data, i + 24)
        if 32 < size <= len(data) - i:
            yield data[i:i + size]
            i += size
        else:
            i += 4


def chunks(sh):
    """Decompressed per-platform blobs (one entry per compressed chunk)."""
    blob = bytes(sh.compressedBlob)
    for p in range(len(sh.platforms)):
        offs, clens, dlens = sh.offsets[p], sh.compressedLengths[p], sh.decompressedLengths[p]
        offs = offs if isinstance(offs, list) else [offs]
        clens = clens if isinstance(clens, list) else [clens]
        dlens = dlens if isinstance(dlens, list) else [dlens]
        for o, c, d in zip(offs, clens, dlens):
            yield lz4.block.decompress(blob[o:o + c], uncompressed_size=d)


def main():
    wanted = sys.argv[1:] or ["GangBeasts/"]
    env = E.load_env(os.environ.get("GB_SHADER_ENV", "stages-menu"))
    done = set()
    for obj in env.objects:
        if obj.type.name != "Shader":
            continue
        sh = obj.read()
        name = sh.m_ParsedForm.m_Name
        if name in done or not any(w in name for w in wanted):
            continue
        done.add(name)
        folder = os.path.join(OUT, re.sub(r"[^A-Za-z0-9_.-]", "_", name))
        os.makedirs(folder, exist_ok=True)
        seen, index = set(), []
        for data in chunks(sh):
            for code in carve_dxbc(data):
                h = hashlib.sha1(code).hexdigest()[:12]
                if h in seen:
                    continue
                seen.add(h)
                asm = disassemble(code)
                stage = "ps" if "\nps_5" in asm or asm.startswith("ps_") or "ps_5_0" in asm else \
                        "vs" if "vs_5_0" in asm else "other"
                n = len(index)
                open(os.path.join(folder, f"{n:04d}_{stage}.asm"), "w", encoding="utf-8").write(asm)
                open(os.path.join(folder, f"{n:04d}_{stage}.dxbc"), "wb").write(code)
                cbvars = sorted(set(re.findall(r"^//\s+\w+\s+(\w+)(?:\[\d+\])?;\s+// Offset", asm, re.M)))
                index.append({"file": f"{n:04d}_{stage}", "stage": stage, "hash": h, "bytes": len(code),
                              "instructions": asm.count("\n") , "cbuffer_vars": cbvars})
        json.dump(index, open(os.path.join(folder, "index.json"), "w"), indent=1)
        print(f"{name}: {len(index)} programs "
              f"({sum(1 for i in index if i['stage'] == 'vs')} vs, {sum(1 for i in index if i['stage'] == 'ps')} ps)")


if __name__ == "__main__":
    main()

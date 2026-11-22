r"""List the object presets in the ds1 files of a range of lvlprest rows, from the classic MPQs.

Usage: python3 -I research/ds1_objects.py <dir with lvlprest.json, objpreset.json> <first Def> <last Def> <archive>...
(pass Patch_D2.mpq first; fetch the tables like research/gen_objects.py does). Prints per ds1 file its
act, size in tiles and the object presets as (objpreset ObjectClass, sub-tile x, y). Used 2026-11-22
for the Kurast super chests (PLAN.md "Super chests"): lvlprest 605..652.
The ds1 layout (version-dependent header, tile layers, then objects) follows Paul Siramy's ds1 notes.
"""
import importlib.util
import json
import pathlib
import struct
import sys

spec = importlib.util.spec_from_file_location("mpq", pathlib.Path(__file__).with_name("mpq.py"))
mpq = importlib.util.module_from_spec(spec)
spec.loader.exec_module(mpq)


def rows(path):
    t = json.load(open(path))
    return list(t.values()) if isinstance(t, dict) else t


def parse(b):
    def i(o):
        return struct.unpack_from("<i", b, o)[0]

    ver, w, h = i(0), i(4) + 1, i(8) + 1
    o, act, tag = 12, 1, 0
    if ver >= 8:
        act, o = min(i(o), 4) + 1, o + 4
    if ver >= 10:
        tag, o = i(o), o + 4
    if ver >= 3:
        n, o = i(o), o + 4
        for _ in range(n):
            o = b.index(b"\0", o) + 1
    if 9 <= ver <= 13:
        o += 8
    if ver >= 4:
        walls, o = i(o), o + 4
        floors = 1
        if ver >= 16:
            floors, o = i(o), o + 4
        layers = walls * 2 + floors + 1 + (1 if tag in (1, 2) else 0)
    else:
        layers = 5
    o += layers * w * h * 4
    objs = []
    if ver >= 2:
        n, o = i(o), o + 4
        for _ in range(n):
            kind, idx, x, y = struct.unpack_from("<4i", b, o)
            o += 20 if ver > 5 else 16
            objs.append((kind, idx, x, y))
    return act, w, h, objs


src, first, last, archives = sys.argv[1], int(sys.argv[2]), int(sys.argv[3]), sys.argv[4:]
names = {(r["Act"], r["Index"]): r["ObjectClass"] for r in rows(f"{src}/objpreset.json")}
for r in rows(f"{src}/lvlprest.json"):
    if not first <= r["Def"] <= last:
        continue
    for k in range(1, 7):
        f = r.get(f"File{k}")
        if not f or f in (0, "0"):
            continue
        path = "data\\global\\tiles\\" + f.replace("/", "\\")
        data = next((d for d in (mpq.read_file(a, path) for a in archives) if d), None)
        if not data:
            print(r["Def"], f, "not found")
            continue
        act, w, h, objs = parse(data)
        found = [(names.get((act, idx), idx), x, y) for kind, idx, x, y in objs if kind == 2]
        print(r["Def"], r["Name"], f, f"act {act}, {w}x{h}", found)

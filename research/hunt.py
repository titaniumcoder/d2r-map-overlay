"""Read-only: find PlayerData blocks named NAME, then units (type 0) referencing them at +0x10; report act/seed/pos."""
import re, struct, sys
pid, name = int(sys.argv[1]), sys.argv[2].encode()
m = open(f"/proc/{pid}/mem", "rb", buffering=0)
def rd(a, n):
    try: m.seek(a); return m.read(n)
    except OSError: return None
regions = []
for line in open(f"/proc/{pid}/maps"):
    p = line.split(maxsplit=5)
    lo, hi = (int(x, 16) for x in p[0].split("-"))
    path = p[5].strip() if len(p) > 5 else ""
    if p[1].startswith("rw") and hi - lo <= (1 << 30) and not path.startswith("/") and "memfd" not in path:
        regions.append((lo, hi))
print("rw regions:", len(regions), "total MiB:", sum(h - l for l, h in regions) >> 20)
names = set()
blobs = []
for lo, hi in regions:
    d = rd(lo, hi - lo)
    if not d: continue
    blobs.append((lo, d))
    for mm in re.finditer(re.escape(name) + b"\x00", d):
        if mm.start() == 0 or d[mm.start() - 1] == 0:
            names.add(lo + mm.start())
print("name hits:", len(names))
units = []
for lo, d in blobs:
    for a in names:
        for mm in re.finditer(re.escape(struct.pack("<Q", a)), d):
            u = lo + mm.start() - 0x10
            if u < lo: continue
            off = mm.start() - 0x10
            utype, cls = struct.unpack_from("<II", d, off)
            if utype == 0 and cls < 10:
                units.append(u)
print("units referencing a name block:", len(units))
for u in units[:40]:
    h = rd(u, 0x40)
    utype, cls, uid, mode = struct.unpack_from("<IIII", h)
    _, actid, act, seed, iseed, path = struct.unpack_from("<QQQQQQ", h, 0x10)
    s = rd(act + 0x1C, 4) if act else None
    pos = None
    if path:
        pb = rd(path, 8)
        if pb: pos = struct.unpack("<HHHH", pb)[1::2]
    print(f"  unit {u:#x} id={uid:#x} mode={mode} actid={actid} act={act:#x} mapSeed={struct.unpack('<I', s)[0] if s else None} pos={pos}")

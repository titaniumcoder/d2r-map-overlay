"""Read-only: scan rw private memory for 8-byte-aligned {seed:u32, 0x29A:u32} pairs; print unique seeds by count."""
import collections, re, struct, sys
pid = int(sys.argv[1])
m = open(f"/proc/{pid}/mem", "rb", buffering=0)
c = collections.Counter(); where = {}
for line in open(f"/proc/{pid}/maps"):
    p = line.split(maxsplit=5)
    lo, hi = (int(x, 16) for x in p[0].split("-"))
    path = p[5].strip() if len(p) > 5 else ""
    if not p[1].startswith("rw") or hi - lo > (1 << 30) or path.startswith("/"): continue
    try: m.seek(lo); d = m.read(hi - lo)
    except OSError: continue
    for mm in re.finditer(rb"\x9a\x02\x00\x00", d):
        o = mm.start() - 4
        if o < 0 or (lo + o) % 8: continue
        s = struct.unpack_from("<I", d, o)[0]
        if s > 0xFFFF:
            c[s] += 1; where.setdefault(s, lo + o)
for s, n in c.most_common(25):
    print(s, n, hex(where[s]))

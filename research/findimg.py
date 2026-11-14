"""Read-only: find D2R.exe's PE image in memory by header, then scan it for D2RMH signatures."""
import re, struct, sys
pid = int(sys.argv[1])
SIGS = {
 "hashTable":   rb"\x48\x03\xC7\x49\x8B\x8C\xC6",
 "uiBase":      rb"\x48\x89\x45\xB7\x4C\x8D\x35.{4}",
 "isExpansion": rb"\x48\x8B\x05.{4}\x48\x8B\xD9\xF3\x0F\x10\x50",
 "roster":      rb"\x02\x45\x33\xD2\x4D\x8B",
 "gameInfo":    rb"\x44\x88\x25.{4}\x66\x44\x89\x25",
}
maps = []
for line in open(f"/proc/{pid}/maps"):
    p = line.split(maxsplit=5)
    lo, hi = (int(x, 16) for x in p[0].split("-"))
    maps.append((lo, hi, p[1], p[5].strip() if len(p) > 5 else ""))
mem = open(f"/proc/{pid}/mem", "rb", buffering=0)
def rd(a, n):
    try: mem.seek(a); return mem.read(n)
    except OSError: return b""
base = None
for lo, hi, perm, path in maps:
    if "r" not in perm: continue
    h = rd(lo, 0x400)
    if h[:2] != b"MZ": continue
    pe = struct.unpack_from("<I", h, 0x3c)[0]
    if pe > 0x3c0 or h[pe:pe+4] != b"PE\0\0": continue
    size_img = struct.unpack_from("<I", h, pe + 24 + 56)[0]
    if 0x2700000 <= size_img <= 0x2900000:
        print(f"candidate image {lo:#x} SizeOfImage={size_img:#x} perm={perm} {path[-50:]}")
        base = (lo, size_img)
if not base: sys.exit("D2R image not found")
lo, size = base
print("regions in image:")
for a, b, perm, path in maps:
    if lo <= a < lo + size: print(f"  {a:#x}-{b:#x} {perm} {(b-a)>>10} KiB")
data = bytearray()
a = lo
while a < lo + size:
    chunk = rd(a, min(1 << 20, lo + size - a))
    data += chunk if chunk else b"\0" * min(1 << 20, lo + size - a)
    a += min(1 << 20, lo + size - a)
text = bytes(data[0x1000:0x1000 + 22916608])
import math, collections
c = collections.Counter(text[:4_000_000]); t = sum(c.values())
print(f".text entropy in memory: {-sum(v/t*math.log2(v/t) for v in c.values()):.2f}")
for n, pat in SIGS.items():
    hits = [m.start() for m in re.finditer(pat, bytes(data), re.S)]
    print(f"  {n}: {len(hits)} hit(s) {[hex(lo+h) for h in hits[:3]]}")

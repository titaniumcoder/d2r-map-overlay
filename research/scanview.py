"""Read-only: scan one memory range of a pid for D2RMH signatures and report decrypted-page share."""
import re, sys, math, collections
pid, lo, size = int(sys.argv[1]), int(sys.argv[2], 16), int(sys.argv[3], 16)
SIGS = {"hashTable": rb"\x48\x03\xC7\x49\x8B\x8C\xC6", "uiBase": rb"\x48\x89\x45\xB7\x4C\x8D\x35.{4}",
        "isExpansion": rb"\x48\x8B\x05.{4}\x48\x8B\xD9\xF3\x0F\x10\x50", "roster": rb"\x02\x45\x33\xD2\x4D\x8B",
        "gameInfo": rb"\x44\x88\x25.{4}\x66\x44\x89\x25"}
m = open(f"/proc/{pid}/mem", "rb", buffering=0); m.seek(lo); d = m.read(size)
text = d[0x1000:0x1000 + 22916608]
low = hi = 0
for i in range(0, len(text), 4096):
    pg = text[i:i+4096]; c = collections.Counter(pg)
    e = -sum(v/4096*math.log2(v/4096) for v in c.values())
    if e > 7.9: hi += 1
    else: low += 1
print(f".text pages: {low} look like plain code/data, {hi} look encrypted (entropy>7.9)")
for n, p in SIGS.items():
    h = [x.start() for x in re.finditer(p, d, re.S)]
    print(f"  {n}: {len(h)} hit(s) {[hex(x) for x in h[:3]]}")

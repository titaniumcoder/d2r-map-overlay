"""Read-only: list every preset unit in the player's level (no seed needed).

Usage: sudo python3 -I presets.py <pid> <unit-addr-hex>
PresetUnit (verified 2026-11-14): +0x04 txtFileNo, +0x08 x (room-relative), +0x10 next, +0x20 type (1 monster, 2 object, 5 tile), +0x24 y
"""
import collections, struct, sys
pid, u = int(sys.argv[1]), int(sys.argv[2], 16)
m = open(f"/proc/{pid}/mem", "rb", buffering=0)
def rd(a, n):
    try: m.seek(a); return m.read(n)
    except (OSError, ValueError, OverflowError): return None
def q(a): b = rd(a, 8); return struct.unpack("<Q", b)[0] if b and len(b) == 8 else 0
def d(a): b = rd(a, 4); return struct.unpack("<I", b)[0] if b and len(b) == 4 else 0
path = q(u + 0x38)
px, py = struct.unpack("<HH", (rd(path + 2, 2) or b"\0\0") + (rd(path + 6, 2) or b"\0\0"))
level = q(q(q(path + 0x20) + 0x18) + 0x90)
print(f"level {d(level + 0x1F8)}, player ({px},{py})")
r, n, found = q(level + 0x10), 0, []
while r and n < 2000:
    n += 1
    rx, ry = d(r + 0x60) * 5, d(r + 0x64) * 5
    p, k = q(r + 0x98), 0
    while p and k < 200:
        k += 1
        typ, txt = d(p + 0x20), d(p + 0x04)
        found.append((typ, txt, rx + d(p + 0x08), ry + d(p + 0x24)))
        p = q(p + 0x10)
    r = q(r + 0x48)
print("rooms:", n, "presets:", len(found), "by type:", dict(collections.Counter(t for t, *_ in found)))
for typ, txt, x, y in sorted(found):
    if typ != 1:
        print(f"  type {typ} id {txt:4d} at ({x},{y})  dist {int(((x-px)**2+(y-py)**2)**0.5)}")

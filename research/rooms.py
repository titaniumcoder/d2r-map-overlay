"""Read-only probe: draw the current level's Room2 layout (no seed needed).

Usage: sudo python3 -I rooms.py <pid> <player-name> <out.png>

Chain (D2R ~3.3.x, verified live 2026-11-14):
  unit +0x10 PlayerData (name @0), +0x20 act, +0x38 path
  path +0x02 x, +0x06 y, +0x20 room1;  room1 +0x18 room2;  room2 +0x90 level;  level +0x1F8 levelId
Room2 (D2RMH layout): +0x48 next, +0x60 posX, +0x64 posY, +0x68 sizeX, +0x6C sizeY (room tiles; x5 = world)
"""
import re
import struct
import sys
import zlib

pid, name, out = int(sys.argv[1]), sys.argv[2].encode(), sys.argv[3]
mem = open(f"/proc/{pid}/mem", "rb", buffering=0)


def rd(a, n):
    try:
        mem.seek(a)
        return mem.read(n)
    except (OSError, ValueError, OverflowError):
        return None


def q(a):
    b = rd(a, 8)
    return struct.unpack("<Q", b)[0] if b and len(b) == 8 else 0


def d(a):
    b = rd(a, 4)
    return struct.unpack("<I", b)[0] if b and len(b) == 4 else 0


def w(a):
    b = rd(a, 2)
    return struct.unpack("<H", b)[0] if b and len(b) == 2 else 0


def find_player():
    """Find the live unit: type 0, PlayerData name == NAME, act and path set, nonzero position."""
    regions = []
    for line in open(f"/proc/{pid}/maps"):
        p = line.split(maxsplit=5)
        lo, hi = (int(x, 16) for x in p[0].split("-"))
        path = p[5].strip() if len(p) > 5 else ""
        if p[1].startswith("rw") and hi - lo <= (1 << 30) and not path.startswith("/") and "memfd" not in path:
            regions.append((lo, hi))
    blobs, names = [], set()
    for lo, hi in regions:
        b = rd(lo, hi - lo)
        if not b:
            continue
        blobs.append((lo, b))
        for mm in re.finditer(re.escape(name) + b"\x00", b):
            if mm.start() == 0 or b[mm.start() - 1] == 0:
                names.add(lo + mm.start())
    live = []
    for lo, b in blobs:
        for a in names:
            for mm in re.finditer(re.escape(struct.pack("<Q", a)), b):
                u = lo + mm.start() - 0x10
                if u >= lo and d(u) == 0 and q(u + 0x20) and q(u + 0x38) and w(q(u + 0x38) + 2):
                    live.append(u)
    return live


units = find_player()
if not units:
    sys.exit("no live player unit found (in game?)")
u = units[0]
path = q(u + 0x38)
px, py = w(path + 2), w(path + 6)
room2 = q(q(path + 0x20) + 0x18)
level = q(room2 + 0x90)
level_id = d(level + 0x1F8)
print(f"units={[hex(x) for x in units]} using {u:#x}: pos=({px},{py}) level={level_id} level@{level:#x}")

# Find level's first-room pointer: a field pointing at a Room2 whose +0x90 is this level.
first, first_off = 0, None
blk = rd(level, 0x200) or b""
for o in range(0, len(blk) - 7, 8):
    p = struct.unpack_from("<Q", blk, o)[0]
    if 0x10000 < p < 0x7FFFFFFFFFFF and q(p + 0x90) == level:
        first, first_off = p, o
        break
if not first:
    sys.exit("no first-room pointer found in level struct")
print(f"level+{first_off:#x} -> first Room2 {first:#x}")

rooms, r, seen = [], first, set()
while r and r not in seen and len(rooms) < 2000:
    seen.add(r)
    if q(r + 0x90) != level:
        break
    x, y, sx, sy = (d(r + o) for o in (0x60, 0x64, 0x68, 0x6C))
    rooms.append((x * 5, y * 5, sx * 5, sy * 5, bool(q(r + 0x58))))
    r = q(r + 0x48)
print(f"rooms: {len(rooms)}  (with loaded Room1/collision: {sum(1 for x in rooms if x[4])})")
if not rooms:
    sys.exit("room chain empty")

minx = min(x for x, *_ in rooms); miny = min(y for _, y, *_ in rooms)
maxx = max(x + sx for x, _, sx, _, _ in rooms); maxy = max(y + sy for _, y, _, sy, _ in rooms)
print(f"level bounds world: x {minx}..{maxx}, y {miny}..{maxy}; player inside: {minx <= px < maxx and miny <= py < maxy}")

# Render: 1px per world tile, scaled down to <= 800px.
W, H = maxx - minx, maxy - miny
s = max(1, (max(W, H) + 799) // 800)
iw, ih = W // s + 1, H // s + 1
img = [bytearray(b"\x18\x18\x18" * iw) for _ in range(ih)]


def put(x, y, c):
    if 0 <= x < iw and 0 <= y < ih:
        img[y][x * 3:x * 3 + 3] = c


for x, y, sx, sy, loaded in rooms:
    fill = b"\x50\x70\x50" if loaded else b"\x40\x40\x58"
    for yy in range((y - miny) // s, (y + sy - miny) // s):
        for xx in range((x - minx) // s, (x + sx - minx) // s):
            edge = yy in ((y - miny) // s, (y + sy - miny) // s - 1) or xx in ((x - minx) // s, (x + sx - minx) // s - 1)
            put(xx, yy, b"\xa0\xa0\xa0" if edge else fill)
cx, cy = (px - minx) // s, (py - miny) // s
for dy in range(-3, 4):
    for dx in range(-3, 4):
        put(cx + dx, cy + dy, b"\xff\x30\x30")

raw = b"".join(b"\x00" + bytes(row) for row in img)
png = b"\x89PNG\r\n\x1a\n"
for tag, data in ((b"IHDR", struct.pack(">IIBBBBB", iw, ih, 8, 2, 0, 0, 0)), (b"IDAT", zlib.compress(raw, 9)), (b"IEND", b"")):
    png += struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data))
open(out, "wb").write(png)
print("wrote", out, f"{iw}x{ih} scale 1:{s}")

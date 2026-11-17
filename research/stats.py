"""Read-only: dump the player's stat list entries to find where the item-granted stats live.

The overlay reads the player stat list at unit +0x88 (entries +0x30, count +0x38, entry
{u16 layer, u16 stat id, u32 value}) and gets level 12 fine, but the character screen shows
res 6/9/8/7 while the read returns zeros. This probe prints everything.

Usage: sudo python3 -I stats.py <pid> <unit table address from the overlay log>
Reads /proc/<pid>/mem; never writes anything."""
import struct
import sys

pid, ht = int(sys.argv[1]), int(sys.argv[2], 16)
m = open(f"/proc/{pid}/mem", "rb", buffering=0)


def rd(a, n):
    try:
        m.seek(a)
        return m.read(n)
    except OSError:
        return None


def u64(a):
    b = rd(a, 8)
    return struct.unpack("<Q", b)[0] if b and len(b) == 8 else None


def u32(a):
    b = rd(a, 4)
    return struct.unpack("<I", b)[0] if b and len(b) == 4 else None


def stat_entries(list_addr):
    """All {layer, stat id, value} entries of the stat list at `list_addr`."""
    count = u64(list_addr + 0x38)
    if not count or count > 512:
        return None
    entries = u64(list_addr + 0x30)
    raw = rd(entries, count * 8) if entries else None
    if not raw:
        return None
    out = []
    for i in range(count):
        layer, sid, val = struct.unpack_from("<HHI", raw, i * 8)
        out.append((layer, sid, val))
    return out


def find_player():
    base = ht  # type 0 block starts at the table itself
    for slot in range(128):
        p = u64(base + slot * 8)
        seen = 0
        while p and seen < 60:
            seen += 1
            hdr = rd(p, 0x40)
            if not hdr:
                break
            utype, txt, uid, mode = struct.unpack_from("<IIII", hdr, 0)
            unit_data, _, act, _, _, path = struct.unpack_from("<QQQQQQ", hdr, 0x10)
            if utype == 0 and act and path:
                px, py = struct.unpack_from("<HH", rd(path, 8) or b"")
                if px and py:
                    return p, uid, txt
            nxt = u32(p + 0x158) or u32(p + 0x150)
            p = u64(nxt) if nxt else None
    return None


player = find_player()
if not player:
    sys.exit("no live player unit found")
addr, uid, txt = player
print(f"player unit {addr:#x} id {uid:#x} class {txt}")

# 1. The stat list the overlay reads: unit +0x88.
sl = u64(addr + 0x88)
print(f"\n== stat list at +0x88 = {sl:#x}")
entries = stat_entries(sl)
for layer, sid, val in entries or []:
    print(f"  layer {layer:5} stat {sid:4} value {val}")

# 2. The raw StatList header — look for chain/parent pointers around it.
print(f"\n== stat list header {sl:#x} (first 0x40 bytes):")
hdr = rd(sl, 0x40) or b""
for off in range(0, 0x40, 8):
    v = struct.unpack_from("<Q", hdr, off)[0]
    print(f"  +{off:#04x}: {v:#018x}")

# 3. Also dump the unit header's other candidate stat-list pointers nearby (+0x80..+0xB0).
print("\n== unit header +0x78..+0xC0:")
uh = rd(addr + 0x78, 0x48) or b""
for off in range(0, 0x48, 8):
    v = struct.unpack_from("<Q", uh, off)[0]
    print(f"  +{0x78 + off:#04x}: {v:#018x}")

# 4. The inventory items and their stat lists: the character screen shows res 6/9/8/7 from
#    gear — find which stat ids the ITEMS use for res.
inv = u64(addr + 0x90)
print(f"\n== inventory at +0x90 = {inv:#x}")
if inv:
    first = u64(inv + 0x18)  # first item unit in the inventory list
    seen = 0
    while first and seen < 60:
        seen += 1
        hdr = rd(first, 0x40)
        if not hdr:
            break
        utype, txt, uid, mode = struct.unpack_from("<IIII", hdr, 0)
        item_data, _, _, _, _, path = struct.unpack_from("<QQQQQQ", hdr, 0x10)
        code = (rd(item_data + 0x20, 4) if item_data else b"")  # guess; printed raw anyway
        stats = stat_entries(u64(first + 0x88) or 0) if u64(first + 0x88) else None
        interesting = [(l, s, v) for l, s, v in (stats or []) if s in (39, 40, 41, 42, 43, 44, 45, 46, 93, 99, 105) or (v and s >= 90)]
        if stats is not None:
            print(f"  item {first:#x} txt {txt} mode {mode} entries {len(stats)}"
                  + (f" interesting: {interesting}" if interesting else ""))
        nxt = u64(first + 0x150) or u64(first + 0x158)
        first = nxt
        if seen >= 60:
            break

# 5. Hunt for the FULL (item-combined) stat list: scan writable memory for StatLists owned by
#    the player's unit id (owner dword at +0x0C like the base list) and print those with
#    resistance entries.
print("\n== scanning writable memory for stat lists owned by the player:")
import re
uid_le = struct.pack("<I", uid & 0xFFFFFFFF)
maps = []
with open(f"/proc/{pid}/maps") as f:
    for line in f:
        parts = line.split(maxsplit=5)
        lo, hi = (int(x, 16) for x in parts[0].split("-"))
        if "rw" in parts[1] and hi - lo < (1 << 30):
            maps.append((lo, hi))
CHUNK = 1 << 22
found_lists = 0
for lo, hi in maps:
    a = lo
    while a < hi:
        n = min(CHUNK, hi - a)
        buf = rd(a, n)
        if not buf:
            break
        for hit in re.finditer(re.escape(uid_le), buf):
            off = hit.start()
            if off < 0xC or off + 0x40 > n:
                continue
            sl = a + off - 0xC  # candidate StatList base (owner at +0x0C)
            count = u64(sl + 0x38)
            if not count or count > 512:
                continue
            entries = stat_entries(sl)
            if not entries:
                continue
            found_lists += 1
            res = [(l, s, v) for l, s, v in entries if s in (39, 40, 41, 42, 43, 44, 45, 46)]
            flag = " <-- HAS RES" if res else ""
            print(f"  stat list {sl:#x} entries {len(entries)}{flag}")
            if res or len(entries) != 21:
                for l, s, v in entries[:40]:
                    print(f"    layer {l:5} stat {s:4} value {v}")
        a += n
print("candidate lists:", found_lists)

"""Read-only: dump the equipped items' socket children (runes) and the runeword flag.

For each equipped ITEM unit (ItemData body slot 1..12): its code, quality, FLAGS (and the
classic runeword bit 0x04000000), then the item's own Inventory (unit +0x90) chain:
inventory +0x10 first item, ItemData +0xB0 next, each child's code / page / body.
Usage: sudo python3 -I sockets.py <pid> <unit table address hex>
"""
import re
import struct
import sys

pid, ht = int(sys.argv[1]), int(sys.argv[2], 16)
m = open(f"/proc/{pid}/mem", "rb", buffering=0)


def rd(a, n):
    try:
        m.seek(a)
        return m.read(n)
    except (OSError, ValueError):
        return None


def u64(a):
    b = rd(a, 8)
    return struct.unpack("<Q", b)[0] if b and len(b) == 8 else None


def u32(a):
    b = rd(a, 4)
    return struct.unpack("<I", b)[0] if b and len(b) == 4 else None


# Items table: the first five weapons.txt codes one 0x1C0 record apart.
SIG = re.compile(rb"hax .{444}axe .{444}2ax ", re.S)
table = None
with open(f"/proc/{pid}/maps") as f:
    regions = []
    for line in f:
        parts = line.split()
        lo, hi = (int(x, 16) for x in parts[0].split("-"))
        if parts[1].startswith("rw") and hi - lo < (1 << 30):
            regions.append((lo, hi))
for lo, hi in regions:
    a = lo
    while a < hi and table is None:
        n = min(8 << 20, hi - a)
        b = rd(a, n)
        if b:
            mt = SIG.search(b)
            if mt:
                table = a + mt.start()
        a += n - 0x400 if n > 0x400 else n
    if table:
        break
print(f"items table {table:#x}" if table else "items table not found")


def code(txt):
    if not table:
        return f"txt{txt}"
    b = rd(table + txt * 0x1C0, 4)
    return b.decode("latin1").strip() if b else "?"


base = ht + 4 * 128 * 8  # ITEM block
for slot in range(128):
    p = u64(base + slot * 8)
    seen = 0
    while p and seen < 200:
        seen += 1
        utype, txt, uid, mode = struct.unpack_from("<IIII", rd(p, 16) or bytes(16), 0)
        d = u64(p + 0x10) if utype == 4 else None
        if d:
            hd = rd(d, 0x60)
            body, page = hd[0x54], hd[0x55]
            if body not in (0, 255):
                quality, owner, flags = u32(d), u32(d + 0x0C), u32(d + 0x18)
                print(f"\nitem {p:#x} {code(txt)} id {uid:#x} body {body} quality {quality} "
                      f"owner {owner:#x} flags {flags:#010x} runeword-bit {bool(flags & 0x04000000)}")
                inv = u64(p + 0x90)
                first = u64(inv + 0x10) if inv else None
                print(f"   inventory {inv and hex(inv)} first {first and hex(first)} "
                      f"count@+0x4C {u32(inv + 0x4C) if inv else None}")
                c, n = first, 0
                while c and n < 8:
                    n += 1
                    ct, ctxt, cid = struct.unpack_from("<III", rd(c, 12) or bytes(12), 0)
                    cd = u64(c + 0x10)
                    cb = rd(cd, 0x60) if cd else None
                    print(f"     child {c:#x} type {ct} {code(ctxt)} id {cid:#x} "
                          f"body {cb and cb[0x54]} page {cb and cb[0x55]} owner {cd and hex(u32(cd + 0x0C))}")
                    c = u64(cd + 0xB0) if cd else None
        p = u64(p + 0x158)

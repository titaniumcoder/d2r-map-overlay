"""Read-only: dump res/speed entries of every ITEM unit, to sum the player's gear stats.

The player's stat list (+0x88) is base-only; the character screen (res 6/9/8/7) must be the
sum of the equipped/carried items' stat lists. This probe prints all item stat entries so the
sum can be checked.
Usage: sudo python3 -I itemstats.py <pid> <unit table address>
"""
import struct
import sys

pid, ht = int(sys.argv[1]), int(sys.argv[2], 16)
m = open(f"/proc/{pid}/mem", "rb", buffering=0)
RES_IDS = {39, 40, 41, 42, 43, 44, 45, 46}
SPEED_IDS = {93, 99, 105, 84, 90}


def rd(a, n):
    try:
        m.seek(a)
        return m.read(n)
    except OSError:
        return None


def u64(a):
    b = rd(a, 8)
    return struct.unpack("<Q", b)[0] if b and len(b) == 8 else None


def entries(list_addr):
    count = u64(list_addr + 0x38)
    if not count or count > 256:
        return None
    p = u64(list_addr + 0x30)
    raw = rd(p, count * 8) if p else None
    if not raw:
        return None
    return [struct.unpack_from("<HHI", raw, i * 8) for i in range(count)]


base = ht + 4 * 128 * 8  # ITEM block
totals = {}
for slot in range(128):
    p = u64(base + slot * 8)
    seen = 0
    while p and seen < 80:
        seen += 1
        hdr = rd(p, 0x40)
        if not hdr:
            break
        utype, txt, uid, mode = struct.unpack_from("<IIII", hdr, 0)
        if utype == 4:
            item_data = u64(p + 0x10)
            page = u8 = None
            body = None
            if item_data:
                b = rd(item_data + 0x54, 2)
                if b:
                    body, page = b[0], b[1]
            st = entries(u64(p + 0x88) or 0) if u64(p + 0x88) else None
            if st:
                res = [(s, v) for _, s, v in st if s in RES_IDS]
                speed = [(s, v) for _, s, v in st if s in SPEED_IDS]
                if res or speed:
                    where = f"body {body}" if body not in (None, 255) else f"page {page}"
                    print(f"item {p:#x} txt {txt} id {uid:#x} mode {mode} {where}: res {res} speed {speed}")
                    for s, v in res + speed:
                        if s in RES_IDS:
                            key = ("res", s)
                        else:
                            key = ("speed", s)
                        totals[key] = totals.get(key, 0) + v
        nxt = u64(p + 0x158) or u64(p + 0x150)
        p = nxt
print("\nSUMS over all item stat lists:")
for (kind, s), v in sorted(totals.items()):
    print(f"  {kind} stat {s}: {v}")
print("\ncharacter screen says: fire 6, cold 9, lightning 8, poison 7 (Normal, no penalty)")

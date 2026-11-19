"""Usage: sudo python3 -I research/fullcheck.py <pid> <unit table hex>
Read-only: for every item unit compare the owner-matching chain sum with the +0xE8 full array."""
import struct, sys
pid, ht = int(sys.argv[1]), int(sys.argv[2], 16)
m = open(f"/proc/{pid}/mem", "rb", buffering=0)
def rd(a, n):
    try:
        m.seek(a); return m.read(n)
    except (OSError, ValueError, OverflowError):
        return None
def u64(a):
    b = rd(a, 8); return struct.unpack("<Q", b)[0] if b and len(b) == 8 else None
def u32(a):
    b = rd(a, 4); return struct.unpack("<I", b)[0] if b and len(b) == 4 else None
def arr(p, c):
    if not p or not c or c > 300: return []
    raw = rd(p, c*8)
    return [struct.unpack_from("<HHI", raw, i*8) for i in range(c)] if raw else []
W = {39,41,43,45,93,99,105}
base = ht + 4*128*8
stats = {"same":0, "diff":0, "noflag":0}
for slot in range(128):
    p = u64(base + slot*8); seen = 0
    while p and seen < 300:
        seen += 1
        utype, txt, uid = struct.unpack_from("<III", rd(p, 12) or bytes(12), 0)
        d = u64(p+0x10) if utype == 4 else None
        sl = u64(p+0x88) if d else None
        if sl:
            hd = rd(d, 0x60); body, page = hd[0x54], hd[0x55]; owner = u32(d+0x0C)
            fl = u32(sl+0x1C)
            chain = {}
            l, n = sl, 0
            while l and n < 8 and u32(l+8) == 4 and u32(l+0xC) == uid:
                for _, s, v in arr(u64(l+0x30), u64(l+0x38)):
                    if s in W: chain[s] = chain.get(s, 0) + v
                l = u64(l+0xD0); n += 1
            full = {}
            for _, s, v in arr(u64(sl+0xE8), u64(sl+0xF0)):
                if s in W: full[s] = full.get(s, 0) + v
            nsock = u64(p+0x90)
            kind = "same" if chain == full else "diff"
            if not fl & 0x80000000: kind = "noflag"
            stats[kind] += 1
            if kind != "same" or body not in (0,255):
                print(f"{kind:6} txt {txt} id {uid:#x} body {body} page {page} owner {owner:#x} flag {fl:#x} chain {chain} full {full}")
        p = u64(p+0x158)
print(stats)

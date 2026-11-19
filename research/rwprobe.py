"""Usage: sudo python3 -I research/rwprobe.py <pid> <unit table hex>
Read-only: equipped items' stat list chains (no owner stop) + socket children chains."""
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
WANT = {39,41,43,45,93,99,105,194,0,2,16,31}
def arr(p, c):
    if not p or not c or c > 300: return []
    raw = rd(p, c*8)
    if not raw: return None
    return [struct.unpack_from("<HHi", raw, i*8) for i in range(c)]
def show_list(sl, indent, maxn=12):
    seen = set()
    n = 0
    while sl and sl not in seen and n < maxn:
        seen.add(sl); n += 1
        hdr = rd(sl, 0x100)
        if not hdr: print(indent, "unreadable", hex(sl)); return
        ty, oid, flags = struct.unpack_from("<III", hdr, 0x08)
        q = struct.unpack_from("<32Q", hdr, 0)
        e = arr(q[6], q[7])
        print(f"{indent}list {sl:#x} owner type {ty} id {oid:#x} +0x10 {flags:#x} +0x14 {u32(sl+0x14):#x} +0x18 {u32(sl+0x18):#x} +0x1c {u32(sl+0x1c):#x} count {q[7]}")
        print(f"{indent}   qwords: " + " ".join(f"{i*8:#x}={v:#x}" for i, v in enumerate(q) if v))
        print(f"{indent}   entries: {e}")
        # Other possible {ptr,count} arrays in the header
        for off in range(0x40, 0xF8, 8):
            p, c = q[off//8], q[off//8+1] if off//8+1 < 32 else 0
            if p > 0x10000 and 0 < c < 100:
                a = arr(p, c)
                if a and all(s < 400 for _, s, _ in a):
                    print(f"{indent}   ARRAY@{off:#x} count {c}: {a}")
        sl = q[0xD0//8]
def code(txt):
    return txt
base = ht + 4*128*8
for slot in range(128):
    p = u64(base + slot*8); seen = 0
    while p and seen < 200:
        seen += 1
        utype, txt, uid, mode = struct.unpack_from("<IIII", rd(p, 16) or bytes(16), 0)
        d = u64(p + 0x10) if utype == 4 else None
        if d:
            hd = rd(d, 0x60)
            body, page = hd[0x54], hd[0x55]
            owner, flags, q = u32(d+0x0C), u32(d+0x18), u32(d)
            if body not in (0, 255) and owner != 0xFFFFFFFF:
                print(f"\n=== item {p:#x} txt {txt} id {uid:#x} body {body} quality {q} owner {owner:#x} flags {flags:#x}")
                show_list(u64(p+0x88), "  ")
                inv = u64(p+0x90)
                c = u64(inv+0x10) if inv else None
                k = 0
                while c and k < 6:
                    k += 1
                    ct, ctxt, cid = struct.unpack_from("<III", rd(c, 12) or bytes(12), 0)
                    print(f"  -- child {c:#x} type {ct} txt {ctxt} id {cid:#x}")
                    show_list(u64(c+0x88), "     ", 4)
                    cd = u64(c+0x10); c = u64(cd+0xB0) if cd else None
        p = u64(p + 0x158)

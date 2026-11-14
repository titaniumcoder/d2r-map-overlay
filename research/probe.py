"""Read-only: follow D2RMH's 2022 struct layout from the hashTable signature to the player unit."""
import re, struct, sys
pid, base, view = int(sys.argv[1]), int(sys.argv[2], 16), int(sys.argv[3], 16)
m = open(f"/proc/{pid}/mem", "rb", buffering=0)
def rd(a, n):
    try: m.seek(a); return m.read(n)
    except OSError: return None
def u64(a):
    b = rd(a, 8); return struct.unpack("<Q", b)[0] if b and len(b) == 8 else None
def u32(a):
    b = rd(a, 4); return struct.unpack("<I", b)[0] if b and len(b) == 4 else None
img = rd(view, 0x279c000)
off = re.search(rb"\x48\x03\xC7\x49\x8B\x8C\xC6", img).start()
rel = struct.unpack_from("<i", img, off + 7)[0]
ht = base + rel
print(f"sig at +{off:#x}, rel={rel:#x}, hashTable={ht:#x}")
found = 0
for i in range(128):
    p = u64(ht + i * 8)
    seen = 0
    while p and seen < 50:
        seen += 1
        hdr = rd(p, 0x40)
        if not hdr: print(f"  slot {i}: unreadable {p:#x}"); break
        utype, txt, uid, mode = struct.unpack_from("<IIII", hdr, 0)
        unit_data, _, act, seed, init_seed, path = struct.unpack_from("<QQQQQQ", hdr, 0x10)
        name = (rd(unit_data, 16) or b"").split(b"\0")[0]
        act_seed = u32(act + 0x1C) if act else None
        act_id = u32(act + 0x28) if act else None
        px = py = None
        if path:
            pb = rd(path, 8)
            if pb: _, px, _, py = struct.unpack("<HHHH", pb)
        print(f"  slot {i}: type={utype} class={txt} id={uid:#x} mode={mode} name={name!r} "
              f"actId={act_id} mapSeed={act_seed} pos=({px},{py})")
        found += 1
        nxt = None
        for cand in (0x150, 0x158):
            pass
        break
print("units found:", found)

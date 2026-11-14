"""Read-only: BFS over pointers (depth 3) from the player unit; report structs with dword 0x29A at +0x844."""
import struct, sys
pid, u = int(sys.argv[1]), int(sys.argv[2], 16)
m = open(f"/proc/{pid}/mem", "rb", buffering=0)
def rd(a, n):
    try: m.seek(a); return m.read(n)
    except OSError: return None
seen, frontier = set(), [(u, "unit")]
for depth in range(3):
    nxt = []
    for addr, path in frontier:
        if addr in seen: continue
        seen.add(addr)
        b = rd(addr + 0x838, 0x30)
        if b and len(b) == 0x30 and struct.unpack_from("<I", b, 0xC)[0] == 0x29A:
            v = [struct.unpack_from("<I", b, i)[0] for i in range(0, 0x30, 4)]
            print(f"{path} @ {addr:#x}: +0x840={v[2]} ({v[2]:#x}) +0x844={v[3]:#x}  +0x860={v[10]:#x} {v[11]:#x}")
        blk = rd(addr, 0x200)
        if not blk: continue
        for o in range(0, len(blk) - 7, 8):
            p = struct.unpack_from("<Q", blk, o)[0]
            if 0x10000 < p < 0x7fffffffffff and p not in seen:
                nxt.append((p, f"{path}+{o:#x}"))
    frontier = nxt
print("structs visited:", len(seen))

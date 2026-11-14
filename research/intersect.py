"""Read-only: values reachable (depth 2, 0x900 bytes per struct) from each unit, intersected with save-file u32s."""
import struct, sys
pid, save = int(sys.argv[1]), sys.argv[2]
units = [int(x, 16) for x in sys.argv[3:]]
d = open(save, "rb").read()
sv = {}
for o in range(0, len(d) - 3):
    v = struct.unpack_from("<I", d, o)[0]
    if 0xFFFFF < v < 0xFFFFFFFF and v not in (0x4fff0cff,): sv.setdefault(v, o)
m = open(f"/proc/{pid}/mem", "rb", buffering=0)
def rd(a, n):
    try: m.seek(a); return m.read(n)
    except OSError: return None
for u in units:
    seen, frontier = set(), [(u, "unit")]
    for depth in range(3):
        nxt = []
        for a, path in frontier:
            if a in seen: continue
            seen.add(a); b = rd(a, 0x900)
            if not b: continue
            for o in range(0, len(b) - 3, 4):
                v = struct.unpack_from("<I", b, o)[0]
                if v in sv: print(f"unit {u:#x}: {path}+{o:#x} = {v} (save offset {sv[v]:#x})")
            if depth < 2:
                for o in range(0, 0x200, 8):
                    p = struct.unpack_from("<Q", b, o)[0]
                    if 0x10000 < p < 0x7fffffffffff: nxt.append((p, f"{path}+{o:#x}"))
        frontier = nxt

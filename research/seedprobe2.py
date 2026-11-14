"""Read-only: walk hash table chains (next @+0x150) to the live player; dump seed hashes and level id."""
import struct, sys
pid, ht = int(sys.argv[1]), int(sys.argv[2], 16)
units = [int(x, 16) for x in sys.argv[3:]]
m = open(f"/proc/{pid}/mem", "rb", buffering=0)
def rd(a, n):
    try: m.seek(a); return m.read(n)
    except OSError: return None
def q(a): b = rd(a, 8); return struct.unpack("<Q", b)[0] if b else 0
def d(a): b = rd(a, 4); return struct.unpack("<I", b)[0] if b else None
def w(a): b = rd(a, 2); return struct.unpack("<H", b)[0] if b else None
for i, u0 in enumerate(units):
    u = u0; n = 0
    while u and n < 64:
        n += 1
        act, path = q(u + 0x20), q(u + 0x38)
        if d(u) == 0 and act and path:
            name = (rd(q(u + 0x10), 16) or b"").split(b"\0")[0]
            x, y = w(path + 2), w(path + 6)
            room1 = q(path + 0x20); room2 = q(room1 + 0x18) if room1 else 0
            level = q(room2 + 0x90) if room2 else 0
            lvl = d(level + 0x1F8) if level else None
            misc = q(act + 0x78)
            h1, h2, e1 = d(misc + 0x840), d(misc + 0x844), d(misc + 0x868)
            diff = w(misc + 0x830)
            print(f"slot {i} unit {u:#x} name={name!r} pos=({x},{y}) levelId={lvl} difficulty={diff}")
            print(f"  initSeedHash1={h1:#010x} initSeedHash2={h2:#010x} endSeedHash1={e1:#010x}  act+0x1C={d(act+0x1C)}")
        break

"""Read-only: hex-dump DrlgAct and ActMisc (0x7c0-0x8c0) for the player unit."""
import struct, sys
pid, u = int(sys.argv[1]), int(sys.argv[2], 16)
m = open(f"/proc/{pid}/mem", "rb", buffering=0)
def q(a): m.seek(a); return struct.unpack("<Q", m.read(8))[0]
def dump(label, base, start, n):
    m.seek(base + start); b = m.read(n)
    print(label)
    for o in range(0, n, 16):
        print(f"  +{start+o:#05x}", " ".join(f"{struct.unpack_from('<I', b, o+i)[0]:08x}" for i in range(0, 16, 4)))
act = q(u + 0x20); misc = q(act + 0x78)
print(f"act {act:#x} misc {misc:#x}")
dump("act", act, 0, 0x80)
dump("misc", misc, 0x7c0, 0x100)

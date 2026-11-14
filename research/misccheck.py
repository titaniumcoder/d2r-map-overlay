"""Read-only: for each pointer field in DrlgAct, show dwords at +0x828..+0x870 (old ActMisc seed/difficulty area)."""
import struct, sys
pid, u = int(sys.argv[1]), int(sys.argv[2], 16)
m = open(f"/proc/{pid}/mem", "rb", buffering=0)
def rd(a, n):
    try: m.seek(a); return m.read(n)
    except OSError: return None
def q(a): return struct.unpack("<Q", rd(a, 8))[0]
act = q(u + 0x20)
for off in range(0, 0x100, 8):
    p = q(act + off)
    if not (0x10000 < p < 0x7fffffffffff): continue
    b = rd(p + 0x828, 0x48)
    if not b: print(f"act+{off:#x} -> {p:#x}: unreadable at +0x828"); continue
    dw = [struct.unpack_from("<I", b, i)[0] for i in range(0, 0x48, 4)]
    print(f"act+{off:#x} -> {p:#x}: diff@0x830={struct.unpack_from('<H', b, 8)[0]}  " + " ".join(f"{x:08x}" for x in dw))

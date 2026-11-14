"""Read-only: sample player (level, x, y) and act+0x14 once per second."""
import struct, sys, time
pid, u, secs = int(sys.argv[1]), int(sys.argv[2], 16), int(sys.argv[3])
m = open(f"/proc/{pid}/mem", "rb", buffering=0)
def rd(a, n): m.seek(a); return m.read(n)
def q(a): return struct.unpack("<Q", rd(a, 8))[0]
def d(a): return struct.unpack("<I", rd(a, 4))[0]
def w(a): return struct.unpack("<H", rd(a, 2))[0]
for _ in range(secs):
    try:
        path, act = q(u + 0x38), q(u + 0x20)
        lvl = d(q(q(q(path + 0x20) + 0x18) + 0x90) + 0x1F8)
        print(lvl, w(path + 2), w(path + 6), d(act + 0x14), flush=True)
    except Exception as e:
        print("err", e, flush=True)
    time.sleep(1)

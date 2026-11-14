"""Read-only: every 2s print level id, room count and loaded-room count for a unit.

Usage: sudo python3 -I roomcount.py <pid> <unit-addr-hex> <seconds>
"""
import struct, sys, time
pid, u, secs = int(sys.argv[1]), int(sys.argv[2], 16), int(sys.argv[3])
m = open(f"/proc/{pid}/mem", "rb", buffering=0)
def rd(a, n):
    try: m.seek(a); return m.read(n)
    except (OSError, ValueError, OverflowError): return None
def q(a): b = rd(a, 8); return struct.unpack("<Q", b)[0] if b and len(b) == 8 else 0
def d(a): b = rd(a, 4); return struct.unpack("<I", b)[0] if b and len(b) == 4 else 0
for _ in range(secs // 2):
    path = q(u + 0x38)
    level = q(q(q(path + 0x20) + 0x18) + 0x90)
    r, n, loaded, seen = q(level + 0x10), 0, 0, set()
    while r and r not in seen and n < 2000:
        seen.add(r); n += 1; loaded += bool(q(r + 0x58)); r = q(r + 0x48)
    x = struct.unpack("<H", rd(path + 2, 2) or b"\0\0")[0]; y = struct.unpack("<H", rd(path + 6, 2) or b"\0\0")[0]
    print(time.strftime("%H:%M:%S"), f"level={d(level + 0x1F8)} rooms={n} loaded={loaded} pos=({x},{y})", flush=True)
    time.sleep(2)

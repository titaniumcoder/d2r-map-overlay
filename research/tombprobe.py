"""Read-only: the real Tal Rasha's Tomb of the game (ActMisc +0x120, D2R `D2DrlgStrc.StaffTombLevel`).

Usage: sudo python3 -I research/tombprobe.py <pid> <unit table address, hex>
(the overlay log prints the unit table address). For each player unit with an act: act +0x70
-> ActMisc; prints difficulty (+0x830, u16) and the staff tomb level (+0x120, u32, 66..72 in Act 2).
2026-11-18: 72 in a Normal game where Duriel's Lair had been entered from tomb 72.
"""
import struct
import sys

pid, table = int(sys.argv[1]), int(sys.argv[2], 16)
m = open(f"/proc/{pid}/mem", "rb", buffering=0)


def rd(a, n):
    try:
        m.seek(a)
        return m.read(n)
    except OSError:
        return None


def q(a):
    b = rd(a, 8)
    return struct.unpack("<Q", b)[0] if b else 0


for slot in range(128):
    u = q(table + slot * 8)
    if not u:
        continue
    typ, txt, uid = struct.unpack("<III", rd(u, 12))
    act = q(u + 0x20)
    if typ != 0 or not act:
        continue
    misc = q(act + 0x70)
    diff = struct.unpack("<H", rd(misc + 0x830, 2))[0]
    tomb = struct.unpack("<I", rd(misc + 0x120, 4))[0]
    print(f"unit {u:#x} id {uid} act {act:#x} misc {misc:#x} difficulty {diff} staff tomb level {tomb}")

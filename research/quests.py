"""Probe: the client's quest flags (current game). Read-only.

PlayerData +0x40/+0x48/+0x50 hold per-difficulty quest BitBuffers, but on the client they stay
all zero (they are the server's copy). The client's own record is a global in .data: a
pointer to a BitBuffer header {+0x00 buffer ptr, +0x08 u32 bit count 0x300, +0x28 u64 byte
size 0x60}; the buffer is 48 little-endian u16 words, one per quest (classic d2s layout:
word 1 Den of Evil ... word 35 Siege on Harrogath, 41 Akara respec). The next .data slot holds
a second header of the same shape (a second record, meaning unknown). Live 2026-11-18: .data
rva 0x1edaf78 / 0x1edaf80 for build 6ad81202-279c000 (unit table rva 0x1ebe5f0).

Bits per word (D2MOO QFLAG): 0 reward granted (quest done, reward used), 1 reward pending
(done, reward not taken: Charsi's imbue, Larzuk's socket, Akara's respec), 2 started, ...,
12 primary goal done, 13 completed now, 14 completed before.

Usage: sudo python3 -I research/quests.py <pid>
"""
import struct
import sys

NAMES = {
    0: "Act I intro (Warriv)", 1: "Den of Evil", 2: "Sisters' Burial Grounds", 3: "Tools of the Trade",
    4: "The Search for Cain", 5: "The Forgotten Tower", 6: "Sisters to the Slaughter", 7: "Act I done",
    8: "Act II intro", 9: "Radament's Lair", 10: "The Horadric Staff", 11: "Tainted Sun",
    12: "Arcane Sanctuary", 13: "The Summoner", 14: "The Seven Tombs", 15: "Act II done",
    16: "Act III intro", 17: "Lam Esen's Tome", 18: "Khalim's Will", 19: "Blade of the Old Religion",
    20: "The Golden Bird", 21: "The Blackened Temple", 22: "The Guardian", 23: "Act III done",
    24: "Act IV intro", 25: "The Fallen Angel", 26: "Terror's End", 27: "Hell's Forge", 28: "Act IV done",
    35: "Siege on Harrogath", 36: "Rescue on Mount Arreat", 37: "Prison of Ice",
    38: "Betrayal of Harrogath", 39: "Rite of Passage", 40: "Eve of Destruction", 41: "Akara respec",
}

pid = int(sys.argv[1])
f = open(f"/proc/{pid}/mem", "rb")


def rd(a, n):
    f.seek(a)
    return f.read(n)


def u32(a):
    return struct.unpack("<I", rd(a, 4))[0]


def u64(a):
    return struct.unpack("<Q", rd(a, 8))[0]


# The image: a memfd inode with a fully readable rw-s alias (MZ at its start) and a live view.
maps = [line.split() for line in open(f"/proc/{pid}/maps")]
alias = live = None
for m in maps:
    a, b = (int(x, 16) for x in m[0].split("-"))
    if m[1] == "rw-s" and int(m[2], 16) == 0 and 0x2600000 <= b - a <= 0x2A00000 and rd(a, 2) == b"MZ":
        alias, inode = a, m[4]
        live = min(int(n[0].split("-")[0], 16) for n in maps if n[4] == inode and n[1] != "rw-s")
print(f"image alias {alias:#x} live {live:#x}")
h = rd(alias, 0x400)
pe = struct.unpack_from("<I", h, 0x3C)[0]
count, opt = struct.unpack_from("<H", h, pe + 6)[0], struct.unpack_from("<H", h, pe + 20)[0]
for i in range(count):
    at = pe + 24 + opt + i * 40
    if h[at:at + 5] == b".data":
        vsize, va = struct.unpack_from("<II", h, at + 8)
data = rd(alias + va, vsize)
hits = []
for o in range(0, len(data) - 8, 8):
    p = struct.unpack_from("<Q", data, o)[0]
    if not 0x10000 <= p < 0x7FFFFFFF0000:
        continue
    try:
        hdr = rd(p, 0x30)
    except OSError:
        continue
    if len(hdr) == 0x30 and struct.unpack_from("<I", hdr, 8)[0] == 0x300 and struct.unpack_from("<Q", hdr, 0x28)[0] == 0x60:
        hits.append(va + o)
print("header slots in .data (rva):", [hex(x) for x in hits])
for rva in hits:
    hdr = u64(live + rva)
    words = struct.unpack("<48H", rd(u64(hdr), 0x60))
    print(f"\nrva {rva:#x} -> header {hdr:#x}")
    for i, w in enumerate(words):
        if w or i in NAMES:
            state = "reward used" if w & 1 and not w & 2 else "reward pending" if w & 2 else "not done" if w else "-"
            print(f"  {i:2d} {NAMES.get(i, '?'):28s} {w:#06x} {state}")

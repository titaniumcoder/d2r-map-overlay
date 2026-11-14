"""Read-only scan of D2R's decrypted code for D2RMH's 5 memory signatures.
Usage: sudo python3 -I livescan.py <pid>
Reads /proc/<pid>/maps and /proc/<pid>/mem; never writes anything."""
import re
import sys

pid = int(sys.argv[1])
SIGS = {
    "hashTable   48 03 C7 49 8B 8C C6": rb"\x48\x03\xC7\x49\x8B\x8C\xC6",
    "uiBase      48 89 45 B7 4C 8D 35 ????": rb"\x48\x89\x45\xB7\x4C\x8D\x35.{4}",
    "isExpansion 48 8B 05 ???? 48 8B D9 F3 0F 10 50": rb"\x48\x8B\x05.{4}\x48\x8B\xD9\xF3\x0F\x10\x50",
    "roster      02 45 33 D2 4D 8B": rb"\x02\x45\x33\xD2\x4D\x8B",
    "gameInfo    44 88 25 ???? 66 44 89 25": rb"\x44\x88\x25.{4}\x66\x44\x89\x25",
}

maps = []
with open(f"/proc/{pid}/maps") as f:
    for line in f:
        parts = line.split(maxsplit=5)
        lo, hi = (int(x, 16) for x in parts[0].split("-"))
        path = parts[5].strip() if len(parts) > 5 else ""
        maps.append((lo, hi, parts[1], path))

print("== mappings mentioning D2R.exe:")
for lo, hi, perm, path in maps:
    if "D2R.exe" in path:
        print(f"  {lo:#x}-{hi:#x} {perm} {(hi - lo) >> 10} KiB")

# The decrypted .text is ~22 MiB; scan every executable region of 1 MiB or more.
exe = [(lo, hi, perm, path) for lo, hi, perm, path in maps if "x" in perm and hi - lo >= (1 << 20)]
print("== executable regions >= 1 MiB:")
for lo, hi, perm, path in exe:
    print(f"  {lo:#x}-{hi:#x} {perm} {(hi - lo) >> 20} MiB {path[-60:]}")

hits = {k: [] for k in SIGS}
with open(f"/proc/{pid}/mem", "rb", buffering=0) as mem:
    for lo, hi, perm, path in exe:
        try:
            mem.seek(lo)
            data = mem.read(hi - lo)
        except OSError as e:
            print(f"  skip {lo:#x}: {e}")
            continue
        for name, pat in SIGS.items():
            hits[name] += [lo + m.start() for m in re.finditer(pat, data, re.S)]

print("== signature hits:")
for name, addrs in hits.items():
    print(f"  {name}: {len(addrs)} hit(s) {[hex(a) for a in addrs[:3]]}")

"""Compare walkability at the player's tile for the candidate seed vs random seeds, per difficulty."""
import json, os, random, struct, subprocess, sys
level, cand = int(sys.argv[1]), int(sys.argv[2])
pts = [tuple(map(int, l.split()[1:3])) for l in open(sys.argv[3]) if l.startswith(str(level) + ' ')]
home = os.path.expanduser("~")
g = f"{home}/.local/share/Steam/compatibilitytools.d/GE-Proton11-12-x86_64/files"
env = dict(os.environ, WINEPREFIX=f"{home}/Games/battlenet/prefix", WINEDLLPATH=f"{g}/lib/wine", WINEDEBUG="-all")
p = subprocess.Popen([f"{g}/bin/wine", r"C:\D2RMH\d2mapapi_piped.exe", r"C:\Diablo II"], stdin=subprocess.PIPE,
                     stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, env=env, cwd=f"{home}/Games/battlenet/prefix/drive_c/D2RMH")
def rd(n):
    b = b""
    while len(b) < n:
        c = p.stdout.read(n - len(b))
        if not c: sys.exit("pipe closed")
        b += c
    return b
assert struct.unpack("<i", rd(4))[0] == 0
def grid(seed, diff):
    p.stdin.write(struct.pack("<III", seed, diff, level)); p.stdin.flush()
    j = json.loads(rd(struct.unpack("<I", rd(4))[0]))
    rows, row = [], []
    for v in j["mapData"]:
        if v == -1: rows.append(row); row = []
        else: row.append(v)
    def walk(x, y):  # run lengths alternate blocked/walkable, starting blocked
        if y < 0 or y >= len(rows): return None
        pos, blocked = 0, True
        for n in rows[y]:
            if pos <= x < pos + n: return not blocked
            pos += n; blocked = not blocked
        return False
    return j, walk
def score(seed, diff):
    j, walk = grid(seed, diff)
    return sum(bool(walk(x - j["offset"]["x"], y - j["offset"]["y"])) for x, y in pts)
best = []
for cs in [int(l) for l in open("cands.txt")]:
    sc = score(cs, 0); best.append((sc, cs)); print(cs, f"{sc}/{len(pts)}", flush=True)
print("random:", [score(r, 0) for r in random.Random(3).sample(range(1, 2**31), 12)])
p.stdin.close(); p.kill()

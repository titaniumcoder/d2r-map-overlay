"""Ask d2mapapi_piped (under GE-Proton wine) for a level and print its bounds."""
import json, os, struct, subprocess, sys
seed, level = int(sys.argv[1]), int(sys.argv[2])
home = os.path.expanduser("~")
g = f"{home}/.local/share/Steam/compatibilitytools.d/GE-Proton11-12-x86_64/files"
env = dict(os.environ, WINEPREFIX=f"{home}/Games/battlenet/prefix", WINEDLLPATH=f"{g}/lib/wine", WINEDEBUG="-all")
p = subprocess.Popen([f"{g}/bin/wine", r"C:\D2RMH\d2mapapi_piped.exe", r"C:\Diablo II"],
                     stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, env=env,
                     cwd=f"{home}/Games/battlenet/prefix/drive_c/D2RMH")
def rd(n):
    b = b""
    while len(b) < n:
        c = p.stdout.read(n - len(b))
        if not c: sys.exit("pipe closed")
        b += c
    return b
init = struct.unpack("<i", rd(4))[0]
if init != 0:
    sys.exit("init failed: " + rd(struct.unpack("<I", rd(4))[0]).decode())
for diff in (0, 1, 2):
    p.stdin.write(struct.pack("<III", seed, diff, level)); p.stdin.flush()
    j = json.loads(rd(struct.unpack("<I", rd(4))[0]))
    if "error" in j: print(diff, j["error"]); continue
    keys = {k: (v if not isinstance(v, (list, dict)) or len(str(v)) < 120 else f"<{type(v).__name__} len {len(v)}>") for k, v in j.items()}
    print(f"difficulty {diff}: {keys}")
p.stdin.close(); p.kill()

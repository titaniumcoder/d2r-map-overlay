"""Read back ui-probe.jsonl (the reader's UI probe, `D2R_UI_PROBE=1`) to find the UI state bytes.

Usage: python3 -I research/ui_probe.py ~/.local/share/d2r-map-overlay/ui-probe.jsonl [since_unix_secs]
Prints the state timeline (trade window item count, stash tab count, inventory cells when they
change), then every `.data` byte and `Inventory` head byte with its changes in time order,
fewest changes first: a byte whose changes line up with the recorded actions (stash open/close,
tab clicks, store tab clicks) is the candidate.
"""
import collections
import json
import sys

path = sys.argv[1]
since = int(sys.argv[2]) if len(sys.argv) > 2 else 0
data = collections.defaultdict(list)
inv = collections.defaultdict(list)
start = None
last_state = None
for line in open(path):
    ev = json.loads(line)
    t = ev["t"]
    if t < since:
        continue
    if ev.get("start"):
        print(f"{t}: probe start, .data rva {ev['data_rva']:#x} len {ev['data_len']:#x}")
        start = t
        continue
    state = (ev["shop"], ev["tabs"], ev["cells"])
    if state != last_state:
        print(f"{t}: shop {ev['shop']} items, {ev['tabs']} stash tabs, inventory: {ev['cells']}")
        last_state = state
    for rva, old, new in ev["data"]:
        data[rva].append((t, old, new))
    for off, old, new in ev["inv"]:
        inv[off].append((t, old, new))


def dump(title, changes, fmt):
    print(f"\n{title}: {len(changes)} bytes changed")
    for key, seq in sorted(changes.items(), key=lambda kv: (len(kv[1]), kv[0])):
        steps = " ".join(f"{t % 10000}:{old}>{new}" for t, old, new in seq)
        print(f"  {fmt(key)} ({len(seq)}): {steps}")


dump(".data", data, lambda rva: f"rva {rva:#x}")
dump("Inventory head", inv, lambda off: f"+{off:#04x}")

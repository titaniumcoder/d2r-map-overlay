"""Read back ui-probe.jsonl (the reader's UI probe, `D2R_UI_PROBE=1`) to find the UI state bytes.

Usage: python3 -I research/ui_probe.py ~/.local/share/d2r-map-overlay/ui-probe.jsonl [since_unix_secs]
Prints the state timeline (trade window item count, stash tab count, the candidate panel flags
that are set, inventory cells when they change), then per source every changed byte or qword
with its changes in time order, fewest changes first: one whose changes line up with the
recorded actions (stash open/close, tab clicks, store tab clicks) is the candidate. Times are
the unix seconds mod 10000. Lines of the first probe version (2026-11-22 12:28) have only
`data` and `inv`, and their `data` was cut at 400 changes ("more"): changes at high rvas in
busy ticks (a window opening) are missing there.
"""
import collections
import json
import sys

path = sys.argv[1]
since = int(sys.argv[2]) if len(sys.argv) > 2 else 0
sources = {name: collections.defaultdict(list) for name in ("data", "ptrs", "panels", "heap", "inv")}
last_state = None
for line in open(path):
    ev = json.loads(line)
    t = ev["t"]
    if t < since:
        continue
    if ev.get("start"):
        print(f"{t}: probe start, .data rva {ev['data_rva']:#x} len {ev['data_len']:#x}")
        continue
    state = (ev["shop"], ev["tabs"], ev.get("open"), ev["cells"])
    if state != last_state:
        print(
            f"{t}: shop {ev['shop']} items, {ev['tabs']} stash tabs, open [{ev.get('open', '?')}],"
            f" inventory: {ev['cells']}"
        )
        last_state = state
    if ev.get("more"):
        print(f"{t}: {ev['more']} .data changes cut")
    for name, changes in sources.items():
        for change in ev.get(name, []):
            *key, old, new = change
            changes[tuple(key)].append((t, old, new))


def show(name, key):
    if name == "data":
        return f"rva {key[0]:#x}"
    if name == "ptrs":
        return f"qword rva {key[0]:#x}"
    if name == "panels":
        return f"panels +{key[0]:#04x}"
    if name == "heap":
        return f"*(rva {key[0]:#x}) +{key[1]:#05x}"
    return f"Inventory +{key[0]:#04x}"


def value(name, v):
    return f"{v:#x}" if name == "ptrs" else str(v)


for name, changes in sources.items():
    print(f"\n{name}: {len(changes)} changed")
    for key, seq in sorted(changes.items(), key=lambda kv: (len(kv[1]), kv[0])):
        steps = " ".join(f"{t % 10000}:{value(name, old)}>{value(name, new)}" for t, old, new in seq)
        print(f"  {show(name, key)} ({len(seq)}): {steps}")

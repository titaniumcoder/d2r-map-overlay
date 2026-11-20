"""Generate src/drops_data.rs from blizzhackers/d2data (D2R JSON game tables).

Usage: python3 -I research/gen_treasure.py <dir with the tables below> > src/drops_data.rs
Fetch the tables with:
  for f in treasureclassex monstats superuniques levels itemtypes armor weapons misc; do
    gh api repos/blizzhackers/d2data/contents/json/$f.json -H "Accept: application/vnd.github.raw" > $f.json
  done

What can drop from each farmed boss, per difficulty: every item code its treasure class (TC)
reaches, flattened offline. Rules (d2data / the game's TC logic):
- The TC is the boss's REPEAT-run one: monstats `TreasureClass` / `(N)` / `(H)` for quest
  bosses (the first quest kill uses `TreasureClassQuest`; Andariel has `Andarielq` in every
  column since D2R 2.4), superuniques.json `TC` / `TC(N)` / `TC(H)` for superuniques. The
  terror-zone (`Desecrated`) columns are left out.
- A TC with a `group` is upgraded to the highest-`level` TC of its group with level <= the
  monster level (superuniques' "Act N Super X" TCs; only the boss's own TC, not sub-TCs).
- Sub-TCs are followed through every `ItemN` with `ProbN > 0`; gold (`gld`) is skipped.
- The auto-generated groups `armoN` / `weapN` / `bowN` / `meleN` (item types with
  itemtypes.txt `TreasureClass = 1`, their sub-types included through `Equiv1` / `Equiv2`)
  hold the spawnable armor and weapons rows of qlvl `level` in N-2..N (steps of 3).
- Monster level = item level of the drop: quest bosses (monstats `boss = 1`) have their fixed
  monstats `Level` / `Level(N)` / `Level(H)`; a superunique has its monstats `Level` + 3 in
  Normal and its area's `MonLvlEx(N)` / `MonLvlEx(H)` + 3 in Nightmare / Hell.
The Countess also has a dedicated rune TC beside her item TC (`Countess` = `Countess Item` +
`Countess Rune`, each `(N)` / `(H)`): Picks 3 of `Runes 4` / `Runes 8` / `Runes 12` against
NoDrop 5. Those are her reliable runes; `COUNTESS_RUNES` lists, per difficulty, each rune it
reaches with the chance of at least one per kill, 1 - (1 - p)^picks, p the chained per-pick
chance (solo: NoDrop as listed, no /players). Higher runes come only through her item TC.
Unique / set items are matched against these codes by the playstyle (`playstyle::worth`)
with the existing name / code / qlvl tables (item_names_data.rs, item_levels_data.rs).
"""
import json
import math
import re
import sys

src = sys.argv[1]


def table(name):
    t = json.load(open(f"{src}/{name}.json"))
    return t if isinstance(t, list) else list(t.values())


tcs = {r["Treasure Class"]: r for r in table("treasureclassex") if r.get("Treasure Class")}
monstats = {r["Id"]: r for r in table("monstats") if r.get("Id")}
supers = {r["Superunique"]: r for r in table("superuniques") if r.get("Superunique")}
levels = {r["Id"]: r for r in table("levels") if "Id" in r}
itemtypes = {r["Code"]: r for r in table("itemtypes") if r.get("Code")}
equipment = [r for r in table("armor") + table("weapons") if r.get("code")]
codes = {r["code"] for r in equipment} | {r["code"] for r in table("misc") if r.get("code")}

# Quest bosses (levels_data::BOSSES) by monstats Id, then the superuniques by
# superuniques.json name: (shown name, source, key).
BOSSES = [
    ("Andariel", "mon", "andariel"),
    ("Duriel", "mon", "duriel"),
    ("Mephisto", "mon", "mephisto"),
    ("Diablo", "mon", "diablo"),
    ("Baal", "mon", "baalcrab"),
    ("Blood Raven", "mon", "bloodraven"),
    ("Griswold", "mon", "griswold"),
    ("Radament", "mon", "radament"),
    ("Summoner", "mon", "summoner"),
    ("Izual", "mon", "izual"),
    ("Nihlathak", "mon", "nihlathakboss"),
    ("The Countess", "super", "The Countess"),
    ("Bishibosh", "super", "Bishibosh"),
    # One entry for the Council: all six share the `Council` TC; Toorc Icefist stands in
    # Travincal (the Durance's three are a few levels higher).
    ("The Council", "super", "Toorc Icefist"),
    ("Pindleskin", "super", "Pindleskin"),
    ("Shenk the Overseer", "super", "Siege Boss"),
    ("Eldritch the Rectifier", "super", "Megaflow Rectifier"),
    ("Thresh Socket", "super", "Threash Socket"),
]
SUFFIX = ["", "(N)", "(H)"]


def type_chain(code):
    """The item type `code` and every type it counts as (Equiv1 / Equiv2, transitively)."""
    out, todo = set(), [code]
    while todo:
        c = todo.pop()
        if c and c not in out and c in itemtypes:
            out.add(c)
            todo += [itemtypes[c].get("Equiv1"), itemtypes[c].get("Equiv2")]
    return out


tc_types = {c for c, r in itemtypes.items() if r.get("TreasureClass") == 1}
auto = {}
for r in equipment:
    if r.get("spawnable") != 1:
        continue
    group = max(3, math.ceil(int(r["level"]) / 3) * 3)
    for t in type_chain(r["type"]) & tc_types:
        auto.setdefault(f"{t}{group}", set()).add(r["code"])

unknown = set()


def flatten(name, seen=()):
    if name in seen:
        return set()
    if name in tcs:
        row, out = tcs[name], set()
        for i in range(1, 11):
            item, prob = row.get(f"Item{i}"), row.get(f"Prob{i}")
            if item and isinstance(prob, int) and prob > 0:
                out |= flatten(item, (*seen, name))
        return out
    if name in auto:
        return set(auto[name])
    code = name.strip('"').split(",")[0]
    if code == "gld":
        return set()
    if code in codes:
        return {code}
    if re.fullmatch(r"[a-z]+\d+", code):
        return set()  # an auto group with no spawnable item (e.g. armo87)
    unknown.add(name)
    return set()


def upgrade(name, mlvl):
    row = tcs[name]
    group, level = row.get("group"), row.get("level")
    if group is None or level is None:
        return name
    better = [(r["level"], n) for n, r in tcs.items() if r.get("group") == group and level < (r.get("level") or 0) <= mlvl]
    return max(better)[1] if better else name


rows = []
for shown, source, key in BOSSES:
    lvls, sets, roots = [], [], []
    for d, sfx in enumerate(SUFFIX):
        if source == "mon":
            m = monstats[key]
            assert m.get("boss") == 1, key
            mlvl = int(m[f"Level{sfx}"])
            tc = m[f"TreasureClass{sfx}"]
        else:
            s = supers[key]
            m = monstats[s["Class"]]
            mlvl = 3 + (int(m["Level"]) if d == 0 else int(levels[s["areaId"]][f"MonLvlEx{sfx}"]))
            tc = upgrade(s[f"TC{sfx}"], mlvl)
        lvls.append(mlvl)
        roots.append(tc)
        sets.append(sorted(flatten(tc)))
    rows.append((shown, lvls, sets, roots))



def pick_chances(name):
    """Per-pick chance of each item code under TC `name`, NoDrop included at every level."""
    row = tcs[name]
    entries = [(row[f"Item{i}"], row[f"Prob{i}"]) for i in range(1, 11) if row.get(f"Item{i}") and isinstance(row.get(f"Prob{i}"), int) and row[f"Prob{i}"] > 0]
    total = sum(p for _, p in entries) + int(row.get("NoDrop") or 0)
    out = {}
    for item, prob in entries:
        if item in tcs:
            assert tcs[item].get("Picks") == 1, f"{item}: a chained TC must pick once"
            for code, p in pick_chances(item).items():
                out[code] = out.get(code, 0) + p * prob / total
        else:
            out[item] = out.get(item, 0) + prob / total
    return out


countess_runes = []
for sfx in SUFFIX:
    root = tcs[supers["The Countess"][f"TC{sfx}"]]
    rune_tcs = [root[f"Item{i}"] for i in range(1, 11) if "Rune" in (root.get(f"Item{i}") or "")]
    assert len(rune_tcs) == 1, rune_tcs
    picks = tcs[rune_tcs[0]]["Picks"]
    assert picks > 0, rune_tcs
    chances = pick_chances(rune_tcs[0])
    assert all(re.fullmatch(r"r\d\d", c) for c in chances), chances
    countess_runes.append((rune_tcs[0], [(c, 100 * (1 - (1 - chances[c]) ** picks)) for c in sorted(chances)]))

assert not unknown, f"unresolved TC entries: {sorted(unknown)}"

lists = []
for _, _, sets, _ in rows:
    for s in sets:
        if s not in lists:
            lists.append(s)


def rust_str(s):
    return json.dumps(s, ensure_ascii=False)


print("//! Generated by `research/gen_treasure.py` from `blizzhackers/d2data` (D2R tables). Do not edit by hand.")
print("//! What can drop from the farmed bosses: the item codes their repeat-run treasure class")
print("//! reaches, flattened, per difficulty. See the generator for the rules.")
for i, s in enumerate(lists):
    print()
    print(f"/// Drop list {i}: item codes, sorted.")
    print("#[rustfmt::skip] // generated; one long line per list")
    print(f"const D{i}: &[&str] = &[{', '.join(rust_str(c) for c in s)}];")
print()
print("/// One boss: (name, level per difficulty, drop codes per difficulty); see [`BOSS_DROPS`].")
print("pub type BossDrops = (&'static str, [u32; 3], [&'static [&'static str]; 3]);")
print()
print("/// (boss, monster level = drop item level per difficulty [Normal, Nightmare, Hell], its")
print("/// sorted drop codes per difficulty). The TC each list was flattened from is noted beside it.")
print("pub const BOSS_DROPS: &[BossDrops] = &[")
for shown, lvls, sets, roots in rows:
    ids = [f"D{lists.index(s)}" for s in sets]
    print(f"    // {' / '.join(roots)}")
    print(f"    ({rust_str(shown)}, [{', '.join(map(str, lvls))}], [{', '.join(ids)}]),")
print("];")
print()
print("/// The Countess's own rune TC per difficulty [Normal, Nightmare, Hell]: each rune code it")
print("/// reaches, by rank, with the chance in percent of at least one per kill (solo). Her higher")
print("/// runes come only through her item TC (in [`BOSS_DROPS`]).")
print("#[rustfmt::skip] // generated; one line per difficulty")
print("pub const COUNTESS_RUNES: [&[(&str, f64)]; 3] = [")
for tc, runes in countess_runes:
    print(f"    // {tc}")
    print(f"    &[{', '.join(f'({rust_str(c)}, {p:.4f})' for c, p in runes)}],")
print("];")

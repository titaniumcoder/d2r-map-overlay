"""Turn a researched build guide (JSON, see ~/.config/d2r-map-overlay/builds/*.source.json)
into an agent-authored build file (builds/<slug>.toml) for the overlay's dashboard.

Usage:
    python3 -I research/gen_build.py SOURCE.json OUT.toml [--leveling LEVELING.json]
                                     [--prerequisites NEXT.json]

Deterministic: the same inputs give the same file byte for byte, so it can be re-run whenever
the research JSON changes. Per-build settings (display name, class, damages, stage chain)
live in BUILDS below, keyed by the source file's stem (`druid-leveling.source` counts as
`druid-leveling`).

- `--leveling` (Tornado / Lightning Fury endgame): the leveling guide's post-respec plan
  (level 38-75) and its tips for those levels are merged in front of the endgame plan; that
  guide's respec IS the switch into this stage (its `respec_note` in BUILDS is the switch note).
- BUILDS `switch_requires` (Summoner Druid: the Fury switch conditions): item groups that must
  all be met before the switch to `next`; each entry is a plain name (a group of one) or a dict
  {label, any, count?} (met when the copies owned of `any` sum to `count`, default 1), emitted as
  a top-level `switch_requires = [...]` (strings, inline tables) after `auto_switch`, only when set.
- `--prerequisites` (the stage before a manual switch): the next stage's `prerequisites`
  become this stage's `switch_note`.
- `skill_plan.respec_after_leveling` (Blizzard Sorceress): with `--leveling`, a second respec
  into `planner_allocations[allocation]` at `level` follows the leveling plan.
- BUILDS `keep_standard_first` (Blizzard Sorceress, the MF farmer): this guide's standard
  pieces stay first even when the leveling guide lists them too.

Skill names are checked against src/skills_data.rs (class skills); mismatches are reported
on stderr and in the exit summary.
"""

import json
import os
import re
import sys

# Per-build settings by source stem. `next` is the next stage's slug (lowercased name).
BUILDS = {
    # Agent-computed from the D2R game tables, not from a guide (user, 2026-11-19: "guides are
    # practically all out of date"). A complete summoner 1-99 (user: "he can be summoner till 99
    # if he doesn't find what he needs"); the switch into Fury (werewolf) is possible from 75 and
    # only with the switch gear in switch_requires.
    "summoner-druid": dict(
        name="Summoner Druid", cls="druid", damages=["physical", "cold"], default=True,
        next="fury druid", switch_level=75, auto_switch=True,
        # The user's conditions for the Fury switch (2026-11-19). "Cannot Be Frozen" and the merc
        # helm group are computed from d2data: every unique/set item (base props) and complete
        # runeword with the `nofreeze` / `lifesteal` property; items of another class's item type
        # (itemtypes.json `Class`) are out: Arreat's Face, Demonhorn's Edge (barbarian), Cerebus'
        # Bite (druid pelt, a merc can't wear class items). No circlet has life steal.
        switch_requires=[
            dict(label="Cannot Be Frozen", any=[
                "Raven Frost", "Darksight Helm", "Duriel's Shell", "Hawkmail", "Kira's Guardian",
                "Lava Gout", "Rune Master", "Sureshrill Frost", "The Gladiator's Bane",
                "The Jade Tan Do", "The Spirit Shroud", "Tyrael's Might", "Death's Guard",
                "Horazon's Legacy", "Trang-Oul's Girth", "Eternity", "Hearth", "Rhyme",
                "Voice of Reason", "Wisdom", "Wrath"]),
            dict(label="Powerful weapon", any=[
                "Ribcracker", "Buriza-Do Kyanon", "Tomb Reaver", "Grief", "Beast",
                "Breath of the Dying", "Eternity", "Oath", "Unbending Will", "Hustle (weapon)"]),
            "Harmony",
            dict(label="Physical immunes", any=["Lawbringer"], count=2),
            "Smoke",
            dict(label="Merc helm with life leech", any=[
                "Andariel's Visage", "Crown of Thieves", "Isenhart's Horns", "Stealskull",
                "Tal Rasha's Horadric Crest", "Undead Crown", "Vampire Gaze", "Wormskull", "Bulwark"]),
        ],
        notes="Summoner Druid 1-99 (Ravens, Spirit Wolves, Dire Wolves, Grizzly, Oak Sage). From level 75 an optional skill reset into Fury (next stage), only once the switch gear is owned: a Cannot Be Frozen item, a powerful weapon, Harmony, two Lawbringers, Smoke and a life-leech merc helm; without it the summoner goes on to 99. Numbers computed from the D2R game tables (blizzhackers/d2data), not from a guide.",
        respec_note=("optional, from level 75 once the switch gear is owned (Cannot Be Frozen item, powerful weapon such as "
                     "Ribcracker or Grief, Harmony for speed on weapon swap, 2x Lawbringer for physical immunes, Smoke for the "
                     "merc, a life-leech merc helm): respec to Fury at Akara (free reset of any difficulty "
                     "whose Den of Evil reward is unused): Fury 20, Werewolf 20, Feral Rage 20, Lycanthropy 20 (460 s form), "
                     "Rabies 1, Oak Sage 5 = 86 points (74 levels + 12 quest points, Normal/Nightmare/Hell); keep the Cannot "
                     "Be Frozen item, Jalal's Mane and String of Ears on, swap the Spirit sword for the powerful weapon and "
                     "Bloodfist for Laying of Hands"),
    ),
    "fury-druid": dict(
        name="Fury Druid", cls="druid", damages=["physical"],
        notes="Fury Werewolf from the optional skill reset at 75 (Summoner Druid with Raven Frost); standard = the user's core (Ribcracker, Lawbringer, Fortitude, Smoke, Bulwark, Jalal's Mane, Laying of Hands, Raven Frost). Durations, breakpoints and skill order computed from the D2R game tables, not from a guide.",
    ),
    # The leveling guide respecs into Blizzard at level 26, after the Normal Throne waves and
    # before entering Nightmare. This sorceress is the account's magic-find farmer (user, 2026-11-19).
    "sorceress-leveling": dict(
        name="Sorceress Leveling", cls="sorceress", damages=["lightning", "cold"], default=True,
        next="blizzard sorceress", switch_level=26, auto_switch=True,
        notes="Maxroll Sorceress Leveling (Teo1904): Charged Bolt / Nova to level 26, then respec to Blizzard (next stage). The account's magic-find farm character: cheap MF pieces (Chance Guards, Nagelring, Goldwrap, Splendor) are listed as alternatives.",
        respec_note=("respec to Blizzard at level 26, after the Normal Throne of Destruction waves and before entering Nightmare (Akara): "
                     "Static Field 5, Teleport 1, Frozen Armor 1, Ice Blast 7, Glacial Spike 9, Blizzard 3, plus 1 each in Telekinesis, "
                     "Ice Bolt and Frost Nova; keep the Nightmare Den of Evil point for level 30 (Blizzard + Cold Mastery)"),
    ),
    "blizzard-sorceress": dict(
        name="Blizzard Sorceress", cls="sorceress", damages=["cold", "fire"], keep_standard_first=True,
        notes="Maxroll Blizzard Sorceress, geared as the account's magic-find farm character: the guide's Magic Find variant is the standard setup (Harlequin Crest, Oculus, 3 Tal Rasha pieces, Chance Guards, War Traveler, MF rings/charms, 105% FCR); the damage Standard, Set build and Gear Options fold as alternatives, Starter stays early. Levels 26-75 follow the leveling guide's post-respec plan (Blizzard + Meteor), then a respec at 75.",
    ),
}

HERE = os.path.dirname(os.path.abspath(__file__))
CLASS_IDS = {"amazon": 0, "sorceress": 1, "necromancer": 2, "paladin": 3, "barbarian": 4,
             "druid": 5, "assassin": 6, "warlock": 7}
SLOTMAP = {"ring": "ring1"}
EXTRA = {"charm": "charms", "jewel": "jewels", "rune": "runes"}
ORDER = ["weapon", "offhand", "swap", "helm", "body", "gloves", "belt", "boots", "amulet", "ring1"]
MAX_POINTS = 20
# Known guide-text errors, fixed deterministically (build, level, skill) -> skill.
FIXES = {
    # The leveling guide says Firestorm at 27, which would push Firestorm to 21 (over the max);
    # the planner's skillProgression for that stage has Fissure there (see the entry's note).
    ("druid-leveling", "27", "Firestorm"): "Fissure",
}

problems = []


def q(s):
    return json.dumps(s, ensure_ascii=False)


def req_group(g):
    """One `switch_requires` entry: a plain name, or an inline table for a dict
    {label, any, count?} (count omitted when absent, the overlay's default is 1)."""
    if isinstance(g, str):
        return q(g)
    parts = [f"label = {q(g['label'])}", "any = [" + ", ".join(q(x) for x in g["any"]) + "]"]
    if "count" in g:
        parts.append(f"count = {int(g['count'])}")
    return "{ " + ", ".join(parts) + " }"


def class_skills(cls):
    """Skill names of class `cls` from the generated src/skills_data.rs."""
    text = open(os.path.join(HERE, "..", "src", "skills_data.rs"), encoding="utf-8").read()
    names = set()
    for m in re.finditer(r'name: "([^"]+)",\s*class: (\d+),', text):
        if int(m.group(2)) == CLASS_IDS[cls]:
            names.add(m.group(1))
    return names


def clean_skill(name):
    """'Oak Sage (max last)' -> 'Oak Sage'."""
    return re.sub(r"\s*\(.*?\)\s*$", "", name).strip()


# ---------------------------------------------------------------- skill plan
LEVEL_RE = re.compile(r"^~?(\d+)(?:\s*-\s*~?(\d+))?")


def level_range(text):
    m = LEVEL_RE.match(str(text).strip())
    if not m:
        return None
    a = int(m.group(1))
    return a, int(m.group(2) or a)


def allocations(entry, current):
    """The single-point allocations (skill names, in order) of one guide entry."""
    skill, points = entry["skill"], entry["points"]
    if isinstance(points, int):
        return [skill] * points
    p = str(points).strip()
    if p == "max":
        return [skill] * (MAX_POINTS - current.get(skill, 0))
    if p == "1 each":
        return [s.strip() for s in skill.split(",")]
    # "Tornado max (+5), Hurricane +5": alternate the parts.
    parts = []
    for part in p.split(","):
        m = re.match(r"\s*(.+?)\s+(?:max\s*)?\(?\+(\d+)\)?\s*$", part)
        if not m:
            problems.append(f"skill plan: unreadable points {p!r} for {skill!r}")
            return []
        parts.append([m.group(1).strip()] * int(m.group(2)))
    out = []
    while any(parts):
        for lst in parts:
            if lst:
                out.append(lst.pop())
    return out


def expand_plan(entries, build, known, current=None):
    """Guide deltas -> cumulative per-level targets: [(skill, points, by_level, note)].
    Quest entries take the next numbered entry's start level (approximate)."""
    current = dict(current or {})
    ranges = [level_range(e["level"]) for e in entries]
    out = []  # [skill, points, level, note]
    for i, e in enumerate(entries):
        e = dict(e)
        e["skill"] = FIXES.get((build, str(e["level"]), e["skill"]), e["skill"])
        rng = ranges[i]
        note = ""
        if rng is None:
            nxt = next((r for r in ranges[i + 1:] if r), None)
            prv = next((r for r in reversed(ranges[:i]) if r), None)
            lvl = nxt[0] if nxt else (prv[1] if prv else 1)
            rng = (lvl, lvl)
            note = f"quest point: {e['level']} (level approximate)"
        elif e.get("main_skill"):
            note = f"main skill from here: {e['main_skill']}"
        elif "~" in str(e["level"]) or "approx" in str(e["level"]):
            note = "levels approximate"
        allocs = allocations(e, current)
        a, b = rng
        span = b - a + 1
        first = True
        for k, skill in enumerate(allocs):
            skill = clean_skill(skill)
            if skill not in known:
                problems.append(f"{build}: skill {skill!r} is not a {BUILDS[build]['cls']} skill in skills_data.rs")
            current[skill] = current.get(skill, 0) + 1
            if current[skill] > MAX_POINTS:
                problems.append(f"{build}: {skill} would reach {current[skill]} at level {a}")
            lvl = a + (k * span) // len(allocs)
            if out and out[-1][0] == skill and out[-1][2] == lvl:
                out[-1][1] = current[skill]
            else:
                out.append([skill, current[skill], lvl, note if first else ""])
            first = False
    return out, current


def simulate(current, start_level, order, build, known, last_level=99):
    """One point per level from start_level+1, maxing `order` (skill, cap) in turn."""
    current = dict(current)
    out = []
    queue = []
    for skill, cap in order:
        skill = clean_skill(skill)
        if skill not in known:
            problems.append(f"{build}: skill {skill!r} is not a {BUILDS[build]['cls']} skill in skills_data.rs")
        queue += [skill] * max(0, cap - current.get(skill, 0))
    for level, skill in zip(range(start_level + 1, last_level + 1), queue):
        current[skill] = current.get(skill, 0) + 1
        out.append([skill, current[skill], level, ""])
    return out


def allocation_at(alloc, level, order, note):
    """A respec: the whole allocation `alloc` due at `level`, in `order` first."""
    names = [clean_skill(s) for s in order if clean_skill(s) in alloc]
    names += [s for s in alloc if s not in names]
    return [[s, alloc[s], level, note if i == 0 else ""] for i, s in enumerate(names)]


# ---------------------------------------------------------------- text helpers
def tip_range(text):
    m = re.search(r"(\d+)\s*-\s*(\d+)", text)
    if m:
        return f"{m.group(1)}-{m.group(2)}"
    m = re.search(r"(\d+)\+", text)
    return f"{m.group(1)}+" if m else None


def req_text(req):
    if isinstance(req, str) or req is None:
        return req or ""
    parts = []
    if "level" in req:
        parts.append(f"character level {req['level']}")
    if "sockets" in req:
        parts.append(f"{req['sockets']} sockets")
    if "item" in req:
        parts.append(req["item"])
    if not parts and "cube_description" in req:
        parts.append("Horadric Cube")
    return "; ".join(parts)


# Quest-reward recipes: the quest whose reward the recipe is (names from src/quests.rs QUESTS),
# so the dashboard can drop the recipe once the reward is used. Matched on the recipe name.
QUEST_REWARDS = [
    (re.compile(r"imbue|tools of the trade", re.I), "Tools of the Trade"),
    (re.compile(r"larzuk|siege on harrogath", re.I), "Siege on Harrogath"),
    (re.compile(r"\banya\b|prison of ice", re.I), "Prison of Ice"),
]


def quest_of(r):
    if r.get("quest"):
        return r["quest"]
    return next((q for pat, q in QUEST_REWARDS if pat.search(r["name"])), "")


def recipe_of(r):
    inputs = list(r.get("inputs") or [])
    req = r.get("req")
    notes = r.get("notes") or ""
    if isinstance(req, dict) and r["kind"] == "runeword" and not any(" " in i for i in inputs):
        bases = " / ".join(req.get("allowed_bases") or ["item"])
        line = f"non-magic {bases} with exactly {req.get('sockets', '?')} sockets"
        if r.get("bases_in_guide"):
            line += " (guide: " + ", ".join(r["bases_in_guide"]) + ")"
        inputs.append(line)
    if r.get("used_for"):
        notes = (notes + " " if notes else "") + "Used for: " + ", ".join(r["used_for"]) + "."
    return dict(name=r["name"], kind=r["kind"], inputs=inputs, result=r.get("result") or "",
                req=req_text(req), notes=notes, quest=quest_of(r))


def short_setup(setup):
    if not setup:
        return ""
    m = re.match(r"(lvl [\d+-]+)", setup)
    if m:
        return m.group(1)
    return setup if setup in ("starter", "standard") else "alternative"


# Target lines the user doesn't want (2026-11-17: "too experienced for that"; the breakpoint
# numbers are on the dashboard's guide chips and the main skill in the skill plan anyway).
DROP_TARGETS = ("Attributes", "Stats", "Resistances", "Immunities", "FCR", "FHR", "Block",
                "Main skill")


def keep_target(line):
    return not any(line.startswith(p + ":") for p in DROP_TARGETS)


def targets_blood_boil(t):
    a = t["attributes"]["planner_totals"]
    return [
        f"FCR: {t['fcr']['minimum']}, goal {t['fcr']['goal']}",
        "Stats: " + t["attributes"]["rule"] + f" (planner lvl 75: {a['starter_lvl75']['str']} str / {a['starter_lvl75']['vit']} vit; lvl 99: {a['standard_lvl99']['str']} str / {a['standard_lvl99']['vit']} vit)",
        "Fire pierce: " + t["fire_resistance_pierce"],
        f"Merc: starter {t['mercenary']['starter']}; endgame {t['mercenary']['standard']} — needs {t['mercenary']['needs']}",
        "Bound demon: " + t["bound_demon"],
    ]


def targets_generic(d):
    """Short target lines from a druid guide's `targets` (hardcore-only parts left out)."""
    t = d.get("targets") or {}
    out = []
    for key, v in t.items():
        if any(w in key for w in ("hardcore", "note", "desirable", "farming")):
            continue
        label = key.replace("_", " ").upper() if len(key) <= 3 else key.replace("_", " ").capitalize()
        if isinstance(v, str):
            out.append(f"{label}: {v}")
        elif isinstance(v, list) and v and isinstance(v[0], dict):
            if "when" in v[0]:
                out.append("Levels: " + "; ".join(f"{x['when']}: {x['target']}" for x in v))
            elif "skill" in v[0]:
                out.append("Main skill: " + "; ".join(f"{x['levels']} {x['skill']}" for x in v))
        elif isinstance(v, dict):
            bits = []
            if "breakpoints" in v:
                bits.append("breakpoints " + " / ".join(f"{b}%" for b in v["breakpoints"]))
            for k in ("rule", "target", "priority", "goal", "general", "standard"):
                if isinstance(v.get(k), str):
                    bits.append(v[k])
            if "minimum" in v and not isinstance(v["minimum"], dict):
                bits.append(f"at least {v['minimum']}" + ("%" if len(key) <= 3 else "") if isinstance(v["minimum"], int) else str(v["minimum"]))
            if bits:
                out.append(f"{label}: " + "; ".join(bits))
    m = d.get("merc") or {}
    if isinstance(m.get("type"), str):
        out.append("Merc: " + m["type"] + (f" — {m['why']}" if m.get("why") else ""))
    elif isinstance(m.get("type"), dict):
        out.append("Merc: " + "; ".join(f"{k} {v['merc']}" for k, v in m["type"].items() if isinstance(v, dict) and v.get("merc")))
    return out


def stats_lines(sp):
    if isinstance(sp, list):
        return [f"lvl {x['level']}: {x['allocation']}" for x in sp]
    if not isinstance(sp, dict):
        return []
    if "rule" in sp:
        lines = [sp["rule"]]
        for name, v in (sp.get("planner_points_spent") or {}).items():
            if "hardcore" in name:
                continue
            lines.append(f"planner {name.replace('_', ' ')}: " + " / ".join(f"{n} {k}" for k, n in v.items() if n))
        return lines
    return [f"{name.replace('_', ' ')}: {v['rule']}" for name, v in sp.items() if isinstance(v, dict) and v.get("rule")]


def switch_note_prereq(p):
    lvl = p.get("character_level")
    lvl = lvl.get("minimum") if isinstance(lvl, dict) else lvl
    full = (p.get("full_potential") or {}).get("must_have") or []
    full_items = [x["item"] if isinstance(x, dict) else x for x in full]
    minimum = (p.get("minimum_to_start") or {}).get("must_have") or []
    note = f"only once the gear is near-perfect (level {lvl}+): {', '.join(full_items)}"
    if minimum:
        note += f". Minimum kit: {', '.join(minimum)}"
    return note


# ---------------------------------------------------------------- main
def main(argv):
    args = argv[1:]
    opts = {}
    for flag in ("--leveling", "--prerequisites"):
        if flag in args:
            i = args.index(flag)
            opts[flag] = args[i + 1]
            del args[i:i + 2]
    src, out = args
    stem = os.path.basename(src).removesuffix(".json").removesuffix(".source")
    cfg = BUILDS[stem]
    d = json.load(open(src, encoding="utf-8"))
    known = class_skills(cfg["cls"])
    for i in d["items"]:
        i["name"] = i["name"].replace("@", " all res")

    def ref(i):
        return i["name"] + (f"@{i['req_level']}" if i.get("req_level") else "")

    leveling_items = any(str(i.get("setup") or "").startswith("lvl") for i in d["items"])
    if leveling_items:
        key = lambda i: (i["slot"], -(i.get("req_level") or 0), i.get("rank_in_slot") or 99)
    else:
        key = lambda i: (i["slot"], i.get("rank_in_slot") if i.get("rank_in_slot") is not None else 999)
    slots, merc, extras = {}, {}, {}
    for i in sorted(d["items"], key=key):
        s = i["slot"]
        if s.startswith("merc_"):
            lst = merc.setdefault(s[5:], [])
        elif s in EXTRA:
            lst = extras.setdefault(EXTRA[s], [])
        else:
            lst = slots.setdefault(SLOTMAP.get(s, s), [])
        if ref(i) not in lst:
            lst.append(ref(i))
    late = []
    if "--leveling" in opts:
        # The leveling guide's late gear (its runewords and magic pieces from level 13 on) is
        # what the character wears when it switches here at ~38: the low-level end of each
        # slot. Pieces this guide lists too move to the end; the others are appended. Only its
        # stage gear ('lvl a-b' setups): its Gear Options alternatives would push this guide's
        # own picks (Raven Frost, Highlord's Wrath, ...) to the end.
        lev = json.load(open(opts["--leveling"], encoding="utf-8"))
        late = [i for i in lev["items"]
                if i.get("kind") in ("runeword", "magic", "unique", "set")
                and str(i.get("setup", "")).startswith("lvl")
                and not str(i.get("setup", "")).startswith("lvl 1-")]
        late.sort(key=lambda i: -(i.get("req_level") or 0))
        names = {i["name"] for i in late} | {"Insight"}
        # ... unless this guide's own standard setup uses it and BUILDS says to keep that first
        # (Blizzard, MF-first: Spirit Monarch, merc Insight).
        if cfg.get("keep_standard_first"):
            names -= {i["name"] for i in d["items"] if short_setup(i.get("setup")) == "standard"}
        for lst in list(slots.values()) + list(merc.values()):
            lst.sort(key=lambda r: r.split("@")[0] in names)
        for i in late:
            s_ = i["slot"]
            lst = merc.setdefault(s_[5:], []) if s_.startswith("merc_") else slots.setdefault(SLOTMAP.get(s_, s_), [])
            if not any(r.split("@")[0] == i["name"] for r in lst):
                lst.append(ref(i))
    pieces = {}
    for i in sorted(d["items"], key=lambda i: i.get("rank_in_slot") if i.get("rank_in_slot") is not None else 999):
        if i["name"] in pieces:
            continue
        notes = [n for n in [i.get("notes") or ""] if n]
        if stem == "blood-boil":
            if "magic_find" in (i.get("setups") or []) and i["setup"] == "alternative":
                notes.insert(0, "Magic Find setup")
            setup = i.get("setup") or ""
        else:
            setup = short_setup(i.get("setup"))
            if setup == "alternative" and i.get("setup") not in (None, "alternative"):
                notes.insert(0, f"{i['setup'].replace('_', ' ')} setup")
            if i.get("merc_tier"):
                notes.insert(0, f"merc gear: {i['merc_tier']} game")
        pieces[i["name"]] = dict(kind=i.get("kind") or "", setup=setup, base=i.get("base") or "",
                                 stats=i.get("stats") or [], why=i.get("why") or "", notes="; ".join(notes))

    for i in late:
        if i["name"] not in pieces:
            pieces[i["name"]] = dict(kind=i.get("kind") or "", setup=short_setup(i.get("setup")),
                                     base=i.get("base") or "", stats=i.get("stats") or [],
                                     why=i.get("why") or "", notes="from the leveling guide; " + (i.get("notes") or ""))
    plan, recipes, tips, stats, switch_note = [], [], [], [], cfg.get("switch_note", "")
    # No attribute plan either (user request): `stats` stays empty.
    if stem == "blood-boil":
        targets = targets_blood_boil(d["targets"])
    else:
        targets = targets_generic(d)
        recipes = [recipe_of(r) for r in d.get("recipes") or []]
        sp = d.get("skill_plan")
        act_tips = list(d.get("act_tips") or [])
        if isinstance(sp, dict) and "entries" in sp:
            # The leveling guide: everything before the respec (the next stage takes over).
            pre = [e for e in sp["entries"] if "respec" not in e["stage"]]
            plan, _ = expand_plan(pre, stem, known)
            if "respec" in d:
                switch_note = cfg.get("respec_note", "")
                if not switch_note:
                    problems.append(f"{stem}: source has a respec but BUILDS has no respec_note")
        elif isinstance(sp, dict):
            if "--leveling" in opts:
                lev = json.load(open(opts["--leveling"], encoding="utf-8"))
                post = [e for e in lev["skill_plan"]["entries"] if "respec" in e["stage"]]
                plan, current = expand_plan(post, stem, known)
                last = max(p[2] for p in plan)
                order = [(s, MAX_POINTS) for s in sp.get("max_order") or []]
                left = (sp.get("leftover_points") or {}).get("skill")
                if left:
                    order.append((left, MAX_POINTS))
                # A second respec once the leveling plan ends (Blizzard: the leveling spec has
                # Meteor, the endgame does not): the planner allocation is due at that level.
                again = sp.get("respec_after_leveling")
                if again:
                    alloc = sp["planner_allocations"][again["allocation"]]
                    for s in alloc:
                        if s not in known:
                            problems.append(f"{stem}: skill {s!r} is not a {cfg['cls']} skill in skills_data.rs")
                    plan += allocation_at(alloc, again["level"], [s for s, _ in order], again["note"])
                    current, last = dict(alloc), again["level"]
                plan += simulate(current, max(last, 75), order, stem, known)
                # The leveling stage's tips from its switch into this stage on.
                lev_stem = os.path.basename(opts["--leveling"]).removesuffix(".json").removesuffix(".source")
                start = BUILDS.get(lev_stem, {}).get("switch_level", 38)
                act_tips = [t for t in lev.get("act_tips") or []
                            if (level_range(tip_range(t["range"]) or "0") or (0,))[0] >= start]
            else:
                alloc = sp["planner_allocations"]["starter_lvl75"]
                order = (sp.get("variants") or {}).get("starter_standard_magic_find", {}).get("max_order") or sp.get("max_order") or []
                for s in alloc:
                    if s not in known:
                        problems.append(f"{stem}: skill {s!r} is not a {cfg['cls']} skill in skills_data.rs")
                plan = allocation_at(alloc, 75, order, "respec: the guide's level-75 Starter allocation")
                plan += simulate(alloc, 75, [(s, MAX_POINTS) for s in order], stem, known)
        for t in act_tips:
            rng = tip_range(t["range"])
            if rng:
                tips += [(rng, x) for x in t.get("tips") or []]
        if "--prerequisites" in opts:
            switch_note = switch_note_prereq(json.load(open(opts["--prerequisites"], encoding="utf-8"))["prerequisites"])

    L = ["# Agent-authored build definition (do not edit while the overlay runs: it is hot-reloaded)."]
    if "header" in cfg:
        L += [h.format(url=d["source_url"], author=d["author"], updated=d["guide_updated"]) for h in cfg["header"]]
    else:
        L += [f"# Source: {d['build']} ({d['source_url']}, {d['author']}, updated {d['guide_updated']}).",
              f"# Generated by research/gen_build.py from {stem}.source.json next to this file; re-run it",
              "# instead of editing by hand. Setups: starter / standard = the guide's variants (or the",
              "# leveling stage, 'lvl a-b'), everything else folds as alternative."]
    L += [f"name = {q(cfg['name'])}", f"damages = [{', '.join(q(x) for x in cfg['damages'])}]",
          "notes = " + q(cfg["notes"]), "targets = [" + ", ".join(q(x) for x in targets if keep_target(x)) + "]"]
    if stem != "blood-boil" or cfg.get("cls"):
        L.append(f"class = {q(cfg['cls'])}")
    if cfg.get("default"):
        L.append("default_for_class = true")
    if cfg.get("next"):
        L += [f"next = {q(cfg['next'])}", f"switch_level = {cfg['switch_level']}",
              f"auto_switch = {'true' if cfg['auto_switch'] else 'false'}"]
        if cfg.get("switch_requires"):
            L.append("switch_requires = [" + ", ".join(req_group(x) for x in cfg["switch_requires"]) + "]")
        if switch_note:
            L.append(f"switch_note = {q(switch_note)}")
    if stats:
        L.append("stats_plan = [" + ", ".join(q(x) for x in stats) + "]")
    if plan:
        L.append("skill_plan = [")
        for skill, points, lvl, note in plan:
            n = f", note = {q(note)}" if note else ""
            L.append(f"  {{ skill = {q(skill)}, points = {points}, by_level = {lvl}{n} }},")
        L.append("]")
    L += ["", "[slots]"]
    for s in ORDER:
        if s in slots:
            L.append(f"{s} = [" + ", ".join(q(x) for x in slots[s]) + "]")
    L += ["", "[merc_slots]"] + [f"{s} = [" + ", ".join(q(x) for x in v) + "]" for s, v in merc.items()]
    L += ["", "[extras]"] + [f"{s} = [" + ", ".join(q(x) for x in v) + "]" for s, v in extras.items()]
    for n, p in pieces.items():
        L += ["", f"[pieces.{q(n)}]"]
        for k in ["kind", "setup", "base", "why", "notes"]:
            if p[k]:
                L.append(f"{k} = {q(p[k])}")
        if p["stats"]:
            L.append("stats = [" + ", ".join(q(x) for x in p["stats"]) + "]")
    for r in recipes:
        L += ["", "[[recipes]]", f"name = {q(r['name'])}", f"kind = {q(r['kind'])}",
              "inputs = [" + ", ".join(q(x) for x in r["inputs"]) + "]"]
        for k in ("result", "req", "notes", "quest"):
            if r[k]:
                L.append(f"{k} = {q(r[k])}")
    for rng, text in tips:
        L += ["", "[[tips]]", f"levels = {q(rng)}", f"text = {q(text)}"]
    with open(out, "w", encoding="utf-8") as f:
        f.write("\n".join(L) + "\n")
    for p in problems:
        print("PROBLEM:", p, file=sys.stderr)
    print(f"{out}: {len(plan)} skill targets, {len(recipes)} recipes, {len(tips)} tips, "
          f"{sum(len(v) for v in slots.values())} slot pieces, {len(problems)} problems", file=sys.stderr)


if __name__ == "__main__":
    main(sys.argv)

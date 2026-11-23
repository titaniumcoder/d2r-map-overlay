"""Turn a researched build guide (JSON, see playstyles/fury-druid/builds/*.source.json)
into an agent-authored build file (builds/<slug>.toml) for the overlay's dashboard.

Usage:
    python3 -I research/gen_build.py SOURCE [OUT.toml] [--leveling LEVELING.json]
                                     [--prerequisites NEXT.json]

SOURCE is a .source.json path or a bare stem (`fury-druid`), which means
playstyles/<playstyle>/builds/<stem>.source.json in the repo, <playstyle> being the stem's
BUILDS `playstyle` (default `fury-druid`). OUT defaults to <stem>.toml next to SOURCE: the
shipped playstyle (a copy in ~/.config/d2r-map-overlay/playstyle/builds/ overrides it per file).

Deterministic: the same inputs give the same file byte for byte, so it can be re-run whenever
the research JSON changes. Per-build settings (display name, class, damages, stage chain)
live in BUILDS below, keyed by the source file's stem (`druid-leveling.source` counts as
`druid-leveling`).

- `--leveling` (Tornado / Lightning Fury endgame): the leveling guide's post-respec plan
  (level 38-75) and its tips for those levels are merged in front of the endgame plan; that
  guide's respec IS the switch into this stage (its `respec_note` in BUILDS is the switch note).
- BUILDS `switch_requires` (optional): item groups that must
  all be met before the switch to `next`; each entry is a plain name (a group of one) or a dict
  {label, any, count?} (met when the copies owned of `any` sum to `count`, default 1), emitted as
  a top-level `switch_requires = [...]` (strings, inline tables) after `auto_switch`, only when set.
- BUILDS `weapon_rule` (Fury Druid: {two_handed, melee}): the player's `weapon` slot takes
  two-handed / non-missile bases only (runeword bases in the stash plan and the next stage
  gear); `swap` is unaffected. Emitted as `weapon_rule = { ... }` after `merc_acts`, only when set.
- BUILDS `order = "rank"` (Fury Druid: leveling and endgame in one build): every slot lists its
  pieces by the source's `rank_in_slot` alone, instead of a leveling build's latest-stage-first.
- BUILDS `stats_plan` (optional): attribute lines, emitted as `stats_plan = [...]`.
- `--prerequisites` (the stage before a manual switch): the next stage's `prerequisites`
  become this stage's `switch_note`.

- The source's `merc.acts` (a list of 1, 2, 3 or 5, primary first; or the single `merc.act`;
  optional) becomes `merc_acts = [N, ...]` after `class`: the acts of the build's mercenaries,
  which decide the weapons its merc slots take (the overlay defaults to Act 2 when it is left
  out, and gives each merc runeword to the first act that can carry it). A merc item with its
  own `merc_act` (an alternative act's piece: Harmony for the Act 1 rogue) sorts after the
  primary act's pieces of its slot; the key itself isn't written out.

- The source's `charms` list (optional) becomes the `[[charms]]` wishlist, in order: unique
  charms `{name, unique: true, stats?, why?}` (the level is the source item's `req_level`) and
  magic ones `{name, size: small|large|grand, count, level?, stats: [{stat, min, tab?, skills?}], why}`
  in the valuables' stat vocabulary. Items of slot `charm` give only their `[pieces]` details
  (no `[extras]` group): the wishlist lists the charms, not the alternatives fold.
- A piece in several slots takes its details from the lowest `rank_in_slot`, a player slot's
  before a merc slot's when both wear it; the merc copy's own leveling window, when it differs,
  becomes `merc_setup`.
- A skill plan target over the level cap (character level - the skill's required level + 1
  hard points) is a problem.

Skill names are checked against crates/core/src/skills_data.rs (class skills); mismatches are reported
on stderr and in the exit summary.
"""

import json
import os
import re
import sys

# Per-build settings by source stem. `next` is the next stage's slug (lowercased name).
BUILDS = {
    "fury-druid": dict(
        name="Fury Druid", cls="druid", damages=["physical"], default=True, order="rank",
        notes="Fury Werewolf from level 1 to 99 in one build: no stage switch, no skill reset. Leveling gear by level (two-handed from level 1, natural-socket bases only, twinked by Vex), then the user's endgame core (Eternity / Breath of the Dying, Fortitude, Jalal's Mane, Laying of Hands, Raven Frost; the merc's 2x Lawbringer, Smoke, Bulwark). Merc: an Act 1 Cold Arrow rogue until Normal Act 5, then the Act 5 Frenzy Barbarian. Durations, breakpoints and skill order computed from the D2R game tables, not from a guide.",
        stats_plan=["Strength only for the gear's requirements: 40 by 6, 70 by 12, 69+ by 24 (Bonesob), 101 by 37 (Hone Sundan), then what the endgame gear needs",
                    "No Dexterity", "Everything else into Vitality"],
        weapon_rule=dict(two_handed=True, melee=True),
    ),
    # The trapsin playstyle (user request, 2026-11-22: "leveling as fire wake (ignore the first 13
    # levels of the build, I do them with tristram runs) and then we just go to trapsin once we
    # hit level 38 or so, nothing else"): Maxroll's Season 16 Assassin Leveling guide from level
    # 13, then its Lightning Sentry endgame guide. No switch_requires: the level alone switches.
    "fire-wake-assassin": dict(
        name="Fire Wake Assassin", cls="assassin", damages=["fire"], default=True,
        playstyle="trapsin", next="trapsin", switch_level=38, auto_switch=True,
        notes="Fire Blast / Wake of Fire leveling (Maxroll Season 16 Assassin Leveling guide) from level 13: levels 1-13 come from Tristram runs, so the skill plan and gear start at 13 with the guide's points up to 13. Stealth, Leaf, then Spirit + Ancients' Pledge and Lore from Nightmare; a Battle Staff of Teleportation and a Wand of Lower Resistance on the swap. Skill reset into the Trapsin (next stage) at 38.",
        respec_note=("at level 38 (the guide: \"The respec assumes that you are Level 38. This is the earliest Level at which you should do it\"; "
                     "its summary says at the start of Act 4 Nightmare, from level 36): Akara's free reset into Lightning Sentry: "
                     "Lightning Sentry 15, Charged Bolt Sentry 10, Death Sentry 9, Burst of Speed 3, Cloak of Shadows 1, Mind Blast 1 "
                     "and the prerequisites Fire Blast, Shock Web, Psychic Hammer, Claw Mastery 1 each = 43 points (37 levels + 6 quest "
                     "points: Normal Den of Evil, Radament, Izual, Nightmare Den of Evil, Radament). Keep Spirit, Ancients' Pledge, Lore, "
                     "Stealth and both swap staves/wands"),
    ),
    "trapsin": dict(
        name="Trapsin", cls="assassin", damages=["lightning", "fire"], playstyle="trapsin",
        notes="Lightning Sentry / Death Sentry Assassin (Maxroll Season 16 endgame guide) from the skill reset at 38; standard = the guide's Standard setup (2x Cunning Greater Talons of Quickness, unsocketed: natural-socket bases only, no socket quest; Griffon's Eye, Enigma, Magefist, Arachnid Mesh, Mara's Kaleidoscope, 2x The Stone of Jordan; the Act 2 Holy Freeze merc with Infinity, Fortitude, Andariel's Visage), starter = the leveling gear she resets with (Spirit, Lore, Stealth, Insight on the merc); every other slot lists the guide's Gear Options as alternatives.",
        # No weapon_rule: the trapsin keeps her off-hand (a second claw or a shield), so the rule
        # that fits would be "one-handed", which WeaponRule can't say; two_handed would be wrong,
        # and melee changes nothing for her weapon-slot runewords (none takes a bow).
    ),
}

HERE = os.path.dirname(os.path.abspath(__file__))
# The playstyle folder of a build without a BUILDS `playstyle` key.
DEFAULT_PLAYSTYLE = "fury-druid"


def builds_dir(stem):
    """The build folder of the shipped playstyle a bare SOURCE stem belongs to."""
    playstyle = BUILDS.get(stem, {}).get("playstyle", DEFAULT_PLAYSTYLE)
    return os.path.normpath(os.path.join(HERE, "..", "playstyles", playstyle, "builds"))


CLASS_IDS = {"amazon": 0, "sorceress": 1, "necromancer": 2, "paladin": 3, "barbarian": 4,
             "druid": 5, "assassin": 6, "warlock": 7}
SLOTMAP = {"ring": "ring1"}
EXTRA = {"jewel": "jewels", "rune": "runes"}
ORDER = ["weapon", "offhand", "swap", "helm", "body", "gloves", "belt", "boots", "amulet", "ring1", "ring2"]
MAX_POINTS = 20


def body_group(slot):
    """The worn slot of a build slot: the weapon and its swap are one hand."""
    return {"swap": "weapon", "ring2": "ring1"}.get(slot, slot)
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


def stat_need(n):
    """One charm `stats` condition as an inline table, keys in the valuables' order."""
    parts = [f"stat = {q(n['stat'])}"]
    if n.get("tab"):
        parts.append(f"tab = {q(n['tab'])}")
    if n.get("skills"):
        parts.append("skills = [" + ", ".join(q(x) for x in n["skills"]) + "]")
    parts.append(f"min = {int(n['min'])}")
    return "{ " + ", ".join(parts) + " }"


def class_skills(cls):
    """Skill name -> required level of class `cls`, from the generated crates/core/src/skills_data.rs."""
    text = open(os.path.join(HERE, "..", "crates", "core", "src", "skills_data.rs"), encoding="utf-8").read()
    skills = {}
    pattern = r'name: "([^"]+)",\s*class: (\d+),\s*page: \d+,\s*row: \d+,\s*col: \d+,\s*req_level: (\d+),'
    for m in re.finditer(pattern, text):
        if int(m.group(2)) == CLASS_IDS[cls]:
            skills[m.group(1)] = int(m.group(3))
    return skills


def check_level_caps(plan, known, build):
    """A skill holds at most character level - its required level + 1 hard points: each target
    over that cap is a problem."""
    for skill, points, level, _ in plan:
        req = known.get(skill)
        if req is not None and points > level - req + 1:
            problems.append(f"{build}: {skill} {points} by level {level} is over the cap "
                            f"{max(0, level - req + 1)} (required level {req})")


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


def targets_generic(d):
    """Short target lines from a build source's `targets` (hardcore-only parts left out)."""
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
    if len(args) not in (1, 2):
        sys.exit(__doc__)
    src = args[0]
    if not src.endswith(".json"):
        src = os.path.join(builds_dir(src), src + ".source.json")
    stem = os.path.basename(src).removesuffix(".json").removesuffix(".source")
    out = args[1] if len(args) == 2 else os.path.join(os.path.dirname(os.path.abspath(src)), stem + ".toml")
    cfg = BUILDS[stem]
    d = json.load(open(src, encoding="utf-8"))
    known = class_skills(cfg["cls"])
    for i in d["items"]:
        i["name"] = i["name"].replace("@", " all res")

    def ref(i):
        return i["name"] + (f"@{i['req_level']}" if i.get("req_level") else "")

    leveling_items = any(str(i.get("setup") or "").startswith("lvl") for i in d["items"])
    merc_src = d.get("merc") or {}
    merc_acts = merc_src.get("acts") or ([merc_src["act"]] if merc_src.get("act") is not None else [])
    # An alternative merc act's piece (its own `merc_act`, not the primary) after the others.
    alt = lambda i: i.get("merc_act") is not None and i["merc_act"] != (merc_acts or [2])[0]
    if cfg.get("order") == "rank":
        key = lambda i: (i["slot"], i.get("rank_in_slot") if i.get("rank_in_slot") is not None else 999)
    elif leveling_items:
        # Stage pieces by required level (the latest stage first); a guide's "alternative"
        # pieces after them in their own rank order, so the stage gear stays first.
        guide = lambda i: i.get("setup") == "alternative"
        key = lambda i: (i["slot"], alt(i), guide(i), 0 if guide(i) else -(i.get("req_level") or 0),
                         i.get("rank_in_slot") or 99)
    else:
        key = lambda i: (i["slot"], alt(i), i.get("rank_in_slot") if i.get("rank_in_slot") is not None else 999)
    slots, merc, extras = {}, {}, {}
    for i in sorted(d["items"], key=key):
        s = i["slot"]
        if s.startswith("merc_"):
            lst = merc.setdefault(s[5:], [])
        elif s == "charm":
            continue
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
        for lst in list(slots.values()) + list(merc.values()):
            lst.sort(key=lambda r: r.split("@")[0] in names)
        for i in late:
            s_ = i["slot"]
            lst = merc.setdefault(s_[5:], []) if s_.startswith("merc_") else slots.setdefault(SLOTMAP.get(s_, s_), [])
            if not any(r.split("@")[0] == i["name"] for r in lst):
                lst.append(ref(i))
    pieces = {}
    piece_slot = {}
    worn_by_both = ({i["name"] for i in d["items"] if not i["slot"].startswith("merc_")}
                    & {i["name"] for i in d["items"] if i["slot"].startswith("merc_")})
    piece_order = lambda i: (i["slot"].startswith("merc_") and i["name"] in worn_by_both,
                             i.get("rank_in_slot") if i.get("rank_in_slot") is not None else 999)
    for i in sorted(d["items"], key=piece_order):
        notes = [n for n in [i.get("notes") or ""] if n]
        setup = short_setup(i.get("setup"))
        if setup == "alternative" and i.get("setup") not in (None, "alternative"):
            notes.insert(0, f"{i['setup'].replace('_', ' ')} setup")
        if i.get("merc_tier"):
            notes.insert(0, f"merc gear: {i['merc_tier']} game")
        details = dict(base=i.get("base") or "", why=i.get("why") or "", notes="; ".join(notes))
        slot = SLOTMAP.get(i.get("slot") or "", i.get("slot") or "")
        if i["name"] in pieces:
            # The same piece in another hand or body slot with its own base (Spirit Monarch
            # beside the Spirit sword): its details for that slot.
            first = pieces[i["name"]]
            if (slot in ORDER and body_group(slot) != body_group(piece_slot[i["name"]])
                    and details["base"] != first["base"]):
                first["slot"].setdefault(slot, details)
            continue
        piece_slot[i["name"]] = slot
        pieces[i["name"]] = dict(kind=i.get("kind") or "", setup=setup, stats=i.get("stats") or [],
                                 slot={}, **details)

    for i in d["items"]:
        piece = pieces.get(i["name"])
        if (piece is not None and i["slot"].startswith("merc_") and i["name"] in worn_by_both
                and short_setup(i.get("setup")) != piece["setup"]):
            piece["merc_setup"] = short_setup(i.get("setup"))
    for i in late:
        if i["name"] not in pieces:
            pieces[i["name"]] = dict(kind=i.get("kind") or "", setup=short_setup(i.get("setup")),
                                     base=i.get("base") or "", stats=i.get("stats") or [], slot={},
                                     why=i.get("why") or "", notes="from the leveling guide; " + (i.get("notes") or ""))
    plan, recipes, tips, switch_note = [], [], [], cfg.get("switch_note", "")
    stats = cfg.get("stats_plan", [])
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
    check_level_caps(plan, known, stem)
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
    L.append(f"class = {q(cfg['cls'])}")
    if cfg.get("default"):
        L.append("default_for_class = true")
    for act in merc_acts:
        if act not in (1, 2, 3, 5):
            problems.append(f"{stem}: merc act {act!r} is not 1, 2, 3 or 5")
    for i in d["items"]:
        if i.get("merc_act") is not None and i["merc_act"] not in (merc_acts or [2]):
            problems.append(f"{stem}: {i['name']}: merc_act {i['merc_act']!r} is not in merc.acts")
    if merc_acts:
        L.append("merc_acts = [" + ", ".join(str(int(a)) for a in merc_acts) + "]")
    if cfg.get("weapon_rule"):
        L.append("weapon_rule = { " + ", ".join(f"{k} = true" for k, on in cfg["weapon_rule"].items() if on) + " }")
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
    if extras:
        L += ["", "[extras]"] + [f"{s} = [" + ", ".join(q(x) for x in v) + "]" for s, v in extras.items()]
    levels = {i["name"]: i.get("req_level") for i in d["items"]}
    for c in d.get("charms") or []:
        L += ["", "[[charms]]", f"name = {q(c['name'])}"]
        if c.get("unique"):
            L.append("unique = true")
            if levels.get(c["name"]):
                L.append(f"level = {int(levels[c['name']])}")
            else:
                problems.append(f"{stem}: unique charm {c['name']!r} has no source item with req_level")
        else:
            L += [f"size = {q(c['size'])}", f"count = {int(c.get('count', 1))}"]
            if c.get("level"):
                L.append(f"level = {int(c['level'])}")
        if c.get("stats"):
            L.append("stats = [" + ", ".join(stat_need(x) for x in c["stats"]) + "]")
        if c.get("why"):
            L.append(f"why = {q(c['why'])}")
    for n, p in pieces.items():
        L += ["", f"[pieces.{q(n)}]"]
        for k in ["kind", "setup", "merc_setup", "base", "why", "notes"]:
            if p.get(k):
                L.append(f"{k} = {q(p[k])}")
        if p["stats"]:
            L.append("stats = [" + ", ".join(q(x) for x in p["stats"]) + "]")
        for slot, sd in p["slot"].items():
            L += ["", f"[pieces.{q(n)}.slot.{slot}]"]
            for k in ["base", "why", "notes"]:
                if sd[k]:
                    L.append(f"{k} = {q(sd[k])}")
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

"""Generate src/runewords_data.rs from blizzhackers/d2data runes.json + itemtypes.json (D2R).

Usage: python3 -I research/gen_runewords.py > src/runewords_data.rs && cargo fmt
The script fetches runes.json itself via `gh api` (read-only; nothing else is written):
  runes.json     — runewords.txt rows: each entry's `*Rune Name` is the display name,
                   `*RunesUsed` the rune names concatenated in socket order (e.g.
                   "RalOrtTal" for Ancient's Pledge), `itype1..itypeN` the item-type codes
                   that can hold it, and `Rune1..N` the rune codes (r20 = Lem) whose
                   fixed runes.txt order is authoritative when `*RunesUsed` has a typo.
                   Rows without `*RunesUsed`/`itype1` are incomplete
                   (unreleased) runewords and are skipped.
The itype codes were verified once against d2data's itemtypes.json and are hardcoded
in BASE_NAMES below (the itemtypes table itself is not fetched at generation time).
"""
import base64
import json
import re
import subprocess
import sys

# The 33 D2 rune names, longest first for the greedy split below; `El` is the only prefix
# of another name (`Eld`), so longest-match splits "*RunesUsed" unambiguously.
RUNE_NAMES = [
    "Shael", "Cham", "Hel", "Thul", "Tir", "Nef", "Eth", "Ith", "Tal", "Ral", "Ort",
    "Amn", "Sol", "Dol", "Io", "Lum", "Ko", "Fal", "Lem", "Pul", "Um", "Mal", "Ist",
    "Gul", "Vex", "Ohm", "Lo", "Sur", "Ber", "Jah", "Zod", "Eld", "El",
]
# Readable plural base names for the item-type codes that appear in runes.json's
# itype1..N columns, taken from itemtypes.json's "ItemType" column (verified, lowercased):
# ashd = "Auric Shields", axe = "Axe", club = "Club", grim = "Grimoire",
# h2h = "Hand to Hand", hamm = "Hammer", head = "Voodoo Heads", helm = "Helm",  (h2h shown as "claws")
# knif = "Knife", mace = "Mace", mele = "Melee Weapon", miss = "Missile Weapon",
# pala = "Paladin Item", pole = "Polearm", scep = "Scepter", shld = "Any Shield",  (pala shown as "paladin shields")
# spea = "Spear", staf = "Staff", swor = "Sword", tors = "Armor", wand = "Wand",
# weap = "Weapon". A few are lightly re-worded for the hint text (noted inline).
BASE_NAMES = {
    "ashd": "auric shields",
    "axe": "axes",
    "club": "clubs",
    "grim": "grimoires",
    "h2h": "claws",  # itemtypes.json: "Hand to Hand" (claw-class melee)
    "hamm": "hammers",
    "head": "voodoo heads",
    "helm": "helms",
    "knif": "knives",
    "mace": "maces",
    "mele": "melee weapons",
    "miss": "missile weapons",
    "pala": "paladin shields",  # itemtypes.json: "Paladin Item"; in-game they are shields
    "pole": "polearms",
    "scep": "scepters",
    "shld": "shields",  # itemtypes.json calls it "Any Shield"
    "spea": "spears",
    "staf": "staves",
    "swor": "swords",
    "tors": "armor",  # mass noun, not "armors"
    "wand": "wands",
    "weap": "weapons",
}


# Upstream data-file names that differ from the in-game display name.
NAME_FIXES = {"Ancients' Pledge": "Ancient's Pledge"}


def fetch(path):
    """Fetches a JSON table from blizzhackers/d2data through `gh api` (read-only)."""
    out = subprocess.run(
        ["gh", "api", f"repos/blizzhackers/d2data/contents/json/{path}", "--jq", ".content"],
        check=True, capture_output=True, text=True,
    ).stdout
    return json.loads(base64.b64decode(out))


# Runes.txt is fixed at 33 runes in display order; a runeword row's `Rune1..N` columns
# reference them by code ("r20" = Lem), which is authoritative for socket order.
RUNE_CODES = {
    f"r{i:02d}": name
    for i, name in enumerate(
        ["El", "Eld", "Tir", "Nef", "Eth", "Ith", "Tal", "Ral", "Ort", "Thul",
         "Amn", "Sol", "Shael", "Dol", "Hel", "Io", "Lum", "Ko", "Fal", "Lem",
         "Pul", "Um", "Mal", "Ist", "Gul", "Vex", "Ohm", "Lo", "Sur", "Ber",
         "Jah", "Cham", "Zod"],
        1,
    )
}


def split_runes(concatenated):
    """Splits a "*RunesUsed" string ("RalOrtTal") into rune names in order."""
    names, rest = [], concatenated
    while rest:
        for rune in sorted(RUNE_NAMES, key=len, reverse=True):
            if rest.startswith(rune):
                names.append(rune)
                rest = rest[len(rune):]
                break
        else:
            return None
    return names


def runes_of(row):
    """A runeword's runes in socket order, from its Rune1..N codes (authoritative).

    The `*RunesUsed` column has typos upstream (Wealth: "LmKoTir" for Lem+Ko+Tir,
    Enlightenment: "PulRalSur" for Pul+Ral+Sol), so it is only the fallback and the
    cross-check.
    """
    codes = sorted(
        (k for k in row if re.fullmatch(r"Rune\d+", k)),
        key=lambda k: int(k.removeprefix("Rune")),
    )
    from_codes = [RUNE_CODES[code] for k in codes if (code := row[k])]
    if not from_codes:
        return split_runes(row["*RunesUsed"]) or []
    if len(from_codes) != len(codes):
        raise SystemExit(f"unknown rune codes in {row.get('*Rune Name')!r}")
    if split_runes(row["*RunesUsed"]) != from_codes:
        print(
            f"note: {row['*Rune Name']}: *RunesUsed {row['*RunesUsed']!r} disagrees with the"
            f" rune codes {''.join(from_codes)!r}; using the rune codes",
            file=sys.stderr,
        )
    return from_codes


def bases_of(row):
    """The readable base names of a runeword row, in its itype1..N column order."""
    codes = sorted(
        (k for k in row if re.fullmatch(r"itype\d+", k)),
        key=lambda k: int(k.removeprefix("itype")),
    )
    unknown = [c for k in codes if (c := row[k]) not in BASE_NAMES]
    if unknown:
        raise SystemExit(f"unknown itype codes {unknown} in {row.get('*Rune Name')!r}")
    bases = []
    for k in codes:
        base = BASE_NAMES[row[k]]
        if base not in bases:
            bases.append(base)
    return bases


def rust_string(text):
    """Rust string literal for `text` (the data never contains quotes or backslashes)."""
    assert '"' not in text and "\\" not in text, text
    return f'"{text}"'


runes = fetch("runes.json")
runes = runes if isinstance(runes, list) else list(runes.values())
rows = sorted(
    (
        (NAME_FIXES.get(row["*Rune Name"], row["*Rune Name"]), runes_of(row), bases_of(row))
        for row in runes
        if row.get("*RunesUsed") and any(k.startswith("itype") for k in row)
    ),
    key=lambda t: t[0].lower(),
)
for name, used, bases in rows:
    assert used, name
    assert bases, name
    assert all(r in RUNE_NAMES for r in used), (name, used)

print("//! Generated by `research/gen_runewords.py` from blizzhackers/d2data runes.json +")
print("//! itemtypes.json (D2R). Do not edit by hand.")
print()
print("/// A runeword: its in-game name, its runes in socket order, and the readable item")
print("/// types (from itemtypes.json) whose base items can hold it.")
print("#[derive(Debug)]")
print("pub struct Runeword {")
print("    pub name: &'static str,")
print("    pub runes: &'static [&'static str],")
print("    pub bases: &'static [&'static str],")
print("}")
print()
print("/// The 33 rune names a `*RunesUsed` column can contain; only the generated tests use")
print("/// them (the runeword rows carry their runes already).")
print("#[cfg(test)]")
print(f"pub const RUNE_NAMES: [&str; {len(RUNE_NAMES)}] = [")
for rune in RUNE_NAMES:
    print(f"    {rust_string(rune)},")
print("];")
print()
print(f"/// Every complete runeword in runewords.txt ({len(rows)} entries), sorted by name.")
print(f"pub static RUNEWORDS: [Runeword; {len(rows)}] = [")
for name, used, bases in rows:
    runes_str = ", ".join(rust_string(r) for r in used)
    bases_str = ", ".join(rust_string(b) for b in bases)
    print(f"    Runeword {{ name: {rust_string(name)}, runes: &[{runes_str}], bases: &[{bases_str}] }},")
print("];")
print()
print("/// Looks a runeword up by name, ignoring ASCII case and apostrophes ([`crate::info::same_name`]:")
print("/// `\"steel\"`, `\"Ancients' Pledge\"` match): the `BiS` piece strings in the config are user-typed. Disambiguated")
print("/// upstream names (`\"Hustle (armor)\"`, `\"Hustle (weapon)\"`) also match their bare")
print("/// form, since that is what the game shows.")
print("#[must_use]")
print("pub fn lookup(name: &str) -> Option<&'static Runeword> {")
print("    RUNEWORDS.iter().find(|w| {")
print("        crate::info::same_name(w.name, name)")
print("            || w.name")
print("                .split_once(\" (\")")
print("                .is_some_and(|(bare, _)| crate::info::same_name(bare, name))")
print("    })")
print("}")
print()
print("#[cfg(test)]")
print("mod tests {")
print("    use super::*;")
print()
print("    #[test]")
print("    fn lookup_matches_any_case() {")
print('        assert!(lookup("steel").is_some(), "lowercase must match");')
print('        assert!(lookup("STEEL").is_some(), "uppercase must match");')
print('        assert!(lookup("Steel").is_some(), "the display name must match");')
print('        assert!(lookup("Crafted Rare").is_none(), "non-runewords must not match");')
print('        assert!(lookup("Ancients\' Pledge").is_some(), "the builds\' apostrophe must match");')
print("    }")
print()
print("    #[test]")
print("    fn ancients_pledge_runes_are_in_socket_order() {")
print('        let rw = lookup("Ancient\'s Pledge");')
print('        assert!(rw.is_some(), "Ancient\'s Pledge must be in the table");')
print("        if let Some(rw) = rw {")
print('            assert_eq!(rw.runes, ["Ral", "Ort", "Tal"].as_slice(), "socket order");')
print('            assert!(rw.bases.contains(&"shields"), "bases must contain shields");')
print("        }")
print("    }")
print()
print("    #[test]")
print("    fn every_runeword_has_at_least_one_rune_and_base() {")
print('        assert!(RUNEWORDS.iter().all(|w| !w.runes.is_empty() && !w.bases.is_empty()));')
print("    }")
print()
print("    #[test]")
print("    fn disambiguated_names_match_their_bare_form() {")
print('        assert!(lookup("Hustle").is_some(), "the game shows Hustle without a suffix");')
print("    }")
print()
print("    #[test]")
print("    fn every_rune_is_a_d2_rune_name() {")
print("        assert!(RUNEWORDS.iter().all(|w| w.runes.iter().all(|r| RUNE_NAMES.contains(r))));")
print("    }")
print("}")

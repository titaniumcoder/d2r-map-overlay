//! Concrete white bases to farm for a runeword: ones that can drop with exactly the runeword's
//! socket count (user request, 2026-11-20: no socket quest on this character, so the dashboard
//! names real natural-socket bases instead of "missile weapons").
//!
//! A base drops with at most `min(gemsockets, type cap)` sockets, where the type cap is the
//! item type's itemtypes.txt `MaxSockets1/2/3` for item level ≤ 25 / ≤ 40 / above
//! ([`crate::bases_data::ITEM_TYPES`]). It drops at all only from item level ≥ its quality
//! level.

use crate::bases_data::{self, ITEM_TYPES, SOCKETABLE};
use crate::runewords_data::Runeword;

/// The item-type code behind each readable runeword base category
/// ([`Runeword::bases`]); mirrors `BASE_NAMES` in `research/gen_runewords.py`.
const CATEGORY_TYPES: [(&str, &str); 22] = [
    ("auric shields", "ashd"),
    ("axes", "axe"),
    ("clubs", "club"),
    ("grimoires", "grim"),
    ("claws", "h2h"),
    ("hammers", "hamm"),
    ("voodoo heads", "head"),
    ("helms", "helm"),
    ("knives", "knif"),
    ("maces", "mace"),
    ("melee weapons", "mele"),
    ("missile weapons", "miss"),
    ("paladin shields", "pala"),
    ("polearms", "pole"),
    ("scepters", "scep"),
    ("shields", "shld"),
    ("spears", "spea"),
    ("staves", "staf"),
    ("swords", "swor"),
    ("armor", "tors"),
    ("wands", "wand"),
    ("weapons", "weap"),
];

/// The act a mercenary was hired in: it decides the weapons the mercenary wields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MercAct {
    /// Rogue Scout: bows (not crossbows).
    Act1,
    /// Desert Mercenary: polearms and spears (not javelins). The default: the live snapshot
    /// doesn't read the hireling's act, and a build file without `merc_act` means this one.
    #[default]
    Act2,
    /// Iron Wolf: a one-handed sword and a shield.
    Act3,
    /// Barbarian: swords, one-handed. The user says the Act 5 mercenary dual-wields two
    /// one-handed swords (the Frenzy merc type, D2R patch 2.4), so a two-handed
    /// sword is never planned for it, and its off-hand slot takes a second sword.
    Act5,
}

impl MercAct {
    /// The act of number `act` (1, 2, 3 or 5; Act 4 has no mercenary).
    #[must_use]
    pub const fn from_number(act: u8) -> Option<Self> {
        match act {
            1 => Some(Self::Act1),
            2 => Some(Self::Act2),
            3 => Some(Self::Act3),
            5 => Some(Self::Act5),
            _ => None,
        }
    }

    /// The act's number.
    #[must_use]
    pub const fn number(self) -> u8 {
        match self {
            Self::Act1 => 1,
            Self::Act2 => 2,
            Self::Act3 => 3,
            Self::Act5 => 5,
        }
    }

    /// The item types it wields (class-only variants, such as amazon bows and spears, drop
    /// out through the class check).
    const fn weapons(self) -> &'static [&'static str] {
        match self {
            Self::Act1 => &["bow"],
            Self::Act2 => &["pole", "spea"],
            Self::Act3 | Self::Act5 => &["swor"],
        }
    }

    /// Whether its weapon must be one-handed: the Act 3 mercenary holds a shield, the Act 5
    /// one a second sword (per the user, see [`MercAct::Act5`]).
    const fn one_handed(self) -> bool {
        matches!(self, Self::Act3 | Self::Act5)
    }

    /// Whether it dual-wields: its off-hand slot takes a weapon (Act 5, per the user).
    const fn dual_wields(self) -> bool {
        matches!(self, Self::Act5)
    }
}

/// Who will wear the base.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wearer {
    /// The player character of this class (player txt row, 0 amazon .. 7 warlock): bases
    /// restricted to another class are skipped.
    Player(u32),
    /// The player character of this class with an off-hand piece (a shield) beside the weapon:
    /// as [`Wearer::Player`], but only one-handed weapons ([`two_handed_for`]).
    PlayerWithOffhand(u32),
    /// The mercenary of this act. It can't wear class-only bases (pelts, barbarian helms, ...;
    /// circlets have no class and stay in), and its weapons are its act's
    /// ([`MercAct::weapons`]); an Act 3 one also takes shields.
    Merc(MercAct),
}

impl Wearer {
    /// Whether its weapon must be one-handed.
    const fn one_hand_only(self) -> bool {
        match self {
            Self::Player(_) => false,
            Self::PlayerWithOffhand(_) => true,
            Self::Merc(act) => act.one_handed(),
        }
    }
}

/// Whether `wearer` can use a base of raw item type `ty`, body slot `slot` and class
/// restriction `class` (its hands aside: [`in_hand`]).
fn wearable(wearer: Wearer, ty: &str, slot: u8, class: Option<u8>) -> bool {
    match wearer {
        Wearer::Player(c) | Wearer::PlayerWithOffhand(c) => class.is_none_or(|only| u32::from(only) == c),
        // Javelins are spears in itemtypes.txt (Equiv2), but no merc weapon.
        Wearer::Merc(act) => {
            class.is_none()
                && match slot {
                    4 => act.weapons().iter().any(|w| is_a(ty, w, 0)) && !is_a(ty, "jave", 0),
                    5 => act == MercAct::Act3,
                    _ => true,
                }
        }
    }
}

/// Whether weapon `code` takes both hands for `wearer`: weapons.txt `2handed`, except that a
/// barbarian player wields a `1or2handed` sword in one hand. The Act 5 mercenary is a barbarian
/// too, but the user plans its swords one-handed ([`MercAct::Act5`]). `false` for a non-weapon.
#[must_use]
pub fn two_handed_for(code: &str, wearer: Wearer) -> bool {
    let barbarian = matches!(wearer, Wearer::Player(4) | Wearer::PlayerWithOffhand(4));
    crate::weapons_data::two_handed(code) == Some(true)
        && !(barbarian && crate::weapons_data::one_or_two_handed(code))
}

/// Whether a base worn in body slot `base_slot` goes in the wanted `slot` (`None`: any): the
/// same slot, or a weapon in the off-hand (5) of a mercenary that dual-wields
/// ([`MercAct::dual_wields`]).
fn in_slot(wearer: Wearer, slot: Option<u8>, base_slot: u8) -> bool {
    let dual = matches!(wearer, Wearer::Merc(act) if act.dual_wields());
    slot.is_none_or(|s| s == base_slot || (dual && s == 5 && base_slot == 4))
}

/// Whether `wearer`'s hands allow base `code`: a wearer limited to one hand
/// ([`Wearer::PlayerWithOffhand`], an Act 3 or Act 5 mercenary) takes no two-handed weapon.
fn in_hand(code: &str, wearer: Wearer) -> bool {
    !wearer.one_hand_only() || !two_handed_for(code, wearer)
}

/// Whether the mercenary of `act` can wear the base with item code `code`: a helm or body
/// armor of no class, a weapon of its act (one-handed for Act 3 and 5), and for Act 3 a shield.
#[must_use]
pub fn merc_wears(code: &str, act: MercAct) -> bool {
    let wearer = Wearer::Merc(act);
    bases_data::base(code).is_some_and(|b| {
        matches!(b.slot, 1 | 3 | 4 | 5) && wearable(wearer, b.kind, b.slot, b.class) && in_hand(code, wearer)
    })
}

/// Whether the hint ranks by defense: the mercenary's helm or body armor (user request,
/// 2026-11-20: the merc wants the strongest base, its strength is no concern of the player's).
const fn by_defense(wearer: Wearer, slot: Option<u8>) -> bool {
    matches!(wearer, Wearer::Merc(_)) && matches!(slot, Some(1 | 3))
}

/// One base that can drop with the wanted socket count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Pick {
    pub name: &'static str,
    pub str: u32,
    pub dex: u32,
    /// Maximum base defense (armor.txt `maxac`; 0 for weapons).
    pub def: u16,
    /// Index into the runeword's `bases` of the category it matched first.
    pub category: usize,
}

/// The itemtypes.txt row of type `code`.
fn item_type(code: &str) -> Option<&'static bases_data::TypeRow> {
    let i = ITEM_TYPES.binary_search_by(|row| row.0.cmp(code)).ok()?;
    ITEM_TYPES.get(i)
}

/// Whether item type `ty` is `want` or reaches it through its Equiv1/Equiv2 parents. The
/// chains are short and acyclic in the data; `depth` guards against a bad table anyway.
fn is_a(ty: &str, want: &str, depth: u8) -> bool {
    if ty == want {
        return true;
    }
    let Some(&(_, e1, e2, _)) = item_type(ty) else {
        return false;
    };
    depth < 8 && [e1, e2].iter().any(|e| !e.is_empty() && is_a(e, want, depth + 1))
}

/// The most sockets a base of type `ty` can drop with at item level `ilvl` (the type cap
/// only; the base's own `gemsockets` limits it further).
fn type_cap(ty: &str, ilvl: u32) -> u8 {
    item_type(ty).map_or(0, |&(_, _, _, [low, mid, high])| match ilvl {
        0..=25 => low,
        26..=40 => mid,
        _ => high,
    })
}

/// Every base fitting one of `categories` (readable runeword base names) that can drop with
/// exactly `sockets` sockets at item level `max_ilvl` or below, whose quality level is ≤
/// `max_ilvl`, that `wearer` can use ([`Wearer`], its hands included) and, with `slot` set,
/// that is worn in that body slot.
/// Sorted by strength + dexterity requirement, then quality level, then name; for the
/// mercenary's helm or body armor by maximum defense, highest first, then the same keys.
///
/// The socket check uses `max_ilvl` alone: the caps only grow with item level, so the
/// highest reachable level is where a base can roll the most sockets.
#[must_use]
pub fn natural_bases(
    categories: &[&str],
    sockets: usize,
    max_ilvl: u32,
    wearer: Wearer,
    slot: Option<u8>,
) -> Vec<Pick> {
    let wanted: Vec<Option<&str>> = categories
        .iter()
        .map(|c| {
            CATEGORY_TYPES
                .iter()
                .find(|(name, _)| name == c)
                .map(|&(_, ty)| ty)
        })
        .collect();
    let mut picks: Vec<(u32, &str, Pick)> = SOCKETABLE
        .iter()
        .filter_map(|&(code, name, ty, max, qlvl, def)| {
            let base = bases_data::base(code)?;
            let category = wanted.iter().position(|w| w.is_some_and(|w| is_a(ty, w, 0)))?;
            let max = usize::from(max.min(type_cap(ty, max_ilvl)));
            (max >= sockets
                && u32::from(qlvl) <= max_ilvl
                && wearable(wearer, ty, base.slot, base.class)
                && in_hand(code, wearer)
                && in_slot(wearer, slot, base.slot))
            .then_some((
                u32::from(qlvl),
                name,
                Pick {
                    name,
                    str: base.str,
                    dex: base.dex,
                    def,
                    category,
                },
            ))
        })
        .collect();
    let defense = by_defense(wearer, slot);
    picks.sort_by_key(|&(qlvl, name, p)| {
        // `Reverse` puts the highest defense first; 0 for everything outside the merc's armor.
        let def = std::cmp::Reverse(if defense { p.def } else { 0 });
        (def, p.str.saturating_add(p.dex), qlvl, name)
    });
    picks.into_iter().map(|(_, _, p)| p).collect()
}

/// The [`SOCKETABLE`] row of item code `code` (the table is sorted by code).
fn socketable_row(code: &str) -> Option<&'static bases_data::SocketRow> {
    let i = SOCKETABLE.binary_search_by(|row| row.0.cmp(code)).ok()?;
    SOCKETABLE.get(i)
}

/// Whether item code `code` is a weapon or armor base that can have sockets ([`SOCKETABLE`]):
/// a runeword base, as opposed to a potion, tome, gem or other plain white item.
#[must_use]
pub fn socketable(code: &str) -> bool {
    socketable_row(code).is_some()
}

/// The maximum base defense of socketable armor `code` (armor.txt `maxac`); `None` for a
/// weapon or a code that isn't a socketable base.
#[must_use]
pub fn max_defense(code: &str) -> Option<u16> {
    socketable_row(code)
        .map(|&(_, _, _, _, _, def)| def)
        .filter(|&def| def > 0)
}

/// Whether the base with item code `code` can carry runeword `word` for `wearer` (the same
/// category, class, mercenary and hand checks as [`natural_bases`]) and, with `slot` set, is
/// worn in that body slot. The socket count is the caller's check: this is about the base only.
#[must_use]
pub fn fits(word: &Runeword, code: &str, wearer: Wearer, slot: Option<u8>) -> bool {
    fits_type(word, code, wearer, slot) && in_hand(code, wearer)
}

/// Whether base `code` would [`fit`](fits) `word` for `wearer` but takes both hands where the
/// wearer has only one free (a druid's Zweihander beside a shield).
#[must_use]
pub fn two_handed_misfit(word: &Runeword, code: &str, wearer: Wearer, slot: Option<u8>) -> bool {
    fits_type(word, code, wearer, slot) && !in_hand(code, wearer)
}

/// [`fits`] without the hand check.
fn fits_type(word: &Runeword, code: &str, wearer: Wearer, slot: Option<u8>) -> bool {
    let (Some(&(_, _, ty, _, _, _)), Some(base)) = (socketable_row(code), bases_data::base(code)) else {
        return false;
    };
    let category = word.bases.iter().any(|c| {
        CATEGORY_TYPES
            .iter()
            .find(|(name, _)| name == c)
            .is_some_and(|&(_, want)| is_a(ty, want, 0))
    });
    category && wearable(wearer, ty, base.slot, base.class) && in_slot(wearer, slot, base.slot)
}

/// Whether base `code` can carry runeword `word` at all: it [`fits`] and can have the
/// runeword's socket count (its `gemsockets` within the type cap at the highest item level).
#[must_use]
pub fn carries(word: &Runeword, code: &str, wearer: Wearer, slot: Option<u8>) -> bool {
    socketable_row(code).is_some_and(|&(_, _, ty, max, _, _)| {
        usize::from(max.min(type_cap(ty, u32::MAX))) >= word.runes.len()
    }) && fits(word, code, wearer, slot)
}

/// A base's requirement text: `"25/35"` (str/dex), `"43 Str"`, `"28 Dex"` or `"no req"`.
fn requirement(p: &Pick) -> String {
    match (p.str, p.dex) {
        (0, 0) => "no req".to_owned(),
        (s, 0) => format!("{s} Str"),
        (0, d) => format!("{d} Dex"),
        (s, d) => format!("{s}/{d}"),
    }
}

/// The dashboard's base hint for runeword `word`: `"4 sockets: Hunter's Bow (28 Dex),
/// Long Bow (22/19), …"` — the cheapest four [`natural_bases`] by requirement, or, when the
/// runeword allows several categories that each have bases, the cheapest three per category
/// (`"4 sockets: swords: … · shields: …"`). For the mercenary's helm or body armor the four
/// strongest, with their defense: `"3 sockets: Corona (def 165, 174 Str), …"`. `None`
/// when nothing qualifies (the caller falls back to the category names).
#[must_use]
pub fn hint(word: &Runeword, max_ilvl: u32, wearer: Wearer, slot: Option<u8>) -> Option<String> {
    let sockets = word.runes.len();
    let picks = natural_bases(word.bases, sockets, max_ilvl, wearer, slot);
    let defense = by_defense(wearer, slot);
    let label = |p: &Pick| {
        if defense {
            format!("{} (def {}, {})", p.name, p.def, requirement(p))
        } else {
            format!("{} ({})", p.name, requirement(p))
        }
    };
    let list =
        |ps: &mut dyn Iterator<Item = &Pick>, n: usize| ps.take(n).map(label).collect::<Vec<_>>().join(", ");
    let groups: Vec<(usize, &&str)> = word
        .bases
        .iter()
        .enumerate()
        .filter(|&(i, _)| picks.iter().any(|p| p.category == i))
        .collect();
    let body = match groups.as_slice() {
        [] => return None,
        [_] => list(&mut picks.iter(), 4),
        _ => groups
            .iter()
            .map(|&(i, name)| {
                format!(
                    "{name}: {}",
                    list(&mut picks.iter().filter(|p| p.category == i), 3)
                )
            })
            .collect::<Vec<_>>()
            .join(" · "),
    };
    Some(format!("{sockets} sockets: {body}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const DRUID: Wearer = Wearer::Player(5);

    fn names(picks: &[Pick]) -> Vec<&'static str> {
        picks.iter().map(|p| p.name).collect()
    }

    #[test]
    fn every_category_maps_to_an_item_type() {
        for (_, ty) in CATEGORY_TYPES {
            assert!(item_type(ty).is_some(), "{ty}");
        }
        for word in &crate::runewords_data::RUNEWORDS {
            for base in word.bases {
                assert!(CATEGORY_TYPES.iter().any(|(c, _)| c == base), "{base}");
            }
        }
    }

    #[test]
    fn harmony_at_a_high_level_lists_hunters_and_composite_bows() {
        let picks = natural_bases(&["missile weapons"], 4, 60, DRUID, None);
        let names = names(&picks);
        assert!(names.contains(&"Hunter's Bow"), "{names:?}");
        assert!(names.contains(&"Composite Bow"), "{names:?}");
        // Cheapest first: Hunter's Bow needs only 28 Dex.
        assert_eq!(names.first(), Some(&"Hunter's Bow"));
    }

    #[test]
    fn a_base_with_too_few_max_sockets_is_left_out() {
        // Short Bow: gemsockets 3, so never 4 sockets however high the item level.
        let picks = natural_bases(&["missile weapons"], 4, 99, DRUID, None);
        assert!(!names(&picks).contains(&"Short Bow"));
        assert!(names(&natural_bases(&["missile weapons"], 3, 99, DRUID, None)).contains(&"Short Bow"));
    }

    #[test]
    fn the_item_level_cap_limits_sockets() {
        // Bows cap at 3 sockets up to item level 25, 4 up to 40.
        assert!(!names(&natural_bases(&["missile weapons"], 4, 25, DRUID, None)).contains(&"Hunter's Bow"));
        assert!(names(&natural_bases(&["missile weapons"], 4, 26, DRUID, None)).contains(&"Hunter's Bow"));
    }

    #[test]
    fn the_quality_level_must_be_reachable() {
        // Razor Bow: quality level 33, four sockets.
        assert!(!names(&natural_bases(&["missile weapons"], 4, 32, DRUID, None)).contains(&"Razor Bow"));
        assert!(names(&natural_bases(&["missile weapons"], 4, 33, DRUID, None)).contains(&"Razor Bow"));
    }

    #[test]
    fn other_classes_restricted_bases_are_left_out() {
        // Amazon bows (Stag Bow, quality 18, 4 sockets after level 25) only for the amazon.
        let amazon = natural_bases(&["missile weapons"], 4, 60, Wearer::Player(0), None);
        assert!(names(&amazon).contains(&"Stag Bow"), "{:?}", names(&amazon));
        assert!(!names(&natural_bases(&["missile weapons"], 4, 60, DRUID, None)).contains(&"Stag Bow"));
        // A druid pelt still counts as a helm for the druid.
        let pelts = natural_bases(&["helms"], 3, 60, DRUID, None);
        assert!(names(&pelts).contains(&"Wolf Head"), "{:?}", names(&pelts));
        assert!(!names(&natural_bases(&["helms"], 3, 60, Wearer::Player(1), None)).contains(&"Wolf Head"));
    }

    #[test]
    fn the_act2_merc_wields_only_polearms_and_spears() -> anyhow::Result<()> {
        let picks = natural_bases(&["weapons"], 4, 60, Wearer::Merc(MercAct::Act2), Some(4));
        assert!(names(&picks).contains(&"Voulge"), "{:?}", names(&picks));
        for p in &picks {
            let (_, _, ty, _, _, _) = SOCKETABLE
                .iter()
                .find(|row| row.1 == p.name)
                .ok_or_else(|| anyhow::anyhow!("{}", p.name))?;
            assert!(matches!(*ty, "pole" | "spea"), "{} is {ty}", p.name);
        }
        // Amazon spears are class-only: never for the merc.
        assert!(!names(&picks).contains(&"Maiden Spear"), "{:?}", names(&picks));
        let insight = crate::runewords_data::lookup("Insight").ok_or_else(|| anyhow::anyhow!("Insight"))?;
        let text = hint(insight, 45, Wearer::Merc(MercAct::Act2), Some(4))
            .ok_or_else(|| anyhow::anyhow!("no hint"))?;
        assert!(text.starts_with("4 sockets: Voulge (50 Str)"), "{text}");
        assert!(!text.contains("Staff") && !text.contains("Bow"), "{text}");
        // Its helm and body armor stay unfiltered by type (class-only pelts still out).
        let helms = names(&natural_bases(
            &["helms"],
            2,
            60,
            Wearer::Merc(MercAct::Act2),
            Some(1),
        ));
        assert!(
            helms.contains(&"Cap") && !helms.contains(&"Wolf Head"),
            "{helms:?}"
        );
        Ok(())
    }

    #[test]
    fn the_merc_helm_and_body_rank_by_defense() -> anyhow::Result<()> {
        let helms = natural_bases(&["helms"], 3, 60, Wearer::Merc(MercAct::Act2), Some(1));
        let names = names(&helms);
        assert!(
            !names.contains(&"Wolf Head") && !names.contains(&"Jawbone Cap"),
            "{names:?}"
        );
        assert!(
            helms
                .windows(2)
                .all(|w| w.first().map(|p| p.def) >= w.get(1).map(|p| p.def)),
            "{names:?}"
        );
        let bulwark = crate::runewords_data::lookup("Bulwark").ok_or_else(|| anyhow::anyhow!("Bulwark"))?;
        let text = hint(bulwark, 99, Wearer::Merc(MercAct::Act2), Some(1))
            .ok_or_else(|| anyhow::anyhow!("no hint"))?;
        assert!(
            text.starts_with("3 sockets: Corona (def 165, 174 Str), Spired Helm (def 159, 192 Str)"),
            "{text}"
        );
        // The merc's weapon and the player's helms keep the requirement order.
        let druid = hint(bulwark, 99, DRUID, Some(1)).ok_or_else(|| anyhow::anyhow!("no hint"))?;
        assert!(!druid.contains("(def "), "{druid}");
        let body = natural_bases(&["armor"], 4, 60, Wearer::Merc(MercAct::Act2), Some(3));
        assert!(
            body.windows(2)
                .all(|w| w.first().map(|p| p.def) >= w.get(1).map(|p| p.def))
        );
        Ok(())
    }

    #[test]
    fn fits_checks_category_wearer_and_slot() -> anyhow::Result<()> {
        let harmony = crate::runewords_data::lookup("Harmony").ok_or_else(|| anyhow::anyhow!("Harmony"))?;
        let spirit = crate::runewords_data::lookup("Spirit").ok_or_else(|| anyhow::anyhow!("Spirit"))?;
        // Hunter's Bow (`hbw`): a missile weapon the druid can use, not the merc.
        assert!(fits(harmony, "hbw", DRUID, None));
        assert!(fits(harmony, "hbw", DRUID, Some(4)));
        assert!(!fits(harmony, "hbw", DRUID, Some(5)), "not an off-hand");
        assert!(
            !fits(harmony, "hbw", Wearer::Merc(MercAct::Act2), None),
            "merc: polearms and spears only"
        );
        assert!(!fits(harmony, "crs", DRUID, None), "a sword is no missile weapon");
        // Spirit: a Crystal Sword in the weapon slot, not the shield slot.
        assert!(fits(spirit, "crs", DRUID, Some(4)));
        assert!(!fits(spirit, "crs", DRUID, Some(5)));
        assert!(!fits(spirit, "hp1", DRUID, None), "a potion is no base");
        assert!(socketable("crs") && !socketable("hp1"));
        Ok(())
    }

    #[test]
    fn quest_items_never_show() {
        let picks = natural_bases(&["weapons"], 1, 99, DRUID, None);
        assert!(
            !names(&picks)
                .iter()
                .any(|n| n.contains("Hellforge") || n.contains("Khalim"))
        );
    }

    #[test]
    fn hint_text_for_harmony_and_spirit() -> anyhow::Result<()> {
        let harmony = crate::runewords_data::lookup("Harmony").ok_or_else(|| anyhow::anyhow!("Harmony"))?;
        let text = hint(harmony, 45, DRUID, None).ok_or_else(|| anyhow::anyhow!("no hint"))?;
        assert!(
            text.starts_with("4 sockets: Hunter's Bow (28 Dex), Long Bow (22/19)"),
            "{text}"
        );
        // Spirit: swords and shields; the weapon slot (body 4) keeps only the swords.
        let spirit = crate::runewords_data::lookup("Spirit").ok_or_else(|| anyhow::anyhow!("Spirit"))?;
        let text = hint(spirit, 45, DRUID, Some(4)).ok_or_else(|| anyhow::anyhow!("no hint"))?;
        assert!(text.starts_with("4 sockets: Crystal Sword (43 Str)"), "{text}");
        assert!(
            hint(spirit, 10, DRUID, Some(4)).is_none(),
            "nothing has 4 sockets at ilvl 10"
        );
        Ok(())
    }

    /// The raw item types of `picks` ([`SOCKETABLE`]).
    fn types(picks: &[Pick]) -> Vec<&'static str> {
        picks
            .iter()
            .filter_map(|p| SOCKETABLE.iter().find(|row| row.1 == p.name).map(|row| row.2))
            .collect()
    }

    #[test]
    fn merc_acts_by_number() {
        for act in [MercAct::Act1, MercAct::Act2, MercAct::Act3, MercAct::Act5] {
            assert_eq!(MercAct::from_number(act.number()), Some(act));
        }
        assert_eq!(MercAct::from_number(4), None, "Act 4 has no mercenary");
        assert_eq!(MercAct::default(), MercAct::Act2);
    }

    #[test]
    fn each_merc_act_wields_its_own_weapons() {
        let weapons = |act| natural_bases(&["weapons"], 3, 99, Wearer::Merc(act), Some(4));
        // Act 1 rogue: bows, no crossbows, no amazon bows.
        let rogue = weapons(MercAct::Act1);
        assert!(names(&rogue).contains(&"Long Bow"), "{:?}", names(&rogue));
        assert!(types(&rogue).iter().all(|&t| t == "bow"), "{:?}", types(&rogue));
        assert!(!names(&rogue).contains(&"Stag Bow"));
        // Act 3 and Act 5: one-handed swords only (the Act 5 one per the user: dual wield).
        for act in [MercAct::Act3, MercAct::Act5] {
            let swords = weapons(act);
            assert!(names(&swords).contains(&"Broad Sword"), "{:?}", names(&swords));
            assert!(
                types(&swords).iter().all(|&t| t == "swor"),
                "{:?}",
                types(&swords)
            );
            for two in ["Zweihander", "Two-Handed Sword", "Claymore"] {
                assert!(!names(&swords).contains(&two), "{act:?}: {two}");
            }
        }
        // The Act 5 merc's off-hand takes a second one-handed sword (dual wield, per the user);
        // no other act's off-hand takes a weapon, and only Act 3 takes a shield.
        let second = natural_bases(&["swords"], 3, 99, Wearer::Merc(MercAct::Act5), Some(5));
        assert!(names(&second).contains(&"Broad Sword") && !names(&second).contains(&"Claymore"));
        assert_eq!(
            natural_bases(&["swords"], 3, 99, Wearer::Merc(MercAct::Act3), Some(5)),
            vec![]
        );
        assert_ne!(
            natural_bases(&["shields"], 3, 99, Wearer::Merc(MercAct::Act3), Some(5)),
            vec![]
        );
        assert_eq!(
            natural_bases(&["shields"], 3, 99, Wearer::Merc(MercAct::Act5), Some(5)),
            vec![]
        );
        // Javelins are spears in itemtypes.txt, but never a merc weapon.
        assert!(!merc_wears("jav", MercAct::Act2));
        assert!(merc_wears("spr", MercAct::Act2) && !merc_wears("spr", MercAct::Act5));
        // Armor fits every act; a shield only the Act 3 merc.
        assert!(merc_wears("cap", MercAct::Act5) && merc_wears("qui", MercAct::Act1));
        assert!(merc_wears("lrg", MercAct::Act3) && !merc_wears("lrg", MercAct::Act5));
        assert!(merc_wears("bsd", MercAct::Act5) && !merc_wears("9fb", MercAct::Act5));
    }

    #[test]
    fn a_weapon_beside_an_offhand_must_be_one_handed() -> anyhow::Result<()> {
        let lawbringer =
            crate::runewords_data::lookup("Lawbringer").ok_or_else(|| anyhow::anyhow!("Lawbringer"))?;
        let with_shield = Wearer::PlayerWithOffhand(5);
        // Broad Sword: one-handed. Zweihander (`9fb`): two-handed for a druid.
        assert!(fits(lawbringer, "bsd", with_shield, Some(4)));
        assert!(!fits(lawbringer, "9fb", with_shield, Some(4)));
        assert!(two_handed_misfit(lawbringer, "9fb", with_shield, Some(4)));
        assert!(!two_handed_misfit(lawbringer, "bsd", with_shield, Some(4)));
        // Without an off-hand both hands are free.
        assert!(fits(lawbringer, "9fb", DRUID, Some(4)));
        assert!(!two_handed_misfit(lawbringer, "9fb", DRUID, Some(4)));
        // A barbarian wields a `1or2handed` sword in one hand; a bow takes both hands for all.
        assert!(fits(lawbringer, "9fb", Wearer::PlayerWithOffhand(4), Some(4)));
        assert!(!two_handed_for("9fb", Wearer::Player(4)) && two_handed_for("9fb", DRUID));
        assert!(two_handed_for("hbw", Wearer::Player(4)));
        assert!(!two_handed_for("cap", DRUID), "no weapon");
        // The Act 5 merc is a barbarian, but its swords are planned one-handed.
        assert!(!fits(lawbringer, "9fb", Wearer::Merc(MercAct::Act5), Some(4)));
        assert!(two_handed_misfit(
            lawbringer,
            "9fb",
            Wearer::Merc(MercAct::Act5),
            Some(4)
        ));
        // The hint drops the two-handers too.
        let hint = hint(lawbringer, 99, with_shield, Some(4)).ok_or_else(|| anyhow::anyhow!("no hint"))?;
        assert!(!hint.contains("Claymore") && !hint.contains("Maul"), "{hint}");
        let free = super::hint(lawbringer, 99, DRUID, Some(4)).ok_or_else(|| anyhow::anyhow!("no hint"))?;
        assert!(free.contains("Claymore") && free.contains("Maul"), "{free}");
        Ok(())
    }
}

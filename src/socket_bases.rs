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

/// Who will wear the base.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wearer {
    /// The player character of this class (player txt row, 0 amazon .. 7 warlock): bases
    /// restricted to another class are skipped.
    Player(u32),
    /// The mercenary, always taken as an Act 2 Desert Mercenary: the live snapshot doesn't read
    /// the hireling's act, and every build here uses Act 2. A mercenary can't wear class-only
    /// bases (pelts, barbarian helms, ...; circlets have no class and stay in), and its weapons
    /// are only polearms and spears ([`ACT2_MERC_WEAPONS`]).
    Act2Merc,
}

/// The item types an Act 2 mercenary wields (polearms, spears). Amazon spears (`aspe`, an
/// Equiv of `spea`) are class-only and drop out through the class check.
const ACT2_MERC_WEAPONS: [&str; 2] = ["pole", "spea"];

/// Whether `wearer` can use a base of raw item type `ty`, body slot `slot` and class
/// restriction `class`.
fn wearable(wearer: Wearer, ty: &str, slot: u8, class: Option<u8>) -> bool {
    match wearer {
        Wearer::Player(c) => class.is_none_or(|only| u32::from(only) == c),
        Wearer::Act2Merc => {
            class.is_none() && (slot != 4 || ACT2_MERC_WEAPONS.iter().any(|w| is_a(ty, w, 0)))
        }
    }
}

/// Whether the hint ranks by defense: the mercenary's helm or body armor (user request,
/// 2026-11-20: the merc wants the strongest base, its strength is no concern of the player's).
fn by_defense(wearer: Wearer, slot: Option<u8>) -> bool {
    wearer == Wearer::Act2Merc && matches!(slot, Some(1 | 3))
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
/// `max_ilvl`, that `wearer` can use ([`Wearer`]) and, with `slot` set, that is worn in that
/// body slot.
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
                && slot.is_none_or(|s| s == base.slot))
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
        let picks = natural_bases(&["weapons"], 4, 60, Wearer::Act2Merc, Some(4));
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
        let text = hint(insight, 45, Wearer::Act2Merc, Some(4)).ok_or_else(|| anyhow::anyhow!("no hint"))?;
        assert!(text.starts_with("4 sockets: Voulge (50 Str)"), "{text}");
        assert!(!text.contains("Staff") && !text.contains("Bow"), "{text}");
        // Its helm and body armor stay unfiltered by type (class-only pelts still out).
        let helms = names(&natural_bases(&["helms"], 2, 60, Wearer::Act2Merc, Some(1)));
        assert!(
            helms.contains(&"Cap") && !helms.contains(&"Wolf Head"),
            "{helms:?}"
        );
        Ok(())
    }

    #[test]
    fn the_merc_helm_and_body_rank_by_defense() -> anyhow::Result<()> {
        let helms = natural_bases(&["helms"], 3, 60, Wearer::Act2Merc, Some(1));
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
        let text = hint(bulwark, 99, Wearer::Act2Merc, Some(1)).ok_or_else(|| anyhow::anyhow!("no hint"))?;
        assert!(
            text.starts_with("3 sockets: Corona (def 165, 174 Str), Spired Helm (def 159, 192 Str)"),
            "{text}"
        );
        // The merc's weapon and the player's helms keep the requirement order.
        let druid = hint(bulwark, 99, DRUID, Some(1)).ok_or_else(|| anyhow::anyhow!("no hint"))?;
        assert!(!druid.contains("(def "), "{druid}");
        let body = natural_bases(&["armor"], 4, 60, Wearer::Act2Merc, Some(3));
        assert!(
            body.windows(2)
                .all(|w| w.first().map(|p| p.def) >= w.get(1).map(|p| p.def))
        );
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
}

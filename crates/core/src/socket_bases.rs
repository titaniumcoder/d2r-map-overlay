//! Natural-socket white bases to farm for a runeword. A base drops with at most
//! `min(gemsockets, item-type cap)` sockets, the cap rising at item levels 26 and 41.

use crate::bases_data::{self, ITEM_TYPES, SOCKETABLE};
use crate::runewords_data::Runeword;

/// Item-type code of each readable runeword base category; mirrors `research/gen_runewords.py`.
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

/// The act a mercenary was hired in, which decides the weapons it wields.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MercAct {
    /// Rogue Scout: bows, amazon bows included since D2R 2.4.
    Act1,
    /// Desert Mercenary: polearms and spears. The default when the act is unknown.
    #[default]
    Act2,
    /// Iron Wolf: a one-handed sword and a shield.
    Act3,
    /// Barbarian: dual-wields one-handed swords (the Frenzy merc type, D2R 2.4).
    Act5,
}

impl MercAct {
    /// Act 4 has no mercenary.
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

    #[must_use]
    pub const fn number(self) -> u8 {
        match self {
            Self::Act1 => 1,
            Self::Act2 => 2,
            Self::Act3 => 3,
            Self::Act5 => 5,
        }
    }

    const fn weapons(self) -> &'static [&'static str] {
        match self {
            Self::Act1 => &["bow"],
            Self::Act2 => &["pole", "spea"],
            Self::Act3 | Self::Act5 => &["swor"],
        }
    }

    /// The class-only bases it can use as (body slot, class id): since D2R 2.4 the rogue uses
    /// amazon bows and the barbarian barbarian helms.
    const fn class_items(self) -> Option<(u8, u8)> {
        match self {
            Self::Act1 => Some((4, 0)),
            Self::Act5 => Some((1, 4)),
            Self::Act2 | Self::Act3 => None,
        }
    }

    const fn one_handed(self) -> bool {
        matches!(self, Self::Act3 | Self::Act5)
    }

    const fn dual_wields(self) -> bool {
        matches!(self, Self::Act5)
    }
}

/// Who will wear the base.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Wearer {
    /// The player character of this class (player txt row, 0 amazon .. 7 warlock).
    Player(u32),
    /// The player character of this class with a shield: one-handed weapons only.
    PlayerWithOffhand(u32),
    /// The mercenary of this act: no class-only bases except [`MercAct::class_items`].
    Merc(MercAct),
}

impl Wearer {
    const fn one_hand_only(self) -> bool {
        match self {
            Self::Player(_) => false,
            Self::PlayerWithOffhand(_) => true,
            Self::Merc(act) => act.one_handed(),
        }
    }
}

/// Whether `wearer` can use a base of item type `ty`, body slot `slot` and class `class`.
fn wearable(wearer: Wearer, ty: &str, slot: u8, class: Option<u8>) -> bool {
    match wearer {
        Wearer::Player(c) | Wearer::PlayerWithOffhand(c) => class.is_none_or(|only| u32::from(only) == c),
        // Javelins are spears in itemtypes.txt (Equiv2), but no merc weapon.
        Wearer::Merc(act) => {
            class.is_none_or(|only| act.class_items() == Some((slot, only)))
                && match slot {
                    4 => act.weapons().iter().any(|w| is_a(ty, w, 0)) && !is_a(ty, "jave", 0),
                    5 => act == MercAct::Act3,
                    _ => true,
                }
        }
    }
}

/// Whether weapon `code` takes both hands for `wearer`; a barbarian player wields a
/// `1or2handed` sword in one hand. `false` for a non-weapon.
#[must_use]
pub fn two_handed_for(code: &str, wearer: Wearer) -> bool {
    let barbarian = matches!(wearer, Wearer::Player(4) | Wearer::PlayerWithOffhand(4));
    crate::weapons_data::two_handed(code) == Some(true)
        && !(barbarian && crate::weapons_data::one_or_two_handed(code))
}

/// What a build's own `weapon` slot takes on top of what the class can hold (a build file's
/// `weapon_rule`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct WeaponRule {
    /// Two-handed weapons only, as the wearer holds them ([`two_handed_for`]).
    pub two_handed: bool,
    /// No missile weapons (bows, crossbows): a shapeshifted druid can't shoot.
    pub melee: bool,
}

impl WeaponRule {
    /// Whether weapon base `code` keeps to the rule for `wearer`.
    #[must_use]
    pub fn allows(self, code: &str, wearer: Wearer) -> bool {
        let missile = bases_data::base(code).is_some_and(|b| is_a(b.kind, "miss", 0));
        (!self.two_handed || two_handed_for(code, wearer)) && !(self.melee && missile)
    }
}

/// Whether a base of body slot `base_slot` goes in the wanted `slot` (`None`: any); a
/// dual-wielding merc's off-hand takes a weapon.
fn in_slot(wearer: Wearer, slot: Option<u8>, base_slot: u8) -> bool {
    let dual = matches!(wearer, Wearer::Merc(act) if act.dual_wields());
    slot.is_none_or(|s| s == base_slot || (dual && s == 5 && base_slot == 4))
}

fn in_hand(code: &str, wearer: Wearer) -> bool {
    !wearer.one_hand_only() || !two_handed_for(code, wearer)
}

/// Whether the mercenary of `act` can wear the base with item code `code`.
#[must_use]
pub fn merc_wears(code: &str, act: MercAct) -> bool {
    let wearer = Wearer::Merc(act);
    bases_data::base(code).is_some_and(|b| {
        matches!(b.slot, 1 | 3 | 4 | 5) && wearable(wearer, b.kind, b.slot, b.class) && in_hand(code, wearer)
    })
}

/// The mercenary's helm or body armor is ranked by defense: its strength is no concern.
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

fn item_type(code: &str) -> Option<&'static bases_data::TypeRow> {
    let i = ITEM_TYPES.binary_search_by(|row| row.0.cmp(code)).ok()?;
    ITEM_TYPES.get(i)
}

/// Whether item type `ty` is `want` or reaches it through its Equiv1/Equiv2 parents.
fn is_a(ty: &str, want: &str, depth: u8) -> bool {
    if ty == want {
        return true;
    }
    let Some(&(_, e1, e2, _)) = item_type(ty) else {
        return false;
    };
    depth < 8 && [e1, e2].iter().any(|e| !e.is_empty() && is_a(e, want, depth + 1))
}

/// Type cap on sockets at item level `ilvl` (the base's own `gemsockets` limits further).
fn type_cap(ty: &str, ilvl: u32) -> u8 {
    item_type(ty).map_or(0, |&(_, _, _, [low, mid, high])| match ilvl {
        0..=25 => low,
        26..=40 => mid,
        _ => high,
    })
}

/// Every base of one of `categories` that can drop with exactly `sockets` sockets by item
/// level `max_ilvl`, usable by `wearer` (in `slot` if set). Sorted by str + dex requirement,
/// quality level and name; merc helms and armor by defense first.
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
        let def = std::cmp::Reverse(if defense { p.def } else { 0 });
        (def, p.str.saturating_add(p.dex), qlvl, name)
    });
    picks.into_iter().map(|(_, _, p)| p).collect()
}

fn socketable_row(code: &str) -> Option<&'static bases_data::SocketRow> {
    let i = SOCKETABLE.binary_search_by(|row| row.0.cmp(code)).ok()?;
    SOCKETABLE.get(i)
}

/// Whether item code `code` is a weapon or armor base that can have sockets (a runeword base).
#[must_use]
pub fn socketable(code: &str) -> bool {
    socketable_row(code).is_some()
}

/// The maximum base defense of socketable armor `code`; `None` for weapons.
#[must_use]
pub fn max_defense(code: &str) -> Option<u16> {
    socketable_row(code)
        .map(|&(_, _, _, _, _, def)| def)
        .filter(|&def| def > 0)
}

/// Whether base `code` can carry runeword `word` for `wearer` (in `slot` if set), sockets aside.
#[must_use]
pub fn fits(word: &Runeword, code: &str, wearer: Wearer, slot: Option<u8>) -> bool {
    fits_type(word, code, wearer, slot) && in_hand(code, wearer)
}

/// Whether base `code` would [`fit`](fits) `word` but takes both hands where one is busy.
#[must_use]
pub fn two_handed_misfit(word: &Runeword, code: &str, wearer: Wearer, slot: Option<u8>) -> bool {
    fits_type(word, code, wearer, slot) && !in_hand(code, wearer)
}

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

/// Whether base `code` [`fits`] `word` and can roll its socket count at the highest item level.
#[must_use]
pub fn carries(word: &Runeword, code: &str, wearer: Wearer, slot: Option<u8>) -> bool {
    socketable_row(code).is_some_and(|&(_, _, ty, max, _, _)| {
        usize::from(max.min(type_cap(ty, u32::MAX))) >= word.runes.len()
    }) && fits(word, code, wearer, slot)
}

/// The first of `acts` whose mercenary can carry `word` in `slot` on some base.
#[must_use]
pub fn merc_act_for(word: &Runeword, acts: &[MercAct], slot: Option<u8>) -> Option<MercAct> {
    acts.iter().copied().find(|&act| {
        SOCKETABLE
            .iter()
            .any(|row| carries(word, row.0, Wearer::Merc(act), slot))
    })
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

/// The dashboard's base hint for `word`, e.g. `"4 sockets: Hunter's Bow (28 Dex), …"`: the
/// cheapest four bases, or three per category when several categories have some.
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
    fn a_weapon_rule_takes_out_bows_and_one_handers() {
        let fury = WeaponRule {
            two_handed: true,
            melee: true,
        };
        // Gothic Bow, Colossus Crossbow: missile weapons. Broad Sword: one-handed.
        assert!(!fury.allows("8lw", DRUID) && !fury.allows("6hx", DRUID));
        assert!(!fury.allows("bsd", DRUID));
        // Legend Sword and Zweihander: 1-or-2-handed swords a druid holds in both hands.
        assert!(fury.allows("72h", DRUID) && fury.allows("9fb", DRUID));
        assert!(
            !fury.allows("72h", Wearer::Player(4)),
            "a barbarian holds it in one hand"
        );
        let any = WeaponRule::default();
        assert!(any.allows("8lw", DRUID) && any.allows("bsd", DRUID));
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
        // Act 1 rogue: bows and amazon bows (D2R 2.4), no crossbows, no amazon spears.
        let rogue = weapons(MercAct::Act1);
        assert!(names(&rogue).contains(&"Long Bow"), "{:?}", names(&rogue));
        assert!(names(&rogue).contains(&"Stag Bow"), "{:?}", names(&rogue));
        assert!(
            types(&rogue).iter().all(|&t| matches!(t, "bow" | "abow")),
            "{:?}",
            types(&rogue)
        );
        assert!(merc_wears("am1", MercAct::Act1) && !merc_wears("am1", MercAct::Act2));
        assert!(!merc_wears("am3", MercAct::Act1), "Maiden Spear: amazon spear");
        assert!(!merc_wears("am5", MercAct::Act1), "Maiden Javelin");
        assert!(!merc_wears("lxb", MercAct::Act1), "Light Crossbow");
        // Act 3 and Act 5: one-handed swords only.
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
        // The Act 5 merc's off-hand takes a second one-handed sword;
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
        // Barbarian helms only for the Act 5 merc.
        assert!(merc_wears("ba1", MercAct::Act5) && !merc_wears("ba1", MercAct::Act2));
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

    #[test]
    fn the_act1_rogue_carries_harmony_on_an_amazon_bow() -> anyhow::Result<()> {
        let harmony = crate::runewords_data::lookup("Harmony").ok_or_else(|| anyhow::anyhow!("Harmony"))?;
        let rogue = Wearer::Merc(MercAct::Act1);
        // Matriarchal Bow (`amb`): an amazon bow that can drop with 4 sockets.
        assert!(carries(harmony, "amb", rogue, Some(4)));
        assert!(carries(harmony, "hbw", rogue, Some(4)), "Hunter's Bow");
        assert!(!carries(harmony, "amb", DRUID, Some(4)), "the druid can't");
        let picks = names(&natural_bases(&["missile weapons"], 4, 99, rogue, Some(4)));
        assert!(
            picks.contains(&"Matriarchal Bow") && picks.contains(&"Hunter's Bow"),
            "{picks:?}"
        );
        // A build with an Act 2 and an Act 1 merc: Insight for the Act 2 one, Harmony the rogue.
        let insight = crate::runewords_data::lookup("Insight").ok_or_else(|| anyhow::anyhow!("Insight"))?;
        let acts = [MercAct::Act2, MercAct::Act1];
        assert_eq!(merc_act_for(insight, &acts, Some(4)), Some(MercAct::Act2));
        assert_eq!(merc_act_for(harmony, &acts, Some(4)), Some(MercAct::Act1));
        assert_eq!(merc_act_for(harmony, &[MercAct::Act5], Some(4)), None);
        Ok(())
    }

    #[test]
    fn the_act5_merc_helm_can_be_a_barbarian_helm() -> anyhow::Result<()> {
        let bulwark = crate::runewords_data::lookup("Bulwark").ok_or_else(|| anyhow::anyhow!("Bulwark"))?;
        let barbarian = Wearer::Merc(MercAct::Act5);
        // Savage Helmet (`ba9`): a barbarian helm with 3 sockets.
        assert!(carries(bulwark, "ba9", barbarian, Some(1)));
        assert!(!carries(bulwark, "ba9", Wearer::Merc(MercAct::Act2), Some(1)));
        let helms = names(&natural_bases(&["helms"], 3, 99, barbarian, Some(1)));
        assert!(
            helms.contains(&"Savage Helmet") && !helms.contains(&"Wolf Head"),
            "{helms:?}"
        );
        Ok(())
    }
}

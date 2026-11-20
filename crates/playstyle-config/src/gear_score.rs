//! Stat-based item comparison: scores an item's stats for the character's guide stage and
//! compares a store item against what is worn in the same body slot. The vendor panel marks
//! an item "better" when the gain passes [`MIN_GAIN`] (user request, 2026-11-18: leveling
//! help, mark only, small gains ignored).
//!
//! An item's stats are its stat-list chain ([`ItemStat`] in [`d2r_core::item_stats`], read by
//! `game::Reader`): the BASE list holds the rolled base values (defense `31` = the base
//! defense, damage pairs `21/22` one-handed, `23/24` two-handed, `159/160` thrown; live
//! 2026-11-18 on a level-4 amazon:
//! Short Bow `23/24` = 1/4, Javelin `21/22` = 1/5 and `159/160` = 6/14, Hard Leather Armor
//! `31` = 25), the affix lists the modifiers (ED% `16` on armor, `17/18` on weapons, res,
//! +skills, flat damage …). ED% applies to the base value only; flat bonuses add after.
//! That split is all the scoring needs it for. Normally the reader takes the item's
//! aggregated array instead ([`ItemStat::from_totals`], live 2026-11-19): every stat once,
//! including runeword and socketed runes (the chain misses those), defense and damage already
//! enhanced; it comes in as base defense/damage with no ED% left, so the same formula yields
//! the totals unchanged.
//!
//! Slot rule ([`verdict`]): a store item is compared with the item worn in its body slot
//! (an empty slot scores 0). Rings: against the worse of the two rings. A two-handed weapon:
//! against the weapon plus whatever the off-hand holds (it would have to come off). A
//! one-handed weapon: against the weapon only. A shield while a two-handed weapon is worn:
//! not compared (it can't be used with it).

use std::collections::BTreeMap;

use d2r_core::bases_data::{self, Base};
use d2r_core::item_stats::{ItemStat, stat};
use d2r_core::skills_data;

/// Score weights: score points per unit of a stat (per +1 skill, per point of average
/// damage, per % of resistance …). The one table to tune; [`MIN_GAIN`] is the bar.
#[derive(Debug)]
pub struct Weights {
    /// +all skills.
    pub all_skills: f64,
    /// +class skills of the own class.
    pub class_skills: f64,
    /// +skill tab holding a planned skill / any other own-class tab.
    pub tab_planned: f64,
    pub tab_other: f64,
    /// +single skill that is planned / any other.
    pub skill_planned: f64,
    pub skill_other: f64,
    /// Average weapon damage (incl. ED%, +min/+max, elemental), only for the weapon kind the
    /// character uses and not for casters.
    pub avg_damage: f64,
    /// Each elemental resistance (fire, cold, lightning, poison), per %.
    pub resist: f64,
    pub life: f64,
    /// Mana for casters / for everyone else.
    pub mana_caster: f64,
    pub mana: f64,
    pub strength: f64,
    pub dexterity: f64,
    pub vitality: f64,
    /// Energy for casters / for everyone else.
    pub energy_caster: f64,
    pub energy: f64,
    /// FCR for casters / for everyone else.
    pub fcr_caster: f64,
    pub fcr: f64,
    /// IAS, attack rating and life leech for weapon users only.
    pub ias: f64,
    pub attack_rating: f64,
    pub life_leech: f64,
    pub mana_leech: f64,
    pub fhr: f64,
    pub frw: f64,
    pub defense: f64,
    pub magic_find: f64,
    pub replenish_life: f64,
}

/// The weights in use.
pub const WEIGHTS: Weights = Weights {
    all_skills: 40.0,
    class_skills: 40.0,
    tab_planned: 30.0,
    tab_other: 3.0,
    skill_planned: 20.0,
    skill_other: 1.0,
    avg_damage: 2.0,
    resist: 1.0,
    life: 1.0,
    mana_caster: 1.0,
    mana: 0.2,
    strength: 0.5,
    dexterity: 0.5,
    vitality: 1.5,
    energy_caster: 0.5,
    energy: 0.1,
    fcr_caster: 1.0,
    fcr: 0.2,
    ias: 0.8,
    attack_rating: 0.05,
    life_leech: 3.0,
    mana_leech: 1.5,
    fhr: 0.5,
    frw: 0.8,
    defense: 0.1,
    magic_find: 0.2,
    replenish_life: 1.0,
};

/// The smallest score gain marked "better": 8 points = +4 average damage, +8 resistance,
/// +80 defense, or any planned +skill.
pub const MIN_GAIN: f64 = 8.0;

/// What the character values, from its class, guide stage and gear.
#[derive(Debug, Clone, Default)]
pub struct Profile {
    /// Class (player txt row, 0 amazon .. 7 warlock).
    pub class: u32,
    pub level: Option<u32>,
    /// Strength and dexterity (base plus gear); `None` when unread (the requirement is then
    /// not checked).
    pub strength: Option<i32>,
    pub dexterity: Option<i32>,
    /// The skills the character uses: the stage's skill plan plus its allocated skills.
    pub skills: Vec<u16>,
    /// The weapon kind it fights with ([`Base::kind`]); `None` = unknown (every kind's
    /// damage counts).
    pub weapon: Option<&'static str>,
    /// A caster: weapon damage, IAS, AR and leech don't count; mana, energy and FCR more.
    pub caster: bool,
}

impl Profile {
    /// Whether one of the character's skills is on tab `param` (class × 8 + tab).
    fn plans_tab(&self, param: u16) -> bool {
        self.skills
            .iter()
            .filter_map(|&id| skills_data::skill(id))
            .any(|s| {
                u32::from(s.class) == self.class
                    && (u16::from(s.class) * 8 + u16::from(s.page)).checked_sub(1) == Some(param)
            })
    }
}

/// The weapon kinds a caster holds (staves, wands, orbs).
const CASTER_KINDS: [&str; 3] = ["staf", "wand", "orb"];

/// Whether fighting with weapon kind `kind` (or none, `None`) makes class `class` a caster:
/// a caster weapon, or no weapon at all on a sorceress or necromancer.
#[must_use]
pub fn is_caster(class: u32, kind: Option<&str>) -> bool {
    kind.map_or(matches!(class, 1 | 2), |k| CASTER_KINDS.contains(&k))
}

/// The weapon kind a build piece names: the longest weapon base name inside it ("Javelin of
/// Swiftness" → `jave`, "Gemmed Hunter's Bow (Chipped Rubies)" → `bow`); `None` for named
/// uniques and runewords (no base name).
#[must_use]
pub fn piece_kind(piece: &str) -> Option<&'static str> {
    let piece = piece.to_lowercase();
    d2r_core::weapons_data::WEAPONS
        .iter()
        .filter(|w| !w.5.is_empty() && piece.contains(&w.5.to_lowercase()))
        .max_by_key(|w| w.5.len())
        .and_then(|w| bases_data::base(w.0))
        .map(|b| b.kind)
}

/// One scored aspect of an item; the `why` text names the biggest differences.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
enum Part {
    AllSkills,
    ClassSkills,
    Tab(u16),
    Skill(u16),
    Damage,
    ResFire,
    ResCold,
    ResLightning,
    ResPoison,
    Life,
    Mana,
    Strength,
    Dexterity,
    Vitality,
    Energy,
    Fcr,
    Ias,
    Fhr,
    Frw,
    Defense,
    MagicFind,
    AttackRating,
    LifeLeech,
    ManaLeech,
    ReplenishLife,
}

impl Part {
    /// Score points per unit for `who` ([`WEIGHTS`]).
    fn weight(self, who: &Profile) -> f64 {
        let w = &WEIGHTS;
        let fighter = if who.caster { 0.0 } else { 1.0 };
        match self {
            Self::AllSkills => w.all_skills,
            Self::ClassSkills => w.class_skills,
            Self::Tab(t) if who.plans_tab(t) => w.tab_planned,
            Self::Tab(_) => w.tab_other,
            Self::Skill(s) if who.skills.contains(&s) => w.skill_planned,
            Self::Skill(_) => w.skill_other,
            Self::Damage => w.avg_damage * fighter,
            Self::ResFire | Self::ResCold | Self::ResLightning | Self::ResPoison => w.resist,
            Self::Life => w.life,
            Self::Mana if who.caster => w.mana_caster,
            Self::Mana => w.mana,
            Self::Strength => w.strength,
            Self::Dexterity => w.dexterity,
            Self::Vitality => w.vitality,
            Self::Energy if who.caster => w.energy_caster,
            Self::Energy => w.energy,
            Self::Fcr if who.caster => w.fcr_caster,
            Self::Fcr => w.fcr,
            Self::Ias => w.ias * fighter,
            Self::AttackRating => w.attack_rating * fighter,
            Self::LifeLeech => w.life_leech * fighter,
            Self::ManaLeech => w.mana_leech,
            Self::Fhr => w.fhr,
            Self::Frw => w.frw,
            Self::Defense => w.defense,
            Self::MagicFind => w.magic_find,
            Self::ReplenishLife => w.replenish_life,
        }
    }

    /// The `why` text for a difference of `amount` units ("+24 avg dmg", "+2 Bow and
    /// Crossbow skills").
    fn describe(self, amount: f64) -> String {
        let n = if (amount - amount.round()).abs() < 0.05 {
            format!("{amount:+.0}")
        } else {
            format!("{amount:+.1}")
        };
        let class_name = |c: u16| skills_data::TREES.get(usize::from(c / 8));
        match self {
            Self::AllSkills => format!("{n} all skills"),
            Self::ClassSkills => format!("{n} class skills"),
            Self::Tab(t) => class_name(t)
                .and_then(|tabs| tabs.get(usize::from(t % 8)))
                .map_or_else(|| format!("{n} tab skills"), |tab| format!("{n} {tab} skills")),
            Self::Skill(s) => format!("{n} {}", skills_data::name(s).unwrap_or("skill")),
            Self::Damage => format!("{n} avg dmg"),
            Self::ResFire => format!("{n} fire res"),
            Self::ResCold => format!("{n} cold res"),
            Self::ResLightning => format!("{n} light res"),
            Self::ResPoison => format!("{n} poison res"),
            Self::Life => format!("{n} life"),
            Self::Mana => format!("{n} mana"),
            Self::Strength => format!("{n} str"),
            Self::Dexterity => format!("{n} dex"),
            Self::Vitality => format!("{n} vit"),
            Self::Energy => format!("{n} energy"),
            Self::Fcr => format!("{n}% FCR"),
            Self::Ias => format!("{n}% IAS"),
            Self::Fhr => format!("{n}% FHR"),
            Self::Frw => format!("{n}% FRW"),
            Self::Defense => format!("{n} def"),
            Self::MagicFind => format!("{n}% MF"),
            Self::AttackRating => format!("{n} AR"),
            Self::LifeLeech => format!("{n}% life leech"),
            Self::ManaLeech => format!("{n}% mana leech"),
            Self::ReplenishLife => format!("{n} replenish life"),
        }
    }
}

/// An item to score: its code (for the base: slot, kind, requirements) and stats.
#[derive(Debug, Clone, Copy)]
pub struct Piece<'a> {
    pub code: Option<&'a str>,
    pub stats: &'a [ItemStat],
}

/// Sum of the stat `id` entries (base list or affix lists, `base` picks), as `f64`: summed
/// in `i64` so misread values can't overflow (at most 8 × 512 `i32` entries).
fn sum(stats: &[ItemStat], id: u16, base: bool) -> f64 {
    let total: i64 = stats
        .iter()
        .filter(|s| s.id == id && s.base == base)
        .map(|s| i64::from(s.value))
        .sum();
    // Clamped into i32 range, which converts to f64 exactly.
    let clamped = total.clamp(i64::from(i32::MIN), i64::from(i32::MAX));
    i32::try_from(clamped).map_or(0.0, f64::from)
}

/// Sum of the stat `id` entries of any list.
fn total(stats: &[ItemStat], id: u16) -> f64 {
    sum(stats, id, true) + sum(stats, id, false)
}

/// `base × (100 + percent) / 100 + flat`, the game's ED% rule.
fn enhanced(base: f64, percent: f64, flat: f64) -> f64 {
    base * (100.0 + percent) / 100.0 + flat
}

/// The average damage of a weapon of kind `kind` (two-handed `two_handed`, wielded by class
/// `class`) from its stats: the thrown pair for javelins and throwing weapons, the two-handed
/// pair for a two-hander (a barbarian swings a one-or-two-hander one-handed when its base
/// list has the one-handed pair), else the one-handed pair; ED% on the base value. Flat
/// +min/+max are taken from whichever damage stat the affix lists carry (the game's damage
/// properties may write all three pairs or only `21/22`; the largest avoids counting twice);
/// plus the average elemental damage.
fn avg_damage(stats: &[ItemStat], kind: &str, two_handed: bool, class: u32) -> f64 {
    let has_1h = stats.iter().any(|s| s.base && s.id == stat::MIN_DAMAGE);
    let (lo, hi) = if matches!(kind, "jave" | "tkni" | "taxe") {
        (stat::THROW_MIN, stat::THROW_MAX)
    } else if !has_1h || (two_handed && class != 4) {
        (stat::MIN_DAMAGE_2H, stat::MAX_DAMAGE_2H)
    } else {
        (stat::MIN_DAMAGE, stat::MAX_DAMAGE)
    };
    let flat = |ids: [u16; 3]| {
        ids.map(|id| sum(stats, id, false))
            .into_iter()
            .fold(0.0, f64::max)
    };
    let min = enhanced(
        sum(stats, lo, true),
        total(stats, stat::MIN_DAMAGE_PERCENT),
        flat([stat::MIN_DAMAGE, stat::MIN_DAMAGE_2H, stat::THROW_MIN]),
    );
    let max = enhanced(
        sum(stats, hi, true),
        total(stats, stat::MAX_DAMAGE_PERCENT),
        flat([stat::MAX_DAMAGE, stat::MAX_DAMAGE_2H, stat::THROW_MAX]),
    );
    let elemental: f64 = stat::ELEMENTAL
        .iter()
        .map(|&(a, b)| f64::midpoint(total(stats, a), total(stats, b)))
        .sum();
    f64::midpoint(min, max) + elemental
}

/// The scored amounts of `piece` for `who`, in natural units (points, %, avg damage).
fn parts(piece: Piece<'_>, who: &Profile) -> BTreeMap<Part, f64> {
    let stats = piece.stats;
    let base = piece.code.and_then(bases_data::base);
    let mut out = BTreeMap::new();
    let mut add = |part: Part, amount: f64| {
        if amount != 0.0 {
            *out.entry(part).or_insert(0.0) += amount;
        }
    };
    for s in stats {
        let v = f64::from(s.value);
        match s.id {
            stat::ALL_SKILLS => add(Part::AllSkills, v),
            stat::CLASS_SKILLS if u32::from(s.param) == who.class => add(Part::ClassSkills, v),
            stat::SKILL_TAB if u32::from(s.param / 8) == who.class => add(Part::Tab(s.param), v),
            stat::SINGLE_SKILL | stat::NONCLASS_SKILL => add(Part::Skill(s.param), v),
            stat::RES_FIRE => add(Part::ResFire, v),
            stat::RES_COLD => add(Part::ResCold, v),
            stat::RES_LIGHTNING => add(Part::ResLightning, v),
            stat::RES_POISON => add(Part::ResPoison, v),
            stat::MAX_LIFE => add(Part::Life, v),
            stat::MAX_MANA => add(Part::Mana, v),
            stat::STRENGTH => add(Part::Strength, v),
            stat::DEXTERITY => add(Part::Dexterity, v),
            stat::VITALITY => add(Part::Vitality, v),
            stat::ENERGY => add(Part::Energy, v),
            stat::FCR => add(Part::Fcr, v),
            stat::IAS => add(Part::Ias, v),
            stat::FHR => add(Part::Fhr, v),
            stat::FRW => add(Part::Frw, v),
            stat::MAGIC_FIND => add(Part::MagicFind, v),
            stat::ATTACK_RATING => add(Part::AttackRating, v),
            stat::LIFE_LEECH => add(Part::LifeLeech, v),
            stat::MANA_LEECH => add(Part::ManaLeech, v),
            stat::REPLENISH_LIFE => add(Part::ReplenishLife, v),
            _ => {}
        }
    }
    add(
        Part::Defense,
        enhanced(
            sum(stats, stat::DEFENSE, true),
            total(stats, stat::DEFENSE_PERCENT),
            sum(stats, stat::DEFENSE, false),
        )
        .floor(),
    );
    if let Some(b) = base.filter(|b| b.slot == 4)
        && who.weapon.is_none_or(|k| k == b.kind)
    {
        let two_handed = d2r_core::weapons_data::two_handed(b.code).unwrap_or(false);
        add(Part::Damage, avg_damage(stats, b.kind, two_handed, who.class));
    }
    out
}

/// The score of `parts` for `who`.
fn score(parts: &BTreeMap<Part, f64>, who: &Profile) -> f64 {
    parts.iter().map(|(p, v)| p.weight(who) * v).sum()
}

/// Whether `who` can use `base` now: its class and its strength/dexterity requirements
/// (lowered by `Requirements -%`); the level is checked by the caller's `req_level`.
fn usable(base: Base, stats: &[ItemStat], who: &Profile) -> bool {
    let class_ok = base.class.is_none_or(|c| u32::from(c) == who.class);
    // A misread "Requirements -%" below -100 must not make everything usable.
    let factor = ((100.0 + total(stats, stat::REQUIREMENTS_PERCENT)) / 100.0).max(0.0);
    let meets = |req: u32, have: Option<i32>| {
        have.is_none_or(|have| (f64::from(req) * factor).floor() <= f64::from(have))
    };
    class_ok && meets(base.str, who.strength) && meets(base.dex, who.dexterity)
}

/// The scored parts of what equipment `item` would replace among the worn `(body slot, piece)`
/// items (the slot rule in the module docs); `None` for non-equipment, an item `who` can't use
/// (class, strength, dexterity; the level is the caller's check) and a shield beside a worn
/// two-handed weapon.
fn replaced(item: Piece<'_>, worn: &[(u8, Piece<'_>)], who: &Profile) -> Option<Vec<BTreeMap<Part, f64>>> {
    let base = bases_data::base(item.code?)?;
    if !usable(base, item.stats, who) {
        return None;
    }
    let in_slot = |slot: u8| worn.iter().find(|(s, _)| *s == slot).map(|(_, p)| parts(*p, who));
    let weapon_2h = |p: &Piece<'_>| {
        p.code
            .and_then(d2r_core::weapons_data::two_handed)
            .unwrap_or(false)
    };
    let worn_2h = worn.iter().any(|(s, p)| *s == 4 && weapon_2h(p));
    Some(match base.slot {
        // Rings: the worse of the two (an empty finger is the worst).
        6 => {
            let rings = [in_slot(6), in_slot(7)].map(Option::unwrap_or_default);
            let worse = rings
                .into_iter()
                .min_by(|a, b| score(a, who).total_cmp(&score(b, who)));
            worse.into_iter().collect()
        }
        4 if weapon_2h(&item) => [in_slot(4), in_slot(5)].into_iter().flatten().collect(),
        5 if worn_2h => return None,
        slot => in_slot(slot).into_iter().collect(),
    })
}

/// The score of `piece` for `who`: all of it, and the part outside defense and weapon damage
/// (what its affixes are worth to the build: skills, resistances, FCR, life …). Works for
/// charms and jewels too (no base needed).
#[must_use]
pub fn scores(piece: Piece<'_>, who: &Profile) -> (f64, f64) {
    let parts = parts(piece, who);
    let affixes = parts
        .iter()
        .filter(|(p, _)| !matches!(p, Part::Defense | Part::Damage))
        .map(|(p, v)| p.weight(who) * v)
        .sum();
    (score(&parts, who), affixes)
}

/// The score of what equipment `item` would replace among the `worn` items ([`verdict`]'s
/// slot rule; an empty slot scores 0); `None` when it isn't equipment `who` can use (class,
/// strength, dexterity) or can't be compared (a shield beside a two-handed weapon).
#[must_use]
pub fn replaced_score(item: Piece<'_>, worn: &[(u8, Piece<'_>)], who: &Profile) -> Option<f64> {
    Some(replaced(item, worn, who)?.iter().map(|p| score(p, who)).sum())
}

/// Whether the store item `item` (required level `req_level`) is better for `who` than what
/// it would replace among the worn `(body slot, piece)` items, by at least [`MIN_GAIN`]; the
/// `why` text ("+24 avg dmg, +2 Bow and Crossbow skills, +15 fire res": the three largest
/// weighted gains) when it is. Items the character can't use (class, level, strength,
/// dexterity) and non-equipment are never better. The slot rule is in the module docs.
#[must_use]
pub fn verdict(
    item: Piece<'_>,
    req_level: Option<u32>,
    worn: &[(u8, Piece<'_>)],
    who: &Profile,
) -> Option<String> {
    if req_level.zip(who.level).is_some_and(|(req, lvl)| req > lvl) {
        return None;
    }
    let old = replaced(item, worn, who)?;
    let mut delta = parts(item, who);
    for (part, v) in old.iter().flatten() {
        *delta.entry(*part).or_insert(0.0) -= v;
    }
    if score(&delta, who) < MIN_GAIN {
        return None;
    }
    let mut gains: Vec<(f64, Part, f64)> = delta
        .into_iter()
        .map(|(p, v)| (p.weight(who) * v, p, v))
        .filter(|(g, _, _)| *g > 0.0)
        .collect();
    gains.sort_by(|a, b| b.0.total_cmp(&a.0));
    Some(
        gains
            .iter()
            .take(3)
            .map(|&(_, p, v)| p.describe(v))
            .collect::<Vec<_>>()
            .join(", "),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stat entry: affix list unless `base`.
    const fn st(id: u16, value: i32) -> ItemStat {
        ItemStat {
            id,
            param: 0,
            value,
            base: false,
        }
    }

    const fn base(id: u16, value: i32) -> ItemStat {
        ItemStat {
            id,
            param: 0,
            value,
            base: true,
        }
    }

    const fn param(id: u16, param: u16, value: i32) -> ItemStat {
        ItemStat {
            id,
            param,
            value,
            base: false,
        }
    }

    /// Fulmina: a level-12 bow amazon planning Exploding Arrow (skill 16) and Fire Arrow (7).
    fn bowazon() -> Profile {
        Profile {
            class: 0,
            level: Some(12),
            strength: Some(30),
            dexterity: Some(40),
            skills: vec![7, 16],
            weapon: Some("bow"),
            caster: false,
        }
    }

    /// The live Short Bow: two-handed damage 1-4 in the base list.
    fn short_bow() -> Vec<ItemStat> {
        vec![base(stat::MIN_DAMAGE_2H, 1), base(stat::MAX_DAMAGE_2H, 4)]
    }

    fn piece<'a>(code: &'a str, stats: &'a [ItemStat]) -> Piece<'a> {
        Piece {
            code: Some(code),
            stats,
        }
    }

    #[test]
    fn weapon_damage_applies_ed_to_the_base_and_adds_flat_after() {
        // Short bow 1-4, +50% ED, +3 max: min 1.5, max 4 × 1.5 + 3 = 9 → avg 5.25.
        let mut stats = short_bow();
        stats.extend([
            st(stat::MIN_DAMAGE_PERCENT, 50),
            st(stat::MAX_DAMAGE_PERCENT, 50),
            st(stat::MAX_DAMAGE_2H, 3),
        ]);
        assert!((avg_damage(&stats, "bow", true, 0) - 5.25).abs() < 1e-9);
        // Flat damage written to the one-handed stat only still counts on a two-hander.
        let mut only_1h = short_bow();
        only_1h.extend([st(stat::MAX_DAMAGE, 3)]);
        assert!((avg_damage(&only_1h, "bow", true, 0) - 4.0).abs() < 1e-9);
        // A Bastard Sword carries both pairs: two-handed for an amazon, one-handed for a barbarian.
        let bastard = [
            base(stat::MIN_DAMAGE, 7),
            base(stat::MAX_DAMAGE, 19),
            base(stat::MIN_DAMAGE_2H, 20),
            base(stat::MAX_DAMAGE_2H, 28),
        ];
        assert!((avg_damage(&bastard, "swor", true, 0) - 24.0).abs() < 1e-9);
        assert!((avg_damage(&bastard, "swor", true, 4) - 13.0).abs() < 1e-9);
        // A javelin is thrown: 159/160, not the melee pair.
        let jav = [
            base(stat::MIN_DAMAGE, 1),
            base(stat::MAX_DAMAGE, 5),
            base(stat::THROW_MIN, 6),
            base(stat::THROW_MAX, 14),
        ];
        assert!((avg_damage(&jav, "jave", false, 0) - 10.0).abs() < 1e-9);
        assert!((avg_damage(&jav, "spea", false, 0) - 3.0).abs() < 1e-9);
    }

    /// The live Stoutnail's aggregated array (2026-11-19): vit 7, damage 14/24 (base 7/12
    /// with its 100% ED applied, ED itself not listed), MDR 2, durability, AR 4.
    const STOUTNAIL: [(u16, u16, u32); 7] = [
        (0, 3, 7),
        (0, 21, 14),
        (0, 22, 24),
        (0, 35, 2),
        (0, 72, 19),
        (0, 73, 19),
        (0, 78, 4),
    ];

    /// The druid of 2026-11-19: level 22, Summon/Elemental caster wielding a club.
    fn druid() -> Profile {
        Profile {
            class: 5,
            level: Some(22),
            weapon: Some("club"),
            caster: true,
            ..Profile::default()
        }
    }

    #[test]
    fn aggregated_totals_do_not_apply_ed_twice() {
        // Stoutnail: the chain gives base 7/12 + 100% ED, the array 14/24 already enhanced.
        let chain = [
            base(stat::MIN_DAMAGE, 7),
            base(stat::MAX_DAMAGE, 12),
            st(stat::MAX_DAMAGE_PERCENT, 100),
            st(stat::MIN_DAMAGE_PERCENT, 100),
        ];
        let base_ids = [21, 22, 72, 73];
        let totals = ItemStat::from_totals(&STOUTNAIL, &base_ids);
        assert!((avg_damage(&chain, "club", false, 5) - 19.0).abs() < 1e-9);
        assert!((avg_damage(&totals, "club", false, 5) - 19.0).abs() < 1e-9);
        // An ED entry in the array would be applied already: dropped, not counted again.
        let mut with_ed = STOUTNAIL.to_vec();
        with_ed.extend([
            (0, stat::MAX_DAMAGE_PERCENT, 100),
            (0, stat::MIN_DAMAGE_PERCENT, 100),
        ]);
        let totals = ItemStat::from_totals(&with_ed, &base_ids);
        assert!((avg_damage(&totals, "club", false, 5) - 19.0).abs() < 1e-9);
        // Live rare boots: base defense 6 + 23% ED → 7 in the array, which is the defense.
        let boots = ItemStat::from_totals(&[(0, 31, 7), (0, 16, 23)], &[31, 72, 73]);
        let who = bowazon();
        let defense = parts(piece("mbl", &boots), &who).get(&Part::Defense).copied();
        assert_eq!(defense, Some(7.0));
        // A staff (base 2H pair only) with +3 damage written to every pair: the 1H residue is
        // dropped, so a barbarian still swings it two-handed (avg 23, not 3).
        let staff = [(0, 21, 3), (0, 22, 3), (0, 23, 13), (0, 24, 33)];
        let staff = ItemStat::from_totals(&staff, &[23, 24]);
        assert!((avg_damage(&staff, "staf", true, 4) - 23.0).abs() < 1e-9);
    }

    #[test]
    fn a_worn_runeword_counts_with_its_runeword_and_rune_stats() {
        let who = druid();
        // Live Ancients' Pledge (Kite Shield): the chain saw base defense 17 and AR 8 only;
        // the aggregated array has the runeword's 50% ED applied (25) and its resists.
        let chain = [base(stat::ATTACK_RATING, 8), base(stat::DEFENSE, 17)];
        let full = [
            (0, 20, 8),
            (0, 31, 25),
            (0, 39, 48),
            (0, 41, 48),
            (0, 43, 43),
            (0, 45, 48),
            (0, 72, 30),
            (0, 73, 30),
            (0, 114, 10),
            (0, 194, 3),
        ];
        let pledge = ItemStat::from_totals(&full, &[20, 31, 72, 73, 194]);
        let points = |stats: &[ItemStat]| score(&parts(piece("kit", stats), &who), &who);
        assert!((points(&chain) - 1.7).abs() < 1e-9);
        assert!((points(&pledge) - 189.5).abs() < 1e-9);
        // A store shield with 25 fire and 25 lightning res: "better" than the chain's view,
        // not than the full runeword.
        let store = [
            base(stat::DEFENSE, 20),
            st(stat::RES_FIRE, 25),
            st(stat::RES_LIGHTNING, 25),
        ];
        let worn_chain = [(5, piece("kit", &chain))];
        assert!(verdict(piece("kit", &store), Some(0), &worn_chain, &who).is_some());
        let worn_full = [(5, piece("kit", &pledge))];
        assert_eq!(verdict(piece("kit", &store), Some(0), &worn_full, &who), None);
    }

    #[test]
    fn a_long_bow_beats_the_short_bow_and_says_why() {
        let short = short_bow();
        let worn = [(4, piece("sbw", &short))];
        // Long Bow 3-10 (avg 6.5 vs 2.5) with +15 fire res.
        let long = [
            base(stat::MIN_DAMAGE_2H, 3),
            base(stat::MAX_DAMAGE_2H, 10),
            st(stat::RES_FIRE, 15),
        ];
        assert_eq!(
            verdict(piece("lbw", &long), Some(0), &worn, &bowazon()).as_deref(),
            Some("+15 fire res, +4 avg dmg")
        );
        // A javelin's damage doesn't count for a bow user: no gain over the bow.
        let jav = [base(stat::THROW_MIN, 6), base(stat::THROW_MAX, 14)];
        assert_eq!(verdict(piece("jav", &jav), Some(0), &worn, &bowazon()), None);
    }

    #[test]
    fn planned_skill_tabs_weigh_most() {
        let worn = [];
        // +2 Bow and Crossbow skills (tab 0 of the amazon): Exploding Arrow is on it.
        let bow_tab = [param(stat::SKILL_TAB, 0, 2)];
        let why = verdict(piece("amu", &bow_tab), Some(0), &worn, &bowazon());
        assert_eq!(why.as_deref(), Some("+2 Bow and Crossbow skills"));
        // +1 Javelin and Spear skills: not planned, 3 points < the bar.
        let jav_tab = [param(stat::SKILL_TAB, 2, 1)];
        assert_eq!(verdict(piece("amu", &jav_tab), Some(0), &worn, &bowazon()), None);
        // A sorceress tab on an amazon counts nothing.
        let fire_tab = [param(stat::SKILL_TAB, 8, 3)];
        assert_eq!(verdict(piece("amu", &fire_tab), Some(0), &worn, &bowazon()), None);
    }

    #[test]
    fn small_gains_are_ignored() {
        let cap = [base(stat::DEFENSE, 3)];
        let worn = [(1, piece("cap", &cap))];
        let better_cap = [base(stat::DEFENSE, 6)];
        assert_eq!(
            verdict(piece("cap", &better_cap), Some(0), &worn, &bowazon()),
            None
        );
        // Into an empty slot (score 0): 20 × 1.5 = 30 def is 3 points.
        let helm = [base(stat::DEFENSE, 20), st(stat::DEFENSE_PERCENT, 50)];
        assert_eq!(verdict(piece("hlm", &helm), Some(0), &[], &bowazon()), None);
    }

    #[test]
    fn unusable_items_are_never_better() {
        let worn = [];
        let res = [st(stat::RES_FIRE, 30)];
        // Required level above clvl 12.
        assert_eq!(verdict(piece("rin", &res), Some(13), &worn, &bowazon()), None);
        assert!(verdict(piece("rin", &res), Some(12), &worn, &bowazon()).is_some());
        // A paladin shield on an amazon.
        assert_eq!(verdict(piece("pa1", &res), Some(0), &worn, &bowazon()), None);
        // Strength 30 < the Gothic Plate's 70; -60% requirements gets it to 28.
        assert_eq!(verdict(piece("gth", &res), Some(0), &worn, &bowazon()), None);
        let light = [st(stat::RES_FIRE, 30), st(stat::REQUIREMENTS_PERCENT, -60)];
        assert!(verdict(piece("gth", &light), Some(0), &worn, &bowazon()).is_some());
        // Potions and other non-equipment.
        assert_eq!(verdict(piece("hp1", &res), Some(0), &worn, &bowazon()), None);
    }

    #[test]
    fn rings_compare_against_the_worse_ring() {
        let good = [st(stat::RES_FIRE, 30)];
        let poor = [st(stat::RES_FIRE, 5)];
        let worn = [(6, piece("rin", &good)), (7, piece("rin", &poor))];
        let mid = [st(stat::RES_FIRE, 20)];
        assert_eq!(
            verdict(piece("rin", &mid), Some(0), &worn, &bowazon()).as_deref(),
            Some("+15 fire res")
        );
        let worn_one = [(6, piece("rin", &good))];
        assert_eq!(
            verdict(piece("rin", &poor), Some(0), &worn_one, &bowazon()),
            None,
            "5 res < the bar even against the empty finger"
        );
    }

    #[test]
    fn two_handers_replace_weapon_and_off_hand() {
        let sword = [base(stat::MIN_DAMAGE, 2), base(stat::MAX_DAMAGE, 7)];
        let shield = [base(stat::DEFENSE, 10), st(stat::RES_FIRE, 20)];
        let worn = [(4, piece("ssd", &sword)), (5, piece("buc", &shield))];
        let melee = Profile {
            weapon: Some("swor"),
            strength: Some(100),
            dexterity: Some(100),
            ..bowazon()
        };
        let two_hander = [base(stat::MIN_DAMAGE_2H, 8), base(stat::MAX_DAMAGE_2H, 17)];
        // Avg 12.5 vs 4.5 = +8 → 16 points, minus the shield's 20 res + 1 def → not better.
        assert_eq!(verdict(piece("2hs", &two_hander), Some(0), &worn, &melee), None);
        let worn_no_shield = [(4, piece("ssd", &sword))];
        assert_eq!(
            verdict(piece("2hs", &two_hander), Some(0), &worn_no_shield, &melee).as_deref(),
            Some("+8 avg dmg")
        );
        // A shield is not compared while a two-hander is worn.
        let bow = short_bow();
        let worn_bow = [(4, piece("sbw", &bow))];
        let big_shield = [base(stat::DEFENSE, 10), st(stat::RES_FIRE, 40)];
        assert_eq!(
            verdict(piece("buc", &big_shield), Some(0), &worn_bow, &bowazon()),
            None
        );
    }

    #[test]
    fn casters_ignore_weapon_damage() {
        let sorc = Profile {
            class: 1,
            level: Some(20),
            weapon: Some("staf"),
            caster: true,
            ..Profile::default()
        };
        let staff = [base(stat::MIN_DAMAGE_2H, 10), base(stat::MAX_DAMAGE_2H, 30)];
        assert_eq!(verdict(piece("bst", &staff), Some(0), &[], &sorc), None);
        assert!(is_caster(1, None));
        assert!(!is_caster(0, None));
        assert!(is_caster(5, Some("staf")));
        assert!(!is_caster(1, Some("bow")));
    }

    #[test]
    fn piece_names_give_the_weapon_kind() {
        assert_eq!(piece_kind("Javelin of Swiftness"), Some("jave"));
        assert_eq!(piece_kind("Gemmed Hunter's Bow (Chipped Rubies)"), Some("bow"));
        assert_eq!(piece_kind("Short Battle Bow"), Some("bow"));
        assert_eq!(piece_kind("Titan's Revenge"), None);
    }
}

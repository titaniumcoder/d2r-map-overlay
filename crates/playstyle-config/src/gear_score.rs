//! Stat-based item comparison: scores an item's stats for the character's guide stage and
//! compares a store item against what is worn in the same body slot.
//!
//! Stats in the BASE list ([`ItemStat::base`]) are base defense / damage, which ED% enhances;
//! aggregated totals ([`ItemStat::from_totals`]) arrive as base values with no ED% left.

use std::collections::BTreeMap;

use d2r_core::bases_data::{self, Base};
use d2r_core::item_stats::{ItemStat, stat};
use d2r_core::skills_data;
use d2r_core::stash::{Holdings, TabKind};

/// Score points per unit of a stat (per +1 skill, per point of average damage, per % …).
#[derive(Debug)]
pub struct Weights {
    pub all_skills: f64,
    /// +class skills of the own class.
    pub class_skills: f64,
    /// +skill tab holding a planned skill / any other own-class tab.
    pub tab_planned: f64,
    pub tab_other: f64,
    /// +single skill that is planned / any other.
    pub skill_planned: f64,
    pub skill_other: f64,
    /// Only for the weapon kind the character uses, and not for casters.
    pub avg_damage: f64,
    /// Each elemental resistance, per %.
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
    /// IAS, attack rating and life leech: for weapon users only.
    pub ias: f64,
    pub attack_rating: f64,
    pub life_leech: f64,
    pub mana_leech: f64,
    pub fhr: f64,
    pub frw: f64,
    pub defense: f64,
    pub magic_find: f64,
    pub replenish_life: f64,
    /// Teleport while [`Profile::teleport`]; else it counts as any other skill.
    pub teleport: f64,
}

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
    teleport: 40.0,
};

/// The smallest score gain marked "better": 8 points = +4 average damage, +8 resistance,
/// +80 defense, or any planned +skill.
pub const MIN_GAIN: f64 = 8.0;

/// What the character values, from its class, guide stage and gear.
#[derive(Debug, Clone, Default)]
pub struct Profile {
    /// Player txt row, 0 amazon .. 7 warlock.
    pub class: u32,
    pub level: Option<u32>,
    /// Base plus gear; `None` when unread (the requirement is then not checked).
    pub strength: Option<i32>,
    pub dexterity: Option<i32>,
    /// The stage's skill plan plus the allocated skills.
    pub skills: Vec<u16>,
    /// The weapon kind it fights with ([`Base::kind`]); `None`: every kind's damage counts.
    pub weapon: Option<&'static str>,
    pub caster: bool,
    /// [`teleport_wanted`].
    pub teleport: bool,
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

/// skills.txt row of Teleport.
pub const TELEPORT: u16 = 54;

/// Whether `stats` give `skill` to any class: as an oskill or as charges (param = skill << 6 |
/// level); a class-only [`stat::SINGLE_SKILL`] doesn't.
#[must_use]
pub fn grants_skill(stats: &[ItemStat], skill: u16) -> bool {
    stats.iter().any(|s| match s.id {
        stat::NONCLASS_SKILL => s.param == skill && s.value > 0,
        stat::CHARGED_SKILL => s.param >> 6 == skill,
        _ => false,
    })
}

/// Whether Teleport on gear is still wanted: no Enigma owned, nor on the own shared pages 1–2
/// (one on the trade pages, page 5 or the mercenary doesn't count).
#[must_use]
pub fn teleport_wanted(holdings: &Holdings<'_>) -> bool {
    let enigma = |n: &str| d2r_core::names::same_name(n, "Enigma");
    let on_own_pages = holdings
        .stash
        .iter()
        .filter(|tab| matches!(tab.kind, TabKind::Page(1 | 2)))
        .flat_map(|tab| &tab.items)
        .any(|i| i.runeword && enigma(&i.name));
    !on_own_pages && !holdings.owned.iter().any(|n| enigma(n))
}

const CASTER_KINDS: [&str; 3] = ["staf", "wand", "orb"];

/// A caster weapon kind, or no weapon at all on a sorceress or necromancer.
#[must_use]
pub fn is_caster(class: u32, kind: Option<&str>) -> bool {
    kind.map_or(matches!(class, 1 | 2), |k| CASTER_KINDS.contains(&k))
}

/// The weapon kind of the longest weapon base name inside a build piece ("Javelin of
/// Swiftness" → `jave`); `None` for uniques and runewords.
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
    /// Counted once however many stats grant it.
    Teleport,
}

impl Part {
    fn weight(self, who: &Profile) -> f64 {
        let w = &WEIGHTS;
        let fighter = if who.caster { 0.0 } else { 1.0 };
        match self {
            Self::AllSkills => w.all_skills,
            Self::ClassSkills => w.class_skills,
            Self::Tab(t) if who.plans_tab(t) => w.tab_planned,
            Self::Tab(_) => w.tab_other,
            Self::Skill(s) if who.skills.contains(&s) => w.skill_planned,
            Self::Teleport if who.teleport => w.teleport,
            Self::Skill(_) | Self::Teleport => w.skill_other,
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

    /// "+24 avg dmg", "+2 Bow and Crossbow skills".
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
            Self::Teleport if amount < 0.0 => "no Teleport".to_owned(),
            Self::Teleport => "Teleport".to_owned(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Piece<'a> {
    pub code: Option<&'a str>,
    pub stats: &'a [ItemStat],
}

/// Sum of the stat `id` entries of the base or the affix lists; in `i64` so misread values
/// can't overflow.
fn sum(stats: &[ItemStat], id: u16, base: bool) -> f64 {
    let total: i64 = stats
        .iter()
        .filter(|s| s.id == id && s.base == base)
        .map(|s| i64::from(s.value))
        .sum();
    let clamped = total.clamp(i64::from(i32::MIN), i64::from(i32::MAX));
    i32::try_from(clamped).map_or(0.0, f64::from)
}

fn total(stats: &[ItemStat], id: u16) -> f64 {
    sum(stats, id, true) + sum(stats, id, false)
}

/// The game's ED% rule.
fn enhanced(base: f64, percent: f64, flat: f64) -> f64 {
    base * (100.0 + percent) / 100.0 + flat
}

/// The base damage stat pair a weapon hits with: thrown for throwing weapons, else two-handed
/// for a two-hander (a barbarian swings one one-handed when it has the one-handed pair).
fn damage_pair(stats: &[ItemStat], kind: &str, two_handed: bool, class: u32) -> (u16, u16) {
    let has_1h = stats.iter().any(|s| s.base && s.id == stat::MIN_DAMAGE);
    if matches!(kind, "jave" | "tkni" | "taxe") {
        (stat::THROW_MIN, stat::THROW_MAX)
    } else if !has_1h || (two_handed && class != 4) {
        (stat::MIN_DAMAGE_2H, stat::MAX_DAMAGE_2H)
    } else {
        (stat::MIN_DAMAGE, stat::MAX_DAMAGE)
    }
}

/// Average weapon damage: ED% on the base pair, plus flat and elemental damage.
fn avg_damage(stats: &[ItemStat], kind: &str, two_handed: bool, class: u32) -> f64 {
    let (lo, hi) = damage_pair(stats, kind, two_handed, class);
    // An affix may write its flat damage to all three pairs or only to 21/22: the largest
    // avoids counting it twice.
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

/// The part one stat adds to for `who`; Teleport is left to [`grants_skill`].
fn part_of(s: &ItemStat, who: &Profile) -> Option<Part> {
    Some(match s.id {
        stat::ALL_SKILLS => Part::AllSkills,
        stat::CLASS_SKILLS if u32::from(s.param) == who.class => Part::ClassSkills,
        stat::SKILL_TAB if u32::from(s.param / 8) == who.class => Part::Tab(s.param),
        stat::NONCLASS_SKILL if s.param == TELEPORT => return None,
        stat::SINGLE_SKILL | stat::NONCLASS_SKILL => Part::Skill(s.param),
        stat::RES_FIRE => Part::ResFire,
        stat::RES_COLD => Part::ResCold,
        stat::RES_LIGHTNING => Part::ResLightning,
        stat::RES_POISON => Part::ResPoison,
        stat::MAX_LIFE => Part::Life,
        stat::MAX_MANA => Part::Mana,
        stat::STRENGTH => Part::Strength,
        stat::DEXTERITY => Part::Dexterity,
        stat::VITALITY => Part::Vitality,
        stat::ENERGY => Part::Energy,
        stat::FCR => Part::Fcr,
        stat::IAS => Part::Ias,
        stat::FHR => Part::Fhr,
        stat::FRW => Part::Frw,
        stat::MAGIC_FIND => Part::MagicFind,
        stat::ATTACK_RATING => Part::AttackRating,
        stat::LIFE_LEECH => Part::LifeLeech,
        stat::MANA_LEECH => Part::ManaLeech,
        stat::REPLENISH_LIFE => Part::ReplenishLife,
        _ => return None,
    })
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
        if let Some(part) = part_of(s, who) {
            add(part, f64::from(s.value));
        }
    }
    if grants_skill(stats, TELEPORT) {
        add(Part::Teleport, 1.0);
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

fn score(parts: &BTreeMap<Part, f64>, who: &Profile) -> f64 {
    parts.iter().map(|(p, v)| p.weight(who) * v).sum()
}

/// Whether `who` meets `base`'s class and strength / dexterity requirements (lowered by
/// `Requirements -%`); the level is the caller's check.
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
/// items: the worse ring, a two-hander's weapon and off-hand, else the item in its slot (an
/// empty slot scores 0). `None` for non-equipment, an item `who` can't use and a shield beside
/// a worn two-hander.
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

/// The score of `piece` for `who`: all of it, and its affixes' part (without defense and
/// weapon damage).
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

/// The score of what equipment `item` would replace among the `worn` items; `None` when it
/// can't be compared.
#[must_use]
pub fn replaced_score(item: Piece<'_>, worn: &[(u8, Piece<'_>)], who: &Profile) -> Option<f64> {
    Some(replaced(item, worn, who)?.iter().map(|p| score(p, who)).sum())
}

/// The `why` text ("+24 avg dmg, +2 Bow and Crossbow skills, +15 fire res") when the store
/// item `item` beats what it would replace among the worn `(body slot, piece)` items by at
/// least [`MIN_GAIN`]; never for an item `who` can't use.
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
    Some(largest_gains(delta, who))
}

/// The three largest weighted gains of `delta`, described.
fn largest_gains(delta: BTreeMap<Part, f64>, who: &Profile) -> String {
    let mut gains: Vec<(f64, Part, f64)> = delta
        .into_iter()
        .map(|(p, v)| (p.weight(who) * v, p, v))
        .filter(|(g, _, _)| *g > 0.0)
        .collect();
    gains.sort_by(|a, b| b.0.total_cmp(&a.0));
    gains
        .iter()
        .take(3)
        .map(|&(_, p, v)| p.describe(v))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An affix-list stat.
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

    /// Planning Exploding Arrow (skill 16) and Fire Arrow (7).
    fn bowazon() -> Profile {
        Profile {
            class: 0,
            level: Some(12),
            strength: Some(30),
            dexterity: Some(40),
            skills: vec![7, 16],
            weapon: Some("bow"),
            caster: false,
            teleport: false,
        }
    }

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
        let mut only_1h = short_bow();
        only_1h.extend([st(stat::MAX_DAMAGE, 3)]);
        assert!((avg_damage(&only_1h, "bow", true, 0) - 4.0).abs() < 1e-9);
        let bastard = [
            base(stat::MIN_DAMAGE, 7),
            base(stat::MAX_DAMAGE, 19),
            base(stat::MIN_DAMAGE_2H, 20),
            base(stat::MAX_DAMAGE_2H, 28),
        ];
        assert!(
            (avg_damage(&bastard, "swor", true, 0) - 24.0).abs() < 1e-9,
            "amazon: 2H"
        );
        assert!(
            (avg_damage(&bastard, "swor", true, 4) - 13.0).abs() < 1e-9,
            "barbarian: 1H"
        );
        let jav = [
            base(stat::MIN_DAMAGE, 1),
            base(stat::MAX_DAMAGE, 5),
            base(stat::THROW_MIN, 6),
            base(stat::THROW_MAX, 14),
        ];
        assert!((avg_damage(&jav, "jave", false, 0) - 10.0).abs() < 1e-9);
        assert!((avg_damage(&jav, "spea", false, 0) - 3.0).abs() < 1e-9);
    }

    /// A live Stoutnail's aggregated array: damage 14/24 is base 7/12 with its 100% ED applied.
    const STOUTNAIL: [(u16, u16, u32); 7] = [
        (0, 3, 7),
        (0, 21, 14),
        (0, 22, 24),
        (0, 35, 2),
        (0, 72, 19),
        (0, 73, 19),
        (0, 78, 4),
    ];

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
        let mut with_ed = STOUTNAIL.to_vec();
        with_ed.extend([
            (0, stat::MAX_DAMAGE_PERCENT, 100),
            (0, stat::MIN_DAMAGE_PERCENT, 100),
        ]);
        let totals = ItemStat::from_totals(&with_ed, &base_ids);
        assert!(
            (avg_damage(&totals, "club", false, 5) - 19.0).abs() < 1e-9,
            "ED not applied twice"
        );
        let boots = ItemStat::from_totals(&[(0, 31, 7), (0, 16, 23)], &[31, 72, 73]);
        let who = bowazon();
        let defense = parts(piece("mbl", &boots), &who).get(&Part::Defense).copied();
        assert_eq!(defense, Some(7.0));
        // +3 written to every pair: the 1H residue is dropped, so a barbarian swings it 2H.
        let staff = [(0, 21, 3), (0, 22, 3), (0, 23, 13), (0, 24, 33)];
        let staff = ItemStat::from_totals(&staff, &[23, 24]);
        assert!((avg_damage(&staff, "staf", true, 4) - 23.0).abs() < 1e-9);
    }

    #[test]
    fn a_worn_runeword_counts_with_its_runeword_and_rune_stats() {
        let who = druid();
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
        let long = [
            base(stat::MIN_DAMAGE_2H, 3),
            base(stat::MAX_DAMAGE_2H, 10),
            st(stat::RES_FIRE, 15),
        ];
        assert_eq!(
            verdict(piece("lbw", &long), Some(0), &worn, &bowazon()).as_deref(),
            Some("+15 fire res, +4 avg dmg")
        );
        let jav = [base(stat::THROW_MIN, 6), base(stat::THROW_MAX, 14)];
        assert_eq!(
            verdict(piece("jav", &jav), Some(0), &worn, &bowazon()),
            None,
            "not the bow user's weapon kind"
        );
    }

    #[test]
    fn planned_skill_tabs_weigh_most() {
        let worn = [];
        let bow_tab = [param(stat::SKILL_TAB, 0, 2)];
        let why = verdict(piece("amu", &bow_tab), Some(0), &worn, &bowazon());
        assert_eq!(why.as_deref(), Some("+2 Bow and Crossbow skills"));
        let jav_tab = [param(stat::SKILL_TAB, 2, 1)];
        assert_eq!(
            verdict(piece("amu", &jav_tab), Some(0), &worn, &bowazon()),
            None,
            "unplanned tab"
        );
        let fire_tab = [param(stat::SKILL_TAB, 8, 3)];
        assert_eq!(
            verdict(piece("amu", &fire_tab), Some(0), &worn, &bowazon()),
            None,
            "another class's tab"
        );
    }

    #[test]
    fn only_an_own_enigma_ends_the_teleport_want() {
        use d2r_core::stash::{StashItem, StashTab};
        fn wanted(stash: &[StashTab], owned: &[String], merc: &[String]) -> bool {
            teleport_wanted(&Holdings {
                stash,
                owned,
                runeword_copies: merc,
                merc_worn: merc,
            })
        }
        let enigma = StashItem {
            cell: None,
            name: "Enigma".to_owned(),
            code: Some("uui".to_owned()),
            quality: 2,
            identified: true,
            ethereal: false,
            runeword: true,
            quantity: None,
            sockets: 3,
            unique_index: None,
            stats: Vec::new(),
        };
        let on_page = |n: u8| {
            [StashTab {
                kind: TabKind::Page(n),
                items: vec![enigma.clone()],
            }]
        };
        let names = ["Enigma".to_owned()];
        assert!(wanted(&[], &[], &[]), "none held");
        assert!(!wanted(&[], &names, &[]), "worn or carried");
        assert!(!wanted(&on_page(1), &[], &[]), "own page 1");
        assert!(!wanted(&on_page(2), &[], &[]), "own page 2");
        assert!(wanted(&on_page(3), &[], &[]), "trade page 3");
        assert!(wanted(&on_page(4), &[], &[]), "trade page 4");
        assert!(wanted(&on_page(5), &[], &[]), "ignored page 5");
        assert!(wanted(&[], &[], &names), "the merc's");
    }

    #[test]
    fn teleport_weighs_until_enigma() {
        assert_eq!(skills_data::name(TELEPORT), Some("Teleport"));
        let charges = [param(stat::CHARGED_SKILL, (TELEPORT << 6) | 1, 10)];
        let oskill = [param(stat::NONCLASS_SKILL, TELEPORT, 1)];
        assert!(grants_skill(&charges, TELEPORT) && grants_skill(&oskill, TELEPORT));
        assert!(
            !grants_skill(&[param(stat::SINGLE_SKILL, TELEPORT, 1)], TELEPORT),
            "sorceress-only"
        );
        let mut who = druid();
        who.teleport = true;
        assert_eq!(
            verdict(piece("amu", &charges), None, &[], &who).as_deref(),
            Some("Teleport")
        );
        let (total, affixes) = scores(piece("amu", &oskill), &who);
        assert!((total - WEIGHTS.teleport).abs() < 1e-9 && (affixes - total).abs() < 1e-9);
        who.teleport = false;
        assert_eq!(
            verdict(piece("amu", &charges), None, &[], &who),
            None,
            "Enigma held"
        );
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
        let helm = [base(stat::DEFENSE, 20), st(stat::DEFENSE_PERCENT, 50)];
        assert_eq!(
            verdict(piece("hlm", &helm), Some(0), &[], &bowazon()),
            None,
            "30 def into an empty slot is 3 points"
        );
    }

    #[test]
    fn unusable_items_are_never_better() {
        let worn = [];
        let res = [st(stat::RES_FIRE, 30)];
        let verdict_of =
            |code, stats: &[ItemStat], level| verdict(piece(code, stats), Some(level), &worn, &bowazon());
        assert_eq!(verdict_of("rin", &res, 13), None, "level 13 > clvl 12");
        assert!(verdict_of("rin", &res, 12).is_some());
        assert_eq!(verdict_of("pa1", &res, 0), None, "paladin shield");
        assert_eq!(verdict_of("gth", &res, 0), None, "strength 30 < 70");
        let light = [st(stat::RES_FIRE, 30), st(stat::REQUIREMENTS_PERCENT, -60)];
        assert!(verdict_of("gth", &light, 0).is_some(), "-60% requirements: 28");
        assert_eq!(verdict_of("hp1", &res, 0), None, "not equipment");
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
        assert_eq!(
            verdict(piece("2hs", &two_hander), Some(0), &worn, &melee),
            None,
            "+8 avg dmg (16 points) minus the shield's 21"
        );
        let worn_no_shield = [(4, piece("ssd", &sword))];
        assert_eq!(
            verdict(piece("2hs", &two_hander), Some(0), &worn_no_shield, &melee).as_deref(),
            Some("+8 avg dmg")
        );
        let bow = short_bow();
        let worn_bow = [(4, piece("sbw", &bow))];
        let big_shield = [base(stat::DEFENSE, 10), st(stat::RES_FIRE, 40)];
        assert_eq!(
            verdict(piece("buc", &big_shield), Some(0), &worn_bow, &bowazon()),
            None,
            "a shield beside a two-hander"
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

//! Valuable ground drops: matches an item against the rules in the playstyle folder's
//! `valuables.json` (`playstyles/summoner-fury/`, written up in `research/valuables.md`) and
//! picks the tier to highlight it with.
//!
//! The rules are compiled into the binary (no per-user override, unlike the folder's TOML
//! files). To regenerate them from blizzhackers/d2data: `python3 research/gen_valuables.py
//! <d2data json dir> playstyles/summoner-fury/valuables.json`, then rebuild.

use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

pub use d2r_core::guidance::{Category, Tier};

use crate::config;
use d2r_core::game::ItemDrop;
use d2r_core::names::{self, identity, shown};
use d2r_core::offsets::item_data;

/// Quality a rule asks for, per the JSON's `conventions.quality`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Quality {
    /// White runeword base: normal or superior (not low quality).
    Normal,
    /// Superior only.
    Superior,
    Magic,
    Rare,
    Set,
    Unique,
    /// Quality is ignored.
    Any,
}

impl Quality {
    const fn matches(self, raw: u32) -> bool {
        match self {
            Self::Normal => raw == item_data::QUALITY_NORMAL || raw == item_data::QUALITY_SUPERIOR,
            Self::Superior => raw == item_data::QUALITY_SUPERIOR,
            Self::Magic => raw == item_data::QUALITY_MAGIC,
            Self::Rare => raw == item_data::QUALITY_RARE,
            Self::Set => raw == item_data::QUALITY_SET,
            Self::Unique => raw == item_data::QUALITY_UNIQUE,
            Self::Any => true,
        }
    }
}

/// How many copies of a matching item are worth keeping (`keep` in `valuables.json`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "lowercase")]
enum Copies {
    /// Every copy (the field is absent).
    #[default]
    All,
    /// One per unique/set name; further copies are give-aways (see [`keep_one_name`]).
    One,
}

/// One rule of `valuables.json`; fields the matcher doesn't use (`note`, `base_names`,
/// `runewords`) are ignored.
#[derive(Debug, Deserialize)]
struct Rule {
    tier: Tier,
    category: Category,
    item_codes: Vec<String>,
    quality: Quality,
    /// Inclusive `[min, max]`; `None` ignores sockets.
    sockets: Option<[u8; 2]>,
    /// `Some` = required value.
    ethereal: Option<bool>,
    name: String,
    /// Unique/set `index` names; empty = any item of the code and quality.
    #[serde(default)]
    item_names: Vec<String>,
    /// Base, quality, sockets and ethereal can't decide the value: a "check me".
    #[serde(default)]
    needs_stats: bool,
    /// Base item names, one per `item_codes` entry (same order).
    #[serde(default)]
    base_names: Vec<String>,
    #[serde(default)]
    keep: Copies,
    /// The map tag of the rule's drops (at most 4 characters, [`crate::labels`]), where the rule
    /// names one item or a natural group (`Shak`, `TalS`); absent: the built-in tags.
    #[serde(default)]
    label: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RuleFile {
    schema: u32,
    rules: Vec<Rule>,
}

/// The rules file schema this code implements.
const SCHEMA: u32 = 1;

static RULES: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    match serde_json::from_str::<RuleFile>(include_str!("../../../playstyles/summoner-fury/valuables.json")) {
        Ok(f) if f.schema == SCHEMA => f.rules,
        Ok(f) => {
            eprintln!(
                "valuables.json: schema {} (expected {SCHEMA}); no drop highlights",
                f.schema
            );
            Vec::new()
        }
        // The `rules_parse` test keeps this from happening in a release.
        Err(e) => {
            eprintln!("valuables.json: {e}; no drop highlights");
            Vec::new()
        }
    }
});

/// Why a drop is highlighted: the best matching rule among the enabled tiers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Highlight {
    pub tier: Tier,
    pub category: Category,
    /// The rule's `name`.
    pub rule: String,
    /// The match rests on what the overlay can't check: the unique/set name is unknown, or the
    /// rule needs the item's stats. Drawn as a dashed ring.
    pub uncertain: bool,
}

/// The in-game display name of an identified unique/set item (`None` otherwise: unidentified
/// items carry no name index yet, non-unique/set items none at all;
/// [`names::unique_set_name_parts`]).
#[must_use]
pub fn unique_set_name(drop: &ItemDrop) -> Option<&'static str> {
    names::unique_set_name_parts(drop.quality, drop.flags, drop.unique_index)
}

/// The rule's verdict on the drop: `None` if it doesn't match, else `Some(uncertain)`.
fn check(rule: &Rule, drop: &ItemDrop, name: Option<&str>) -> Option<bool> {
    let code = drop.code.as_deref()?;
    // A finished runeword is a white socketed item too, but no longer a base.
    let base_rule =
        rule.category == Category::Twink || matches!(rule.quality, Quality::Normal | Quality::Superior);
    if base_rule && drop.flags & item_data::FLAG_RUNEWORD != 0 {
        return None;
    }
    let fits = rule.item_codes.iter().any(|c| c == code)
        && rule.quality.matches(drop.quality)
        && rule
            .sockets
            .is_none_or(|[lo, hi]| (lo..=hi).contains(&drop.sockets))
        && rule.ethereal.is_none_or(|e| e == drop.ethereal);
    if !fits {
        return None;
    }
    let name_unknown = if rule.item_names.is_empty() {
        false
    } else {
        match name {
            Some(n) if rule.item_names.iter().any(|r| r == n) => false,
            Some(_) => return None,
            None => true,
        }
    };
    Some(name_unknown || rule.needs_stats)
}

/// Every tier of both categories (the spare lists' verdicts). Built once, not per item and
/// tick.
static EVERYTHING: LazyLock<config::Valuables> = LazyLock::new(config::Valuables::everything);

/// The best rule matching `drop` among the tiers `enabled` turns on: highest tier first, then a
/// certain match over an uncertain one, then the first rule in the file.
#[must_use]
pub fn best(drop: &ItemDrop, enabled: &config::Valuables) -> Option<Highlight> {
    best_of(
        &RULES,
        drop,
        identity(drop.quality, drop.flags, drop.unique_index),
        enabled,
    )
}

fn best_of(
    rules: &[Rule],
    drop: &ItemDrop,
    name: Option<&str>,
    enabled: &config::Valuables,
) -> Option<Highlight> {
    best_rule(rules, drop, name, enabled).map(|(r, uncertain)| Highlight {
        tier: r.tier,
        category: r.category,
        rule: r.name.clone(),
        uncertain,
    })
}

/// The rule [`best_of`] reports, with its `uncertain` flag.
// The lifetime ties the returned rule to `rules`, not to `drop` or `name`.
fn best_rule<'r>(
    rules: &'r [Rule],
    drop: &ItemDrop,
    name: Option<&str>,
    enabled: &config::Valuables,
) -> Option<(&'r Rule, bool)> {
    let mut found: Option<(&Rule, bool)> = None;
    for rule in rules.iter().filter(|r| enabled.enabled(r.category, r.tier)) {
        let Some(uncertain) = check(rule, drop, name) else {
            continue;
        };
        // Higher is better: (tier, certain).
        let better = found.is_none_or(|(b, b_unc)| (rule.tier, !uncertain) > (b.tier, !b_unc));
        if better {
            found = Some((rule, uncertain));
        }
    }
    found
}

/// The tiers the inventory keep list shows, whatever the config says for ground drops.
#[must_use]
pub fn keep_tiers() -> config::Valuables {
    config::Valuables {
        trade: vec![Tier::S, Tier::A],
        twink: vec![Tier::S, Tier::A],
    }
}

/// The `label` of the best rule of any tier matching `drop` as `name`, if that match is
/// certain: an unidentified ring must not be tagged `SoJ`.
#[must_use]
pub fn rule_label(drop: &ItemDrop, name: Option<&str>) -> Option<&'static str> {
    match best_rule(&RULES, drop, name, &EVERYTHING)? {
        (rule, false) => rule.label.as_deref(),
        (_, true) => None,
    }
}

/// What to call `item` in the lists: its unique/set name if known; else the base name
/// (from the rules, else the armor / weapon / misc tables, else the item code), with "eth" before and the socket count after. The
/// stash panel names its items the same way (see [`d2r_core::stash::stash_item`]).
#[must_use]
pub fn display_name(item: &ItemDrop) -> String {
    if let Some(name) = identity(item.quality, item.flags, item.unique_index) {
        return shown(name).to_owned();
    }
    let code = item.code.as_deref().unwrap_or("?");
    let base = RULES
        .iter()
        .find_map(|r| {
            let i = r.item_codes.iter().position(|c| c == code)?;
            r.base_names.get(i)
        })
        .map(String::as_str)
        .or_else(|| {
            d2r_core::item_levels_data::armor_name(code).or_else(|| d2r_core::weapons_data::name(code))
        })
        .or_else(|| d2r_core::affixes_data::misc(code).map(|(_, name)| name))
        .unwrap_or(code);
    let mut name = String::new();
    if item.ethereal {
        name.push_str("eth ");
    }
    name.push_str(base);
    if item.sockets > 0 {
        name = format!("{name} {}os", item.sockets);
    }
    name
}

/// What to do with a stored unique or set no S or A rule wants ([`verdict`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// No rule of any tier matches.
    Sell,
    /// The best match is a B rule.
    GiveAway,
}

/// Codes of quest and quest-like items (classic misc.txt/weapons.txt plus the D2R uber keys,
/// organs and tokens): never listed as spare, whatever their quality.
const QUEST_CODES: [&str; 36] = [
    "leg", "hdm", "bks", "bkd", "ass", "box", "tr1", "tr2", "msf", "vip", "hst", "j34", "g34", "g33", "bbb",
    "qf1", "qf2", "qey", "qhr", "qbr", "mss", "hfh", "ice", "xyz", "pk1", "pk2", "pk3", "dhn", "bey", "mbr",
    "std", "toa", "tes", "ceh", "bet", "fed",
];

/// uniqueitems rows of the act quest uniques: Amulet of the Viper, Staff of Kings, Horadric
/// Staff, Hell Forge Hammer, Khalim's Flail and Will.
const QUEST_UNIQUE_ROWS: std::ops::RangeInclusive<u32> = 123..=128;

/// Whether a stored item can go, conservatively: only an identified unique or set item with a
/// known name and code, not a charm, jewel, gem, rune or quest item. `None`: keep it (an S or A
/// rule matches, or it is none of these).
#[must_use]
pub fn verdict(item: &ItemDrop) -> Option<Verdict> {
    use d2r_core::items::DropKind;
    if !d2r_core::game::identified_unique_or_set(item.quality, item.flags)
        || !matches!(item.kind, DropKind::Unique | DropKind::Set)
    {
        return None;
    }
    // Without the code a quest item or jewel can't be ruled out.
    let code = item.code.as_deref()?;
    if code == "jew" || QUEST_CODES.contains(&code) {
        return None;
    }
    if item.quality == item_data::QUALITY_UNIQUE
        && item.unique_index.is_some_and(|i| QUEST_UNIQUE_ROWS.contains(&i))
    {
        return None;
    }
    identity(item.quality, item.flags, item.unique_index)?;
    match best(item, &EVERYTHING) {
        None => Some(Verdict::Sell),
        Some(h) if h.tier == Tier::B => Some(Verdict::GiveAway),
        Some(_) => None,
    }
}

/// The unique/set name of `item` if its best rule keeps only one copy per name (the other
/// copies are give-aways, [`crate::stash_plan`]).
#[must_use]
pub fn keep_one_name(item: &ItemDrop) -> Option<&'static str> {
    let name = identity(item.quality, item.flags, item.unique_index)?;
    let (rule, _) = best_rule(&RULES, item, Some(name), &EVERYTHING)?;
    (rule.keep == Copies::One).then_some(name)
}

#[cfg(test)]
mod tests {
    use super::*;
    use d2r_core::items::DropKind;

    fn drop(code: &str, quality: u32, sockets: u8, ethereal: bool) -> ItemDrop {
        ItemDrop {
            id: 1,
            code: Some(code.into()),
            kind: DropKind::Base,
            quality,
            flags: 0,
            ethereal,
            sockets,
            unique_index: None,
            stats: Vec::new(),
            item_data_head: String::new(),
            x: 0,
            y: 0,
        }
    }

    fn all() -> config::Valuables {
        config::Valuables::everything()
    }

    #[test]
    fn misc_names_are_sorted_by_code() {
        // `affixes_data::misc` (the misc base names of `display_name`) binary-searches it.
        assert!(d2r_core::affixes_data::MISC.is_sorted_by(|a, b| a.0 < b.0));
    }

    #[test]
    fn rules_parse() -> serde_json::Result<()> {
        let parsed: RuleFile =
            serde_json::from_str(include_str!("../../../playstyles/summoner-fury/valuables.json"))?;
        assert_eq!(parsed.schema, SCHEMA);
        assert_eq!(parsed.rules.len(), RULES.len());
        assert!(RULES.len() > 50, "{} rules", RULES.len());
        for r in parsed
            .rules
            .iter()
            .filter_map(|r| r.label.as_ref().map(|l| (&r.name, l)))
        {
            assert!(
                !r.1.is_empty() && r.1.chars().count() <= 4,
                "label {:?} of {}: 1 to 4 characters",
                r.1,
                r.0
            );
        }
        Ok(())
    }

    /// (item, expected (tier, category, uncertain), what).
    type Case = (ItemDrop, Option<(Tier, Category, bool)>, &'static str);

    #[test]
    fn matching_table() {
        // (item, expected (tier, category, uncertain), what)
        let cases: [Case; 12] = [
            (
                drop("crs", 2, 4, false),
                Some((Tier::A, Category::Twink, false)),
                "white 4os Crystal Sword",
            ),
            (
                drop("crs", 3, 4, false),
                Some((Tier::A, Category::Twink, false)),
                "superior counts as normal",
            ),
            (drop("crs", 1, 4, false), None, "low quality is not a base"),
            (drop("crs", 4, 4, false), None, "magic can't take a runeword"),
            // 3os Crystal Sword is only a B rule (Lawbringer), so with everything on it's B ...
            (
                drop("crs", 2, 3, false),
                Some((Tier::B, Category::Twink, false)),
                "3os Crystal Sword",
            ),
            (drop("crs", 2, 0, false), None, "unsocketed Crystal Sword"),
            (
                drop("7vo", 2, 0, true),
                None,
                "eth Colossus Voulge 0os: no Larzuk rule",
            ),
            (drop("7vo", 2, 0, false), None, "non-eth Colossus Voulge 0os"),
            (
                drop("7vo", 2, 4, true),
                Some((Tier::S, Category::Twink, false)),
                "eth 4os: S beats the A polearm rule",
            ),
            (
                drop("r26", 2, 0, false),
                Some((Tier::S, Category::Trade, false)),
                "Vex",
            ),
            (
                drop("cm3", 7, 0, false),
                Some((Tier::S, Category::Trade, true)),
                "unique grand charm, name unknown",
            ),
            (
                drop("cm3", 4, 0, false),
                Some((Tier::A, Category::Trade, true)),
                "magic grand charm needs stats",
            ),
        ];
        for (d, want, what) in cases {
            let got = best(&d, &all()).map(|h| (h.tier, h.category, h.uncertain));
            assert_eq!(got, want, "{what}");
        }
        let mut no_code = drop("r26", 2, 0, false);
        no_code.code = None;
        assert_eq!(best(&no_code, &all()), None, "no items table, no code: no rule");
    }

    #[test]
    fn three_socket_crystal_sword_is_not_highlighted_by_default() {
        assert_eq!(
            best(&drop("crs", 2, 3, false), &config::Valuables::default()),
            None
        );
    }

    #[test]
    fn name_rules_with_a_known_identity() {
        let shako = drop("uap", 7, 0, false);
        let hit = best_of(&RULES, &shako, Some("Harlequin Crest"), &all());
        assert_eq!(
            hit.map(|h| (h.tier, h.uncertain)),
            Some((Tier::S, false)),
            "named and known"
        );
        assert_eq!(
            best_of(&RULES, &shako, Some("Peasant Crown"), &all()),
            None,
            "another unique on the base"
        );
        assert_eq!(
            best_of(&RULES, &shako, None, &all()).map(|h| (h.tier, h.uncertain)),
            Some((Tier::S, true)),
            "name unknown"
        );
    }

    #[test]
    fn identity_needs_identified_and_a_known_row() {
        let mut shako = drop("uap", 7, 0, false);
        shako.unique_index = Some(248);
        assert_eq!(
            identity(shako.quality, shako.flags, shako.unique_index),
            None,
            "unidentified"
        );
        assert_eq!(best(&shako, &all()).map(|h| h.uncertain), Some(true));
        shako.flags = item_data::FLAG_IDENTIFIED;
        assert_eq!(
            identity(shako.quality, shako.flags, shako.unique_index),
            Some("Harlequin Crest")
        );
        assert_eq!(
            best(&shako, &all()).map(|h| (h.tier, h.uncertain)),
            Some((Tier::S, false)),
            "identified Shako: certain"
        );
        shako.unique_index = Some(127);
        assert_eq!(
            identity(shako.quality, shako.flags, shako.unique_index),
            Some("KhalimFlail"),
            "the live-verified row"
        );
        assert_eq!(
            best(&shako, &all()),
            None,
            "a known other unique: no name rule fires"
        );
        shako.unique_index = Some(99_999);
        assert_eq!(
            identity(shako.quality, shako.flags, shako.unique_index),
            None,
            "row not in the table"
        );
        let mut set = drop("xtb", 5, 0, false);
        set.flags = item_data::FLAG_IDENTIFIED;
        set.unique_index = Some(69);
        assert_eq!(
            identity(set.quality, set.flags, set.unique_index),
            Some("Aldur's Advance"),
            "set table for quality 5"
        );
        set.quality = 2;
        assert_eq!(
            identity(set.quality, set.flags, set.unique_index),
            None,
            "no table for normal items"
        );
    }

    #[test]
    fn finished_runewords_are_not_bases() {
        let mut spirit = drop("crs", 2, 4, false);
        assert!(best(&spirit, &all()).is_some(), "white 4os Crystal Sword");
        spirit.flags = item_data::FLAG_RUNEWORD;
        assert_eq!(best(&spirit, &all()), None, "a Spirit sword, not a base");
        let mut eth_voulge = drop("7vo", 2, 4, true);
        eth_voulge.flags = item_data::FLAG_RUNEWORD | item_data::FLAG_IDENTIFIED;
        assert_eq!(best(&eth_voulge, &all()), None, "an Infinity, not a base");
        // Trade rules on runes etc. don't care about the flag.
        let mut vex = drop("r26", 2, 0, false);
        vex.flags = item_data::FLAG_RUNEWORD;
        assert!(best(&vex, &all()).is_some());
    }

    #[test]
    fn tier_filter_by_config() {
        let vex = drop("r26", 2, 0, false);
        let lem = drop("r20", 2, 0, false);
        let only_s = config::Valuables {
            trade: vec![Tier::S],
            twink: vec![Tier::S, Tier::A],
        };
        assert!(best(&vex, &only_s).is_some());
        assert_eq!(best(&lem, &only_s), None, "A trade is off");
        let no_trade = config::Valuables {
            trade: vec![],
            twink: vec![Tier::S, Tier::A],
        };
        assert_eq!(best(&vex, &no_trade), None);
        assert!(
            best(&drop("crs", 2, 4, false), &no_trade).is_some(),
            "twink still on"
        );
        let twink_off = config::Valuables {
            trade: vec![Tier::S, Tier::A],
            twink: vec![],
        };
        // eth 4os Colossus Voulge: only twink rules match.
        assert_eq!(best(&drop("7vo", 2, 4, true), &twink_off), None);
    }

    /// An identified unique (`quality` 7) or set (5) item of `code` and row `index`.
    fn named(code: &str, quality: u32, index: u32) -> ItemDrop {
        let mut d = drop(code, quality, 0, false);
        d.kind = if quality == item_data::QUALITY_SET {
            DropKind::Set
        } else {
            DropKind::Unique
        };
        d.flags = item_data::FLAG_IDENTIFIED;
        d.unique_index = Some(index);
        d
    }

    #[test]
    fn verdict_sell_give_away_or_keep() {
        assert_eq!(
            verdict(&named("hax", 7, 12)),
            Some(Verdict::Sell),
            "Gravenspine: no rule"
        );
        assert_eq!(
            verdict(&named("tgl", 7, 105)),
            Some(Verdict::GiveAway),
            "Magefist: a B rule"
        );
        assert_eq!(verdict(&named("uap", 7, 248)), None, "Harlequin Crest: S");
        assert_eq!(
            verdict(&named("bsw", 5, 13)),
            Some(Verdict::Sell),
            "Isenhart's Lightbrand: no rule"
        );
    }

    #[test]
    fn verdict_excludes_unsafe_items() {
        let mut unid = named("hax", 7, 12);
        unid.flags = 0;
        assert_eq!(verdict(&unid), None, "unidentified");
        let mut rw = named("hax", 7, 12);
        rw.flags |= item_data::FLAG_RUNEWORD;
        assert_eq!(verdict(&rw), None, "runeword flag");
        assert_eq!(verdict(&named("uap", 7, 127)), None, "Khalim's Flail by row");
        for code in ["hdm", "msf", "vip", "g33", "qf1", "qf2", "qey", "hfh", "hst"] {
            assert_eq!(verdict(&named(code, 7, 12)), None, "quest code {code}");
        }
        let mut charm = named("cm1", 7, 381);
        charm.kind = DropKind::Charm {
            size: d2r_core::items::CharmSize::Small,
            unique: true,
        };
        assert_eq!(verdict(&charm), None, "unique charm");
        assert_eq!(verdict(&named("jew", 7, 392)), None, "Rainbow Facet jewel");
        let mut magic = named("hax", 4, 12);
        magic.kind = DropKind::Base;
        assert_eq!(verdict(&magic), None, "magic");
        assert_eq!(verdict(&named("hax", 2, 12)), None, "normal");
        let mut no_code = named("hax", 7, 12);
        no_code.code = None;
        assert_eq!(verdict(&no_code), None, "no code: quest item not ruled out");
        assert_eq!(verdict(&named("hax", 7, 99_999)), None, "unknown name");
    }

    /// Sigon's Visor (Great Helm, setitems row 36), identified.
    fn visor() -> ItemDrop {
        named("ghm", 5, 36)
    }

    #[test]
    fn sigon_rule_matches_identified_pieces_and_keeps_one() {
        let rule = RULES.iter().find(|r| r.name == "Sigon's Complete Steel");
        assert_eq!(
            rule.map(|r| (r.tier, r.category, r.keep)),
            Some((Tier::A, Category::Twink, Copies::One))
        );
        // setitems rows 35..=40: Gage, Visor, Shelter, Sabot, Wrap, Guard.
        for (code, row) in [
            ("hgl", 35),
            ("ghm", 36),
            ("gth", 37),
            ("hbt", 38),
            ("hbl", 39),
            ("tow", 40),
        ] {
            let piece = named(code, 5, row);
            assert_eq!(
                best(&piece, &all()).map(|h| (h.tier, h.category, h.uncertain, h.rule)),
                Some((
                    Tier::A,
                    Category::Twink,
                    false,
                    "Sigon's Complete Steel".to_owned()
                )),
                "{code}"
            );
            assert_eq!(verdict(&piece), None, "{code}: an A rule, not spare by itself");
        }
        let mut unid = visor();
        unid.flags = 0;
        assert_eq!(keep_one_name(&unid), None, "unidentified: name unknown");
        assert_eq!(
            keep_one_name(&named("uap", 7, 248)),
            None,
            "Shako: every copy counts"
        );
    }

    /// The map's marks without any build wants: trade marks only.
    fn apply(drops: &mut Vec<ItemDrop>, enabled: &config::Valuables) -> d2r_core::guidance::Marks {
        crate::marks::apply(drops, enabled, &crate::stash_plan::Wants::default())
    }

    #[test]
    fn apply_drops_unmatched_bases_only() {
        let mut gem = drop("gcg", 2, 0, false);
        gem.kind = DropKind::Gem(d2r_core::items::GemColour::Emerald);
        gem.id = 3;
        let mut three = drop("crs", 2, 3, false);
        three.id = 2;
        let mut monarch = drop("uit", 2, 4, false);
        monarch.id = 4;
        let mut drops = vec![drop("crs", 2, 4, false), three, gem, monarch];
        let marks = apply(&mut drops, &config::Valuables::default());
        let left: Vec<_> = drops
            .iter()
            .map(|d| {
                (
                    d.code.as_deref(),
                    d.sockets,
                    marks.get(&d.id).and_then(|m| m.trade),
                )
            })
            .collect();
        assert_eq!(
            left,
            vec![
                (Some("gcg"), 0, None),
                (Some("uit"), 4, Some((Tier::A, Category::Trade)))
            ],
            "the 3os base is gone (B is off), the 4os Crystal Sword too (a twink A base is no \
             trade good and no need wants it), the gem stays without a ring"
        );
    }

    #[test]
    fn keeping_every_coded_base_in_the_reader_changes_no_shown_drop() {
        // Step 3 of the plugin refactor (PLAN.md): the reader used to drop the ground bases no
        // rule of any tier matches (the old `is_candidate`); it now keeps every base with a
        // code and `apply` is the only filter. Over every rule code (plus an unlisted code and
        // none) with the qualities, sockets, eth and runeword flag around the rules, both
        // pipelines leave the same drops with the same marks, for every tier selection tried.
        let mut codes: Vec<Option<&str>> = RULES
            .iter()
            .flat_map(|r| r.item_codes.iter().map(|c| Some(c.as_str())))
            .chain([Some("hax"), None])
            .collect();
        codes.sort_unstable();
        codes.dedup();
        let mut drops = Vec::new();
        for &code in &codes {
            for quality in 1..=8 {
                for sockets in [0, 2, 3, 4, 5, 6] {
                    for ethereal in [false, true] {
                        for flags in [item_data::FLAG_IDENTIFIED, item_data::FLAG_RUNEWORD] {
                            let mut d = drop("", quality, sockets, ethereal);
                            d.code = code.map(str::to_owned);
                            d.flags = flags;
                            d.id = u32::try_from(drops.len()).unwrap_or(u32::MAX);
                            drops.push(d);
                        }
                    }
                }
            }
        }
        let was_candidate = |d: &&ItemDrop| d.kind != DropKind::Base || best(d, &EVERYTHING).is_some();
        let has_code = |d: &&ItemDrop| d.kind != DropKind::Base || d.code.is_some();
        let tiers = [
            vec![],
            vec![Tier::S],
            vec![Tier::S, Tier::A],
            vec![Tier::B],
            vec![Tier::S, Tier::A, Tier::B],
        ];
        let mut shown = 0;
        for trade in &tiers {
            for twink in &tiers {
                let enabled = config::Valuables {
                    trade: trade.clone(),
                    twink: twink.clone(),
                };
                let mut old: Vec<ItemDrop> = drops.iter().filter(was_candidate).cloned().collect();
                let old_marks = apply(&mut old, &enabled);
                let mut new: Vec<ItemDrop> = drops.iter().filter(has_code).cloned().collect();
                let new_marks = apply(&mut new, &enabled);
                assert_eq!(new, old, "trade {trade:?}, twink {twink:?}");
                assert_eq!(new_marks, old_marks, "trade {trade:?}, twink {twink:?}");
                shown += new.len();
            }
        }
        assert!(shown > 1000, "the sample reaches the rules: {shown} drops shown");
    }
}

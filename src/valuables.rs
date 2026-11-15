//! Valuable ground drops: matches an item against the rules in `research/valuables.json`
//! (written up in `research/valuables.md`) and picks the tier to highlight it with.
//!
//! The rules are compiled into the binary, so it needs no data files at run time. To regenerate
//! them from blizzhackers/d2data: `python3 research/gen_valuables.py <d2data json dir>
//! research/valuables.json`, then rebuild.

use std::sync::LazyLock;

use serde::{Deserialize, Serialize};

use crate::game::ItemDrop;
use crate::offsets::item_data;
use crate::{config, item_names_data};

/// How much a drop is worth. Declared lowest first, so the derived order is "worse < better"
/// (and the best ring is drawn last, on top).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Tier {
    B,
    A,
    S,
}

/// What a drop is valuable for: selling (`trade`) or a runeword base for a twink (`twink`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    Trade,
    Twink,
}

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
    /// One per unique/set name; further copies are give-aways (see [`surplus`]).
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
}

#[derive(Debug, Deserialize)]
struct RuleFile {
    schema: u32,
    rules: Vec<Rule>,
}

/// The rules file schema this code implements.
const SCHEMA: u32 = 1;

static RULES: LazyLock<Vec<Rule>> = LazyLock::new(|| {
    match serde_json::from_str::<RuleFile>(include_str!("../research/valuables.json")) {
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

/// The unique or set name (`index` string) of the drop, if known: only for an identified item
/// whose file index is in the table of its quality. The index is verified for identified
/// uniques only (see `item_data::FILE_INDEX`); unidentified ones stay unknown, and their name
/// rules match as uncertain.
fn identity(drop: &ItemDrop) -> Option<&'static str> {
    if drop.flags & item_data::FLAG_IDENTIFIED == 0 {
        return None;
    }
    let table = match drop.quality {
        item_data::QUALITY_UNIQUE => item_names_data::UNIQUE_NAMES,
        item_data::QUALITY_SET => item_names_data::SET_NAMES,
        _ => return None,
    };
    let index = drop.unique_index?;
    let row = table.binary_search_by_key(&index, |&(i, _)| i).ok()?;
    table.get(row).map(|&(_, name)| name)
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

/// Every tier of both categories: what the reader keeps as ground candidates. Built once, not
/// per item and tick.
static EVERYTHING: LazyLock<config::Valuables> = LazyLock::new(config::Valuables::everything);

/// True if a rule of any tier matches `drop` (the reader keeps it; [`apply`] decides later).
pub fn is_candidate(drop: &ItemDrop) -> bool {
    best(drop, &EVERYTHING).is_some()
}

/// The best rule matching `drop` among the tiers `enabled` turns on: highest tier first, then a
/// certain match over an uncertain one, then the first rule in the file.
pub fn best(drop: &ItemDrop, enabled: &config::Valuables) -> Option<Highlight> {
    best_of(&RULES, drop, identity(drop), enabled)
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
pub fn keep_tiers() -> config::Valuables {
    config::Valuables {
        trade: vec![Tier::S, Tier::A],
        twink: vec![Tier::S, Tier::A],
    }
}

/// An inventory item worth keeping (see [`keep_list`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Keep {
    /// Display name: unique/set name when known, else the base name with sockets and "eth";
    /// ends in `?` for an uncertain match.
    pub name: String,
    pub tier: Tier,
    pub category: Category,
    /// Identical entries are listed once with their count.
    pub count: u32,
}

/// The keep list of the player's `items` in the inventory: every certain S or A match
/// ([`keep_tiers`]) and uncertain S matches (an uncertain A, e.g. a magic grand charm, is left
/// out), identical ones merged, best tier first, then by name. A surplus copy of a keep-one item
/// ([`surplus`], decided over all places) is left out: it is a give-away.
pub fn keep_list(items: &[(ItemDrop, Place)]) -> Vec<Keep> {
    let tiers = keep_tiers();
    let mut out: Vec<Keep> = Vec::new();
    for ((item, place), spare) in items.iter().zip(surplus(items)) {
        if *place != Place::Inventory || spare {
            continue;
        }
        let Some(h) = best(item, &tiers) else { continue };
        if h.uncertain && h.tier != Tier::S {
            continue;
        }
        let mut name = display_name(item);
        if h.uncertain {
            name.push('?');
        }
        match out
            .iter_mut()
            .find(|k| k.name == name && k.tier == h.tier && k.category == h.category)
        {
            Some(k) => k.count = k.count.saturating_add(1),
            None => out.push(Keep {
                name,
                tier: h.tier,
                category: h.category,
                count: 1,
            }),
        }
    }
    out.sort_by(|a, b| b.tier.cmp(&a.tier).then_with(|| a.name.cmp(&b.name)));
    out
}

/// What to call `item` in the keep list: its unique/set name if known; else the base name
/// (from the rules, else the item code), with "eth" before and the socket count after.
fn display_name(item: &ItemDrop) -> String {
    if let Some(name) = identity(item) {
        return name.to_owned();
    }
    let code = item.code.as_deref().unwrap_or("?");
    let base = RULES
        .iter()
        .find_map(|r| {
            let i = r.item_codes.iter().position(|c| c == code)?;
            r.base_names.get(i)
        })
        .map_or(code, String::as_str);
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

/// Where a stored item of the player is (see `game::snapshot` for how page and owner map to it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Place {
    Inventory,
    Cube,
    Stash,
    Shared,
}

impl Place {
    /// The short label shown after the item name: `inv`, `cube`, `stash`, `shared`.
    pub const fn label(self) -> &'static str {
        match self {
            Self::Inventory => "inv",
            Self::Cube => "cube",
            Self::Stash => "stash",
            Self::Shared => "shared",
        }
    }
}

/// A stored item that can go: sold to a vendor or given away (see [`spare_lists`]).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Spare {
    /// The unique or set name.
    pub name: String,
    pub place: Place,
    /// Identical entries (same name and place) are listed once with their count.
    pub count: u32,
}

/// What to do with a stored item no S or A rule wants (or a [`surplus`] copy).
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

/// Cheap pre-check on the raw `ItemData` fields: an identified unique or set item, not a
/// runeword. Only these can be spare, so the reader skips the rest of the stash.
pub const fn may_spare(quality: u32, flags: u32) -> bool {
    (quality == item_data::QUALITY_UNIQUE || quality == item_data::QUALITY_SET)
        && flags & item_data::FLAG_IDENTIFIED != 0
        && flags & item_data::FLAG_RUNEWORD == 0
}

/// Whether a stored item can go, conservatively: only an identified unique or set item with a
/// known name and code, not a charm, jewel, gem, rune or quest item. `None`: keep it (an S or A
/// rule matches, or it is none of these).
pub fn verdict(item: &ItemDrop) -> Option<Verdict> {
    use crate::items::DropKind;
    if !may_spare(item.quality, item.flags) || !matches!(item.kind, DropKind::Unique | DropKind::Set) {
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
    identity(item)?;
    match best(item, &EVERYTHING) {
        None => Some(Verdict::Sell),
        Some(h) if h.tier == Tier::B => Some(Verdict::GiveAway),
        Some(_) => None,
    }
}

/// The unique/set name of `item` if its best rule keeps only one copy per name.
fn keep_one_name(item: &ItemDrop) -> Option<&'static str> {
    let name = identity(item)?;
    let (rule, _) = best_rule(&RULES, item, Some(name), &EVERYTHING)?;
    (rule.keep == Copies::One).then_some(name)
}

/// For each of the player's `items`: true if it is a surplus copy of a keep-one item. Of the
/// copies with one name the first in a stash (personal or shared) is kept, else the first in the
/// cube, else the first in the inventory; every other copy is surplus.
fn surplus(items: &[(ItemDrop, Place)]) -> Vec<bool> {
    let rank = |p: Place| match p {
        Place::Stash | Place::Shared => 0,
        Place::Cube => 1,
        Place::Inventory => 2,
    };
    let names: Vec<_> = items.iter().map(|(d, _)| keep_one_name(d)).collect();
    names
        .iter()
        .enumerate()
        .map(|(i, name)| {
            if name.is_none() {
                return false;
            }
            let kept = names
                .iter()
                .zip(items)
                .enumerate()
                .filter(|(_, (n, _))| *n == name)
                .min_by_key(|&(j, (_, (_, p)))| (rank(*p), j))
                .map(|(j, _)| j);
            kept != Some(i)
        })
        .collect()
}

/// The sell and the give-away lists of the player's `items` (see [`verdict`]; [`surplus`]
/// copies are give-aways), identical ones merged, each sorted by name, then place.
pub fn spare_lists(items: &[(ItemDrop, Place)]) -> (Vec<Spare>, Vec<Spare>) {
    let (mut sell, mut give): (Vec<Spare>, Vec<Spare>) = (Vec::new(), Vec::new());
    for ((item, place), spare) in items.iter().zip(surplus(items)) {
        let v = if spare {
            Some(Verdict::GiveAway)
        } else {
            verdict(item)
        };
        let (Some(v), Some(name)) = (v, identity(item)) else {
            continue;
        };
        let list = match v {
            Verdict::Sell => &mut sell,
            Verdict::GiveAway => &mut give,
        };
        match list.iter_mut().find(|s| s.name == name && s.place == *place) {
            Some(s) => s.count = s.count.saturating_add(1),
            None => list.push(Spare {
                name: name.to_owned(),
                place: *place,
                count: 1,
            }),
        }
    }
    for list in [&mut sell, &mut give] {
        list.sort_by(|a, b| a.name.cmp(&b.name).then(a.place.cmp(&b.place)));
    }
    (sell, give)
}

/// Sets each drop's highlight from the tiers `enabled` turns on, and removes the drops that are
/// only shown for a rule ([`crate::items::DropKind::Base`]) when no enabled rule matches.
pub fn apply(drops: &mut Vec<ItemDrop>, enabled: &config::Valuables) {
    for d in drops.iter_mut() {
        d.highlight = best(d, enabled);
    }
    drops.retain(|d| d.kind != crate::items::DropKind::Base || d.highlight.is_some());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::items::DropKind;

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
            item_data_head: String::new(),
            highlight: None,
            x: 0,
            y: 0,
        }
    }

    fn all() -> config::Valuables {
        config::Valuables::everything()
    }

    #[test]
    fn rules_parse() -> serde_json::Result<()> {
        let parsed: RuleFile = serde_json::from_str(include_str!("../research/valuables.json"))?;
        assert_eq!(parsed.schema, SCHEMA);
        assert_eq!(parsed.rules.len(), RULES.len());
        assert!(RULES.len() > 50, "{} rules", RULES.len());
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
                Some((Tier::A, Category::Twink, false)),
                "eth Colossus Voulge 0os",
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
        assert_eq!(identity(&shako), None, "unidentified");
        assert_eq!(best(&shako, &all()).map(|h| h.uncertain), Some(true));
        shako.flags = item_data::FLAG_IDENTIFIED;
        assert_eq!(identity(&shako), Some("Harlequin Crest"));
        assert_eq!(
            best(&shako, &all()).map(|h| (h.tier, h.uncertain)),
            Some((Tier::S, false)),
            "identified Shako: certain"
        );
        shako.unique_index = Some(127);
        assert_eq!(identity(&shako), Some("KhalimFlail"), "the live-verified row");
        assert_eq!(
            best(&shako, &all()),
            None,
            "a known other unique: no name rule fires"
        );
        shako.unique_index = Some(99_999);
        assert_eq!(identity(&shako), None, "row not in the table");
        let mut set = drop("xtb", 5, 0, false);
        set.flags = item_data::FLAG_IDENTIFIED;
        set.unique_index = Some(69);
        assert_eq!(identity(&set), Some("Aldur's Advance"), "set table for quality 5");
        set.quality = 2;
        assert_eq!(identity(&set), None, "no table for normal items");
    }

    #[test]
    fn finished_runewords_are_not_bases() {
        let mut spirit = drop("crs", 2, 4, false);
        assert!(is_candidate(&spirit), "white 4os Crystal Sword");
        spirit.flags = item_data::FLAG_RUNEWORD;
        assert_eq!(best(&spirit, &all()), None, "a Spirit sword, not a base");
        assert!(!is_candidate(&spirit));
        let mut eth_voulge = drop("7vo", 2, 4, true);
        eth_voulge.flags = item_data::FLAG_RUNEWORD | item_data::FLAG_IDENTIFIED;
        assert_eq!(best(&eth_voulge, &all()), None, "an Infinity, not a base");
        // Trade rules on runes etc. don't care about the flag.
        let mut vex = drop("r26", 2, 0, false);
        vex.flags = item_data::FLAG_RUNEWORD;
        assert!(best(&vex, &all()).is_some());
    }

    #[test]
    fn name_tables_are_sorted_by_row() {
        for table in [item_names_data::UNIQUE_NAMES, item_names_data::SET_NAMES] {
            assert!(
                table.windows(2).all(|w| matches!(w, [a, b] if a.0 < b.0)),
                "binary_search needs ascending, unique rows"
            );
            assert_ne!(table.len(), 0);
        }
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

    #[test]
    fn keep_list_is_s_and_a_merged_and_sorted() {
        let identified = |mut d: ItemDrop, index: Option<u32>| {
            d.flags = item_data::FLAG_IDENTIFIED;
            d.unique_index = index;
            d
        };
        let items = vec![
            identified(drop("r20", 2, 0, false), None),      // Lem: A
            identified(drop("uap", 7, 0, false), Some(248)), // Harlequin Crest: S
            identified(drop("r20", 2, 0, false), None),      // a second Lem
            identified(drop("r01", 2, 0, false), None),      // El: B, left out
            identified(drop("crs", 2, 3, false), None),      // 3os sword: B, left out
            identified(drop("7vo", 2, 0, true), None),       // eth 0os Colossus Voulge: A
            identified(drop("cm3", 4, 0, false), None),      // magic grand charm: uncertain A, left out
            drop("cm3", 7, 0, false),                        // unidentified unique grand charm: uncertain S
            identified(drop("hax", 2, 0, false), None),      // no rule
        ];
        let items: Vec<_> = items.into_iter().map(|d| (d, Place::Inventory)).collect();
        let keep: Vec<_> = keep_list(&items)
            .into_iter()
            .map(|k| (k.name, k.tier, k.category, k.count))
            .collect();
        let row = |n: &str, t, c, count| (n.to_owned(), t, c, count);
        assert_eq!(
            keep,
            vec![
                row("Grand Charm?", Tier::S, Category::Trade, 1),
                row("Harlequin Crest", Tier::S, Category::Trade, 1),
                row("Lem Rune", Tier::A, Category::Trade, 2),
                row("eth Colossus Voulge", Tier::A, Category::Twink, 1),
            ]
        );
        assert_eq!(keep_list(&[]), vec![]);
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
            size: crate::items::CharmSize::Small,
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

    #[test]
    fn spare_lists_merge_and_sort() {
        let items = vec![
            (named("tgl", 7, 105), Place::Stash),
            (named("hax", 7, 12), Place::Shared),
            (named("hax", 7, 12), Place::Inventory),
            (named("hax", 7, 12), Place::Inventory),
            (named("uap", 7, 248), Place::Inventory),
            (named("bsw", 5, 13), Place::Cube),
        ];
        let (sell, give) = spare_lists(&items);
        let spare = |n: &str, place, count| Spare {
            name: n.into(),
            place,
            count,
        };
        assert_eq!(
            sell,
            vec![
                spare("Gravenspine", Place::Inventory, 2),
                spare("Gravenspine", Place::Shared, 1),
                spare("Isenhart's Lightbrand", Place::Cube, 1),
            ]
        );
        assert_eq!(give, vec![spare("Magefist", Place::Stash, 1)]);
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

    #[test]
    fn keep_one_in_the_inventory_only_is_kept() {
        let items = vec![
            (visor(), Place::Inventory),
            (named("hgl", 5, 35), Place::Inventory),
        ];
        let keep: Vec<_> = keep_list(&items).into_iter().map(|k| (k.name, k.count)).collect();
        assert_eq!(
            keep,
            vec![("Sigon's Gage".to_owned(), 1), ("Sigon's Visor".to_owned(), 1)]
        );
        assert_eq!(spare_lists(&items), (vec![], vec![]));
    }

    #[test]
    fn keep_one_prefers_the_stashed_copy() {
        let give = |place, count| Spare {
            name: "Sigon's Visor".into(),
            place,
            count,
        };
        // A stashed copy: the inventory one is a give-away, the stashed one isn't listed.
        let items = vec![(visor(), Place::Inventory), (visor(), Place::Shared)];
        assert_eq!(keep_list(&items), vec![]);
        assert_eq!(spare_lists(&items), (vec![], vec![give(Place::Inventory, 1)]));
        // Cube beats the inventory; two in the inventory with one in the cube: both inventory
        // ones go.
        let items = vec![
            (visor(), Place::Inventory),
            (visor(), Place::Cube),
            (visor(), Place::Inventory),
        ];
        assert_eq!(keep_list(&items), vec![]);
        assert_eq!(spare_lists(&items), (vec![], vec![give(Place::Inventory, 2)]));
        // Two in the stash: one goes; two inventory copies: one stays on the keep line.
        let items = vec![(visor(), Place::Stash), (visor(), Place::Stash)];
        assert_eq!(spare_lists(&items), (vec![], vec![give(Place::Stash, 1)]));
        let items = vec![(visor(), Place::Inventory), (visor(), Place::Inventory)];
        assert_eq!(
            keep_list(&items).iter().map(|k| k.count).collect::<Vec<_>>(),
            vec![1]
        );
        assert_eq!(spare_lists(&items), (vec![], vec![give(Place::Inventory, 1)]));
    }

    #[test]
    fn keep_one_leaves_other_rules_alone() {
        // Two Shakos (S, every copy counts) and two Magefists (B): unchanged by keep-one.
        let items = vec![
            (named("uap", 7, 248), Place::Inventory),
            (named("uap", 7, 248), Place::Stash),
            (named("uap", 7, 248), Place::Inventory),
            (named("tgl", 7, 105), Place::Stash),
            (named("tgl", 7, 105), Place::Stash),
            (visor(), Place::Stash),
        ];
        let keep: Vec<_> = keep_list(&items).into_iter().map(|k| (k.name, k.count)).collect();
        assert_eq!(keep, vec![("Harlequin Crest".to_owned(), 2)]);
        let (sell, give) = spare_lists(&items);
        assert_eq!(sell, vec![]);
        assert_eq!(
            give,
            vec![Spare {
                name: "Magefist".into(),
                place: Place::Stash,
                count: 2
            }]
        );
    }

    #[test]
    fn apply_drops_unmatched_bases_only() {
        let mut gem = drop("gcg", 2, 0, false);
        gem.kind = DropKind::Gem(crate::items::GemColour::Emerald);
        let mut drops = vec![drop("crs", 2, 4, false), drop("crs", 2, 3, false), gem];
        apply(&mut drops, &config::Valuables::default());
        let left: Vec<_> = drops
            .iter()
            .map(|d| (d.code.as_deref(), d.sockets, d.highlight.as_ref().map(|h| h.tier)))
            .collect();
        assert_eq!(
            left,
            vec![(Some("crs"), 4, Some(Tier::A)), (Some("gcg"), 0, None)],
            "the 3os base is gone (B is off), the gem stays without a ring"
        );
    }
}

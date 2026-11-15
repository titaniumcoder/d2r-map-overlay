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
    found.map(|(r, uncertain)| Highlight {
        tier: r.tier,
        category: r.category,
        rule: r.name.clone(),
        uncertain,
    })
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

/// The keep list of the inventory `items`: every certain S or A match ([`keep_tiers`]) and
/// uncertain S matches (an uncertain A, e.g. a magic grand charm, is left out), identical ones
/// merged, best tier first, then by name.
pub fn keep_list(items: &[ItemDrop]) -> Vec<Keep> {
    let tiers = keep_tiers();
    let mut out: Vec<Keep> = Vec::new();
    for item in items {
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

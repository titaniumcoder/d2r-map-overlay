//! The open vendor trade window: the store items hanging off a vendor NPC's inventory
//! (read by `game::Reader::shop`), named from their base and magic affixes and matched
//! against the build's pieces.

use crate::offsets::item_data;
use crate::{affixes_data, item_levels_data, weapons_data};
use serde::{Deserialize, Serialize};

/// The trade window that is open right now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Shop {
    /// The vendor's monstats row (monster unit txt).
    pub npc_txt: u32,
    /// The vendor's English name; `None` for an NPC not in `affixes_data::VENDORS`.
    pub npc: Option<String>,
    /// The store items in the vendor inventory's chain order.
    pub items: Vec<ShopItem>,
}

/// One item for sale.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShopItem {
    /// The 3-character item code; `None` without the items table.
    pub code: Option<String>,
    /// `item_data::QUALITY` (2 normal, 3 superior, 4 magic, …).
    pub quality: u32,
    /// The store tab (`item_data::PAGE`): 0 armor, 1 and 2 weapons, 3 misc (live, Ormus).
    pub page: u8,
    /// The base item's English name (the item code when unknown).
    pub base: String,
    /// The magic prefix's English name (magic items only).
    pub prefix: Option<String>,
    /// The magic suffix's English name (magic items only).
    pub suffix: Option<String>,
    /// The name as the game shows it: `prefix base suffix`.
    pub name: String,
    /// max(base `levelreq`, prefix `levelreq`, suffix `levelreq`); `None` when the base is
    /// unknown and no affix sets one.
    pub req_level: Option<u32>,
    /// The item's stat entries (`game::Reader::item_stats`), for the comparison with the
    /// worn gear ([`crate::gear_score`]); empty when unread.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stats: Vec<crate::gear_score::ItemStat>,
}

/// Names a store item from its code, quality, store tab and raw affix ids
/// (`item_data::MAGIC_PREFIX` / `MAGIC_SUFFIX`, 0 = none; prefix ids are pre-shifted in
/// `affixes_data::PREFIXES`). Only magic items are decoded;
/// any other quality is shown by its base name.
#[must_use]
pub fn shop_item(code: Option<&str>, quality: u32, page: u8, prefix_id: u16, suffix_id: u16) -> ShopItem {
    let (base_level, base) = code.map_or((None, None), base_of);
    let magic = quality == item_data::QUALITY_MAGIC;
    // Id 0 is taken as "no affix". Live 2026-11-18: 9 store items "… of Health" were named with
    // their suffix and none of 1312 magic store items came out affix-less, so "of Health" does
    // not read 0.
    let prefix = (magic && prefix_id != 0)
        .then(|| affixes_data::prefix(prefix_id))
        .flatten();
    let suffix = (magic && suffix_id != 0)
        .then(|| affixes_data::suffix(suffix_id))
        .flatten();
    let req_level = [base_level, prefix.map(|p| p.1), suffix.map(|s| s.1)]
        .into_iter()
        .flatten()
        .max();
    let base = base.map_or_else(|| code.unwrap_or("?").to_owned(), str::to_owned);
    let name = [prefix.map(|p| p.0), Some(base.as_str()), suffix.map(|s| s.0)]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
    ShopItem {
        code: code.map(str::to_owned),
        quality,
        page,
        prefix: prefix.map(|p| p.0.to_owned()),
        suffix: suffix.map(|s| s.0.to_owned()),
        base,
        name,
        req_level,
        stats: Vec::new(),
    }
}

/// The base item's (`levelreq`, English name) from the armor, weapons or misc table.
fn base_of(code: &str) -> (Option<u32>, Option<&'static str>) {
    if let Some(name) = item_levels_data::armor_name(code) {
        return (item_levels_data::armor_level(code), Some(name));
    }
    if let Some(name) = weapons_data::name(code) {
        return (weapons_data::level(code), Some(name));
    }
    affixes_data::misc(code).map_or((None, None), |(lvl, name)| (Some(lvl), Some(name)))
}

/// Whether `item` is the build piece named `piece` (a bare piece name, see
/// [`crate::info::piece_name`]): the piece names the item's base plus any of its affixes, so
/// "Coral Heavy Gloves" matches Coral Heavy Gloves of Frost, "Sash" any sash, but "Russet
/// Heavy Boots of Haste" needs both affixes. Names compare with [`crate::info::same_name`].
#[must_use]
pub fn matches(item: &ShopItem, piece: &str) -> bool {
    let base = item.base.as_str();
    let pre = item.prefix.as_deref();
    let suf = item.suffix.as_deref();
    let forms = [
        Some(base.to_owned()),
        pre.map(|p| format!("{p} {base}")),
        suf.map(|s| format!("{base} {s}")),
        Some(item.name.clone()),
    ];
    forms.iter().flatten().any(|f| crate::info::same_name(f, piece))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Memory id of a magic prefix by name and level requirement (the table has several rows
    /// per name).
    fn prefix_id(name: &str, lvl: u32) -> u16 {
        affixes_data::PREFIXES
            .iter()
            .find(|&&(_, n, l)| n == name && l == lvl)
            .map_or(0, |&(i, _, _)| i)
    }

    #[test]
    fn decodes_magic_affixes_and_required_level() {
        // Live 2026-11-18: an owned Battle Staff of Teleportation had suffix id 532.
        let staff = shop_item(Some("bst"), 4, 1, 0, 532);
        assert_eq!(staff.name, "Battle Staff of Teleportation");
        assert_eq!(staff.suffix.as_deref(), Some("of Teleportation"));
        assert_eq!(
            staff.req_level,
            Some(24),
            "the suffix's levelreq beats the base's"
        );

        let coral = prefix_id("Coral", 13);
        // Prefix ids are shifted past the suffixes: 785 + the magicprefix row.
        assert!(coral > 785, "prefix ids are shifted");
        let gloves = shop_item(Some("vgl"), 4, 0, coral, 0);
        assert_eq!(gloves.name, "Coral Heavy Gloves");
        assert_eq!(gloves.req_level, Some(13));
    }

    #[test]
    fn non_magic_items_keep_their_base_name() {
        // Affix words on a non-magic item are not decoded.
        let sash = shop_item(Some("lbl"), 2, 0, 900, 90);
        assert_eq!((sash.name.as_str(), sash.prefix.as_deref()), ("Sash", None));
        let potion = shop_item(Some("hp1"), 2, 3, 0, 0);
        assert_eq!(potion.name, "Minor Healing Potion");
        assert_eq!(potion.req_level, Some(0));
        let unknown = shop_item(Some("zzz"), 2, 0, 0, 0);
        assert_eq!((unknown.name.as_str(), unknown.req_level), ("zzz", None));
    }

    #[test]
    fn suffix_zero_is_no_suffix() {
        let boots = shop_item(Some("vbt"), 4, 0, prefix_id("Russet", 9), 0);
        assert_eq!(boots.suffix, None);
        assert_eq!(boots.name, "Russet Heavy Boots");
    }

    #[test]
    fn piece_names_the_base_plus_any_affixes() {
        let gloves = ShopItem {
            code: Some("vgl".into()),
            quality: 4,
            page: 0,
            base: "Heavy Gloves".into(),
            prefix: Some("Coral".into()),
            suffix: Some("of Frost".into()),
            name: "Coral Heavy Gloves of Frost".into(),
            req_level: Some(13),
            stats: Vec::new(),
        };
        assert!(matches(&gloves, "Coral Heavy Gloves"));
        assert!(matches(&gloves, "heavy gloves of frost"));
        assert!(matches(&gloves, "Heavy Gloves"));
        assert!(matches(&gloves, "Coral Heavy Gloves of Frost"));
        assert!(!matches(&gloves, "Russet Heavy Gloves"));
        assert!(!matches(&gloves, "Coral Heavy Gloves of Haste"));
        assert!(!matches(&gloves, "Coral"));

        let boots = shop_item(Some("vbt"), 4, 0, prefix_id("Russet", 9), 112);
        assert_eq!(boots.name, "Russet Heavy Boots of Haste");
        assert!(matches(&boots, "Russet Heavy Boots of Haste"));
        let plain = shop_item(Some("vbt"), 4, 0, prefix_id("Russet", 9), 0);
        assert!(
            !matches(&plain, "Russet Heavy Boots of Haste"),
            "both affixes needed"
        );
    }
}

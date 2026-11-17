//! The shared stash as read from unit memory (see `research/notes/stash-format.md` §3 and
//! `PLAN.md`): each shared tab is a player-type unit without an act, one per page. The
//! player unit's own `Inventory` lists the live tabs in the game's UI order
//! ([`crate::offsets::inventory::TAB_LIST`]: the normal pages, then the gems/runes/materials
//! tab), and each tab unit's `Inventory` chains its items
//! ([`crate::offsets::inventory::FIRST_ITEM`]). Stale tab units from earlier characters linger
//! in the unit table; they are not in the list (and the fallback discriminator,
//! [`crate::offsets::inventory::OWNER_ID`] = the current player, leaves them out too).
//!
//! D2R loads the stash contents only once the stash has been opened in the game, and the tab
//! units vanish again later (UI closed, area transitions), so the session caches the captured
//! tabs for the rest of the game and drops the cache only when the player unit id changes
//! (user decision, 2026-11-17; see `session::Session`). A torn mid-load read (tabs present,
//! every tab still empty) never downgrades a cache that has items.

use serde::{Deserialize, Serialize};

use crate::game::ItemDrop;
use crate::items::{self, DropKind};
use crate::offsets::item_data;
use crate::valuables;

/// One item of a shared-stash tab, named for the dashboard and the `BiS` ticks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StashItem {
    /// The unique or set display name when known ([`valuables::unique_set_name`]), else the
    /// base name with `eth` and sockets, like the keep list ([`valuables::display_name`]); a
    /// materials-tab stack is named by [`material_name`] when its code is known.
    pub name: String,
    /// The item code (`"r01"`), `None` when the items table wasn't found.
    pub code: Option<String>,
    /// Raw `ItemData` quality (2 normal, 4 magic, 5 set, 6 rare, 7 unique; see
    /// [`item_data::QUALITY`]).
    pub quality: u8,
    pub identified: bool,
    pub ethereal: bool,
    pub runeword: bool,
    /// Stack size ([`item_data::QUANTITY`]): only items of the gems/runes/materials tab.
    pub quantity: Option<u32>,
}

/// Which shared-stash tab: a normal page, numbered 1-based in the game's UI order, or the
/// gems/runes/materials tab. JSON: `{"page": 1}` / `"materials"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TabKind {
    Page(u8),
    Materials,
}

/// One shared-stash tab with its items, in the tab's inventory order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StashTab {
    pub kind: TabKind,
    pub items: Vec<StashItem>,
}

/// The `StashItem` for a stored item's fields; `quantity` is the stack size of a
/// materials-tab item (`None` elsewhere).
#[must_use]
pub fn stash_item(drop: &ItemDrop, quantity: Option<u32>) -> StashItem {
    let material = quantity.and(drop.code.as_deref()).and_then(material_name);
    let name = material.unwrap_or_else(|| {
        valuables::unique_set_name(drop).map_or_else(|| valuables::display_name(drop), str::to_owned)
    });
    StashItem {
        name,
        code: drop.code.clone(),
        quality: u8::try_from(drop.quality).unwrap_or(u8::MAX),
        identified: drop.flags & item_data::FLAG_IDENTIFIED != 0,
        ethereal: drop.ethereal,
        runeword: drop.flags & item_data::FLAG_RUNEWORD != 0,
        quantity,
    }
}

/// Rune names by rune number (`r01` El … `r33` Zod), runes.txt order.
pub const RUNE_NAMES: [&str; 33] = [
    "El", "Eld", "Tir", "Nef", "Eth", "Ith", "Tal", "Ral", "Ort", "Thul", "Amn", "Sol", "Shael", "Dol",
    "Hel", "Io", "Lum", "Ko", "Fal", "Lem", "Pul", "Um", "Mal", "Ist", "Gul", "Vex", "Ohm", "Lo", "Sur",
    "Ber", "Jah", "Cham", "Zod",
];

/// Gem grade prefixes by [`items::gem`] grade; the normal grade has none.
const GEM_GRADES: [&str; 5] = ["Chipped ", "Flawed ", "", "Flawless ", "Perfect "];

/// Names of the other stackable codes the materials tab can hold (classic misc.txt plus the
/// D2R uber keys, organs and essences). Unknown codes keep their code as the name.
const OTHER_MATERIALS: [(&str, &str); 13] = [
    ("rvs", "Rejuvenation Potion"),
    ("rvl", "Full Rejuvenation Potion"),
    ("pk1", "Key of Terror"),
    ("pk2", "Key of Hate"),
    ("pk3", "Key of Destruction"),
    ("dhn", "Diablo's Horn"),
    ("bey", "Baal's Eye"),
    ("mbr", "Mephisto's Brain"),
    ("tes", "Twisted Essence of Suffering"),
    ("ceh", "Charged Essence of Hatred"),
    ("bet", "Burning Essence of Terror"),
    ("fed", "Festering Essence of Destruction"),
    ("toa", "Token of Absolution"),
];

/// The in-game name of a gem, rune or other stackable by its code (`"gcv"` → `"Chipped
/// Amethyst"`, `"r01"` → `"El Rune"`); `None` for an unknown code.
#[must_use]
pub fn material_name(code: &str) -> Option<String> {
    if let Some(&(_, name)) = OTHER_MATERIALS.iter().find(|(c, _)| *c == code) {
        return Some(name.to_owned());
    }
    let bytes = items::code_bytes(code)?;
    if let Some((colour, grade)) = items::gem(bytes) {
        let prefix = GEM_GRADES.get(usize::from(grade)).copied().unwrap_or_default();
        return Some(format!("{prefix}{}", colour.name()));
    }
    match items::code_kind(bytes) {
        Some(DropKind::Rune(n)) => RUNE_NAMES
            .get(usize::from(n).checked_sub(1)?)
            .map(|r| format!("{r} Rune")),
        _ => None,
    }
}

/// The runes and gems among `items` (the player's own inventory, cube and stash items; the
/// shared materials tab's stacks are counted from [`StashTab`]s instead) as `(name, count)`
/// with [`material_name`] names, sorted by name: the recipe inputs' ✓ on the dashboard.
#[must_use]
pub fn held_materials<'a>(items: impl IntoIterator<Item = &'a ItemDrop>) -> Vec<(String, u32)> {
    let mut out: Vec<(String, u32)> = Vec::new();
    for item in items {
        let Some(bytes) = item.code.as_deref().and_then(items::code_bytes) else {
            continue;
        };
        let material =
            items::gem(bytes).is_some() || matches!(items::code_kind(bytes), Some(DropKind::Rune(_)));
        let Some(name) = item.code.as_deref().filter(|_| material).and_then(material_name) else {
            continue;
        };
        match out.iter_mut().find(|(n, _)| *n == name) {
            Some((_, count)) => *count += 1,
            None => out.push((name, 1)),
        }
    }
    out.sort();
    out
}

/// The white (normal or superior quality) socketed bases among `items` that hold no runeword
/// yet, as `"[eth ]<base name> <n>os"`, sorted: the recipe base lines' hint on the dashboard.
#[must_use]
pub fn white_bases<'a>(items: impl IntoIterator<Item = &'a ItemDrop>) -> Vec<String> {
    let mut out: Vec<String> = items
        .into_iter()
        .filter(|i| {
            matches!(i.quality, item_data::QUALITY_NORMAL | item_data::QUALITY_SUPERIOR)
                && i.sockets > 0
                && i.flags & item_data::FLAG_RUNEWORD == 0
        })
        .filter_map(|i| {
            let code = i.code.as_deref()?;
            let base =
                crate::item_levels_data::armor_name(code).or_else(|| crate::weapons_data::name(code))?;
            let eth = if i.ethereal { "eth " } else { "" };
            Some(format!("{eth}{base} {}os", i.sockets))
        })
        .collect();
    out.sort();
    out
}

/// Display names of the identified unique/set items across `tabs`, first-seen order,
/// duplicates (case-insensitive) left out: the ✓ source for `BiS` pieces sitting in the
/// stash. An identified unique/set whose `item_data::FILE_INDEX` was unreadable keeps its
/// base name here, which no `BiS` piece name matches — a wrong tick is therefore unlikely.
#[must_use]
pub fn stash_names(tabs: &[StashTab]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for tab in tabs {
        for item in &tab.items {
            if matches!(
                u32::from(item.quality),
                item_data::QUALITY_SET | item_data::QUALITY_UNIQUE
            ) && item.identified
                && !out.iter().any(|n| n.eq_ignore_ascii_case(&item.name))
            {
                out.push(item.name.clone());
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(name: &str, quality: u32, flags: u32) -> StashItem {
        StashItem {
            name: name.to_owned(),
            code: None,
            quality: u8::try_from(quality).unwrap_or(u8::MAX),
            identified: flags & item_data::FLAG_IDENTIFIED != 0,
            ethereal: false,
            runeword: false,
            quantity: None,
        }
    }

    #[test]
    fn stash_names_keeps_identified_unique_and_set_names() {
        let id = item_data::FLAG_IDENTIFIED;
        let tabs = [StashTab {
            kind: TabKind::Page(1),
            items: vec![
                item("Harlequin Crest", item_data::QUALITY_UNIQUE, id),
                item("uap", item_data::QUALITY_UNIQUE, id), // index unreadable: base name
                item("Sigon's Visor", item_data::QUALITY_SET, id),
                item("Cathan's Seal", item_data::QUALITY_SET, 0), // unidentified: out
                item("Crystal Sword", item_data::QUALITY_NORMAL, id), // not unique/set: out
                item("Harlequin Crest", item_data::QUALITY_UNIQUE, id), // duplicate: out
            ],
        }];
        assert_eq!(
            stash_names(&tabs),
            vec!["Harlequin Crest", "uap", "Sigon's Visor"]
        );
        assert_eq!(stash_names(&[]), Vec::<String>::new());
    }

    #[test]
    fn material_names_cover_gems_runes_and_others() {
        assert_eq!(material_name("gcv").as_deref(), Some("Chipped Amethyst"));
        assert_eq!(material_name("gzv").as_deref(), Some("Flawless Amethyst"));
        assert_eq!(material_name("gsy").as_deref(), Some("Topaz"));
        assert_eq!(material_name("gpw").as_deref(), Some("Perfect Diamond"));
        assert_eq!(material_name("skz").as_deref(), Some("Perfect Skull"));
        assert_eq!(material_name("sku").as_deref(), Some("Skull"));
        assert_eq!(material_name("r01").as_deref(), Some("El Rune"));
        assert_eq!(material_name("r16").as_deref(), Some("Io Rune"));
        assert_eq!(material_name("r33").as_deref(), Some("Zod Rune"));
        assert_eq!(material_name("rvl").as_deref(), Some("Full Rejuvenation Potion"));
        assert_eq!(material_name("r34"), None);
        assert_eq!(material_name("crs"), None);
    }

    /// An item with `code`, `quality` and `sockets` (no flags) for the holdings tests.
    fn drop_of(code: &str, quality: u32, sockets: u8) -> ItemDrop {
        ItemDrop {
            id: 0,
            code: Some(code.to_owned()),
            kind: DropKind::Base,
            quality,
            flags: 0,
            ethereal: false,
            sockets,
            unique_index: None,
            item_data_head: String::new(),
            highlight: None,
            x: 0,
            y: 0,
        }
    }

    #[test]
    fn held_materials_count_runes_and_gems_only() {
        let items = [
            drop_of("r07", item_data::QUALITY_NORMAL, 0),
            drop_of("gcv", item_data::QUALITY_NORMAL, 0),
            drop_of("r07", item_data::QUALITY_NORMAL, 0),
            drop_of("rvl", item_data::QUALITY_NORMAL, 0),
            drop_of("crs", item_data::QUALITY_NORMAL, 4),
        ];
        assert_eq!(
            held_materials(&items),
            [("Chipped Amethyst".to_owned(), 1), ("Tal Rune".to_owned(), 2)],
            "potions and weapons are not recipe materials"
        );
    }

    #[test]
    fn white_bases_are_socketed_non_magic_items_without_a_runeword() {
        let mut runeword = drop_of("crs", item_data::QUALITY_NORMAL, 4);
        runeword.flags = item_data::FLAG_RUNEWORD;
        let mut eth = drop_of("crs", item_data::QUALITY_SUPERIOR, 4);
        eth.ethereal = true;
        let items = [
            drop_of("crs", item_data::QUALITY_NORMAL, 4),
            drop_of("crs", item_data::QUALITY_MAGIC, 4),
            drop_of("crs", item_data::QUALITY_NORMAL, 0),
            runeword,
            eth,
        ];
        assert_eq!(
            white_bases(&items),
            ["Crystal Sword 4os", "eth Crystal Sword 4os"]
        );
    }

    #[test]
    fn rune_names_are_the_generated_rune_set() {
        // The generated runeword data lists the same 33 names (in another order).
        let mut ours = RUNE_NAMES.to_vec();
        let mut generated = crate::runewords_data::RUNE_NAMES.to_vec();
        ours.sort_unstable();
        generated.sort_unstable();
        assert_eq!(ours, generated);
    }
}

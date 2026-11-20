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
use crate::names::ItemKind;
use crate::offsets::item_data;
use crate::runewords_data::{RUNEWORDS, Runeword};
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
    /// Socket count ([`ItemDrop::sockets`]); 0 in dumps written before it existed.
    #[serde(default)]
    pub sockets: u8,
    /// uniqueitems / setitems row of a unique or set item ([`ItemDrop::unique_index`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub unique_index: Option<u32>,
    /// The stats of a magic, rare or crafted item ([`ItemDrop::stats`]); empty otherwise.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub stats: Vec<crate::item_stats::ItemStat>,
}

impl StashItem {
    /// The item as the playstyle's rules take it ([`crate::stash_plan`]): the fields the
    /// reader had, the flags rebuilt from the identified / ethereal / runeword bits (the only
    /// ones the rules look at), no unit id or position.
    #[must_use]
    pub fn to_drop(&self) -> ItemDrop {
        let quality = u32::from(self.quality);
        let flag = |on: bool, bit: u32| if on { bit } else { 0 };
        ItemDrop {
            id: 0,
            code: self.code.clone(),
            kind: items::classify(quality, self.code.as_deref().and_then(items::code_bytes))
                .unwrap_or(DropKind::Base),
            quality,
            flags: flag(self.identified, item_data::FLAG_IDENTIFIED)
                | flag(self.ethereal, item_data::FLAG_ETHEREAL)
                | flag(self.runeword, item_data::FLAG_RUNEWORD),
            ethereal: self.ethereal,
            sockets: self.sockets,
            unique_index: self.unique_index,
            stats: self.stats.clone(),
            item_data_head: String::new(),
            x: 0,
            y: 0,
        }
    }
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
        sockets: drop.sockets,
        unique_index: drop.unique_index,
        stats: drop.stats.clone(),
    }
}

/// Rune names by rune number (`r01` El … `r33` Zod), runes.txt order.
pub const RUNE_NAMES: [&str; 33] = [
    "El", "Eld", "Tir", "Nef", "Eth", "Ith", "Tal", "Ral", "Ort", "Thul", "Amn", "Sol", "Shael", "Dol",
    "Hel", "Io", "Lum", "Ko", "Fal", "Lem", "Pul", "Um", "Mal", "Ist", "Gul", "Vex", "Ohm", "Lo", "Sur",
    "Ber", "Jah", "Cham", "Zod",
];

/// Required character level of each rune by rune number (`r01` El … `r33` Zod): misc.txt
/// `levelreq` (blizzhackers/d2data `misc.json`, D2R). Hel is 0 there, not 33.
const RUNE_LEVELS: [u32; 33] = [
    11, 11, 13, 13, 15, 15, 17, 19, 21, 23, 25, 27, 29, 31, 0, 35, 37, 39, 41, 43, 45, 47, 49, 51, 53, 55,
    57, 59, 61, 63, 65, 67, 69,
];

/// The rune number (1 El … 33 Zod) of a rune code (`"r07"` → 7); `None` for any other code.
fn rune_number(code: &str) -> Option<u8> {
    match items::code_kind(items::code_bytes(code)?) {
        Some(DropKind::Rune(n)) => Some(n),
        _ => None,
    }
}

/// The runeword spelled by the rune `codes` in socket order (`["r07", "r05"]` (Tal Eth) →
/// Stealth); `None` when a code isn't a rune or no runeword has exactly that sequence.
/// When several runewords share the runes (Shael Ko Eld is "Hustle (armor)" and
/// "Hustle (weapon)"), the one whose bases fit the item code `base` wins; without a code
/// (or no fit) the first in table order.
#[must_use]
pub fn runeword_by_runes(codes: &[String], base: Option<&str>) -> Option<&'static Runeword> {
    let names = codes
        .iter()
        .map(|c| {
            RUNE_NAMES
                .get(usize::from(rune_number(c)?).checked_sub(1)?)
                .copied()
        })
        .collect::<Option<Vec<&str>>>()?;
    let mut matches = RUNEWORDS.iter().filter(|w| w.runes == names.as_slice());
    let first = matches.next()?;
    let fits = |w: &Runeword| base.is_some_and(|code| base_fits(w, code));
    if fits(first) {
        return Some(first);
    }
    Some(matches.find(|w| fits(w)).unwrap_or(first))
}

/// The armour-side item-type groups of [`Runeword::bases`] (every other group in the table
/// is a weapon group: "weapons", "axes", "missile weapons", ...).
const ARMOR_GROUPS: [&str; 7] = [
    "armor",
    "auric shields",
    "grimoires",
    "helms",
    "paladin shields",
    "shields",
    "voodoo heads",
];

/// Whether runeword `word` can be made in the base with item code `code`, coarsely: a weapon
/// (a weapons.txt code) needs a weapon group, anything else an armour group.
fn base_fits(word: &Runeword, code: &str) -> bool {
    let weapon = crate::weapons_data::name(code).is_some();
    word.bases.iter().any(|b| ARMOR_GROUPS.contains(b) != weapon)
}

/// The required character level of runeword `word`'s runes: the highest rune `levelreq`
/// (a runeword item needs the max of that and its base's own requirement).
#[must_use]
pub fn runes_level(word: &Runeword) -> Option<u32> {
    word.runes
        .iter()
        .map(|r| {
            let i = RUNE_NAMES.iter().position(|n| n == r)?;
            RUNE_LEVELS.get(i).copied()
        })
        .collect::<Option<Vec<u32>>>()?
        .into_iter()
        .max()
}

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

/// Display names of the identified unique/set items and the named runewords (the reader
/// names them from their socketed runes) across `tabs`, first-seen order, duplicates
/// (case-insensitive) left out: the ✓ source for `BiS` pieces sitting in the stash. An
/// identified unique/set whose `item_data::FILE_INDEX` was unreadable keeps its base name
/// here, which no `BiS` piece name matches — a wrong tick is therefore unlikely.
#[must_use]
pub fn stash_names(tabs: &[StashTab]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for tab in tabs {
        for item in &tab.items {
            let unique_or_set = matches!(
                u32::from(item.quality),
                item_data::QUALITY_SET | item_data::QUALITY_UNIQUE
            ) && item.identified;
            let runeword = item.runeword && crate::runewords_data::lookup(&item.name).is_some();
            if (unique_or_set || runeword) && !out.iter().any(|n| n.eq_ignore_ascii_case(&item.name)) {
                out.push(item.name.clone());
            }
        }
    }
    out
}

/// What the account holds of named uniques, sets and runewords, for counting copies: the
/// farm list's status and the stage switch's `switch_requires` groups
/// ([`crate::playstyle::config::SwitchGroup`]).
#[derive(Debug, Clone, Copy)]
pub struct Holdings<'a> {
    /// The (session-cached) shared-stash tabs.
    pub stash: &'a [StashTab],
    /// The played character's identified unique/set and runeword names, each once
    /// ([`crate::game::Snapshot::owned_names`]: worn, carried, cubed, privately stashed).
    pub owned: &'a [String],
    /// One name per runeword copy outside the shared stash, the mercenary's worn ones
    /// included ([`crate::game::Snapshot::runeword_copies`]).
    pub runeword_copies: &'a [String],
    /// One name per identified unique/set item and runeword the mercenary wears
    /// ([`crate::game::Snapshot::merc_worn`]); only its uniques/sets count here, its runewords
    /// are already in `runeword_copies`.
    pub merc_worn: &'a [String],
}

impl Holdings<'_> {
    /// The copies of `name`: `(in the shared stash, on the played character or its
    /// mercenary)`, matched by [`crate::names::same_name`]. `kind` says which items count;
    /// `None` takes a runeword name ([`crate::runewords_data::lookup`]) as a runeword and any
    /// other as a unique or set. A runeword counts one per copy (the stash's runeword items
    /// and `runeword_copies`); a unique or set counts its identified stash items of that
    /// quality, at most one copy the character holds (`owned` lists each name once) and each
    /// one the mercenary wears.
    #[must_use]
    pub fn copies(&self, name: &str, kind: Option<ItemKind>) -> (u32, u32) {
        let same = |n: &str| crate::names::same_name(n, name);
        let stash_items = self.stash.iter().flat_map(|tab| &tab.items);
        let runeword = kind.map_or_else(
            || crate::runewords_data::lookup(name).is_some(),
            |kind| kind == ItemKind::Runeword,
        );
        let (stashed, carried) = if runeword {
            (
                stash_items.filter(|i| i.runeword && same(&i.name)).count(),
                self.runeword_copies.iter().filter(|n| same(n)).count(),
            )
        } else {
            let quality_ok = |q: u32| match kind {
                Some(ItemKind::Unique) => q == item_data::QUALITY_UNIQUE,
                Some(ItemKind::Set) => q == item_data::QUALITY_SET,
                _ => matches!(q, item_data::QUALITY_UNIQUE | item_data::QUALITY_SET),
            };
            (
                stash_items
                    .filter(|i| i.identified && quality_ok(u32::from(i.quality)) && same(&i.name))
                    .count(),
                usize::from(self.owned.iter().any(|n| same(n)))
                    .saturating_add(self.merc_worn.iter().filter(|n| same(n)).count()),
            )
        };
        let clamp = |n: usize| u32::try_from(n).unwrap_or(u32::MAX);
        (clamp(stashed), clamp(carried))
    }

    /// All copies of `name`, wherever held ([`Holdings::copies`] with the kind inferred).
    #[must_use]
    pub fn count(&self, name: &str) -> u32 {
        let (stashed, carried) = self.copies(name, None);
        stashed.saturating_add(carried)
    }
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
            sockets: 0,
            unique_index: None,
            stats: Vec::new(),
        }
    }

    #[test]
    fn stash_names_keeps_identified_unique_set_and_runeword_names() {
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
                StashItem {
                    runeword: true,
                    ..item("Stealth", item_data::QUALITY_NORMAL, id)
                },
                // A runeword the runes didn't name keeps its base name: out.
                StashItem {
                    runeword: true,
                    ..item("Studded Leather 2os", item_data::QUALITY_NORMAL, id)
                },
            ],
        }];
        assert_eq!(
            stash_names(&tabs),
            vec!["Harlequin Crest", "uap", "Sigon's Visor", "Stealth"]
        );
        assert_eq!(stash_names(&[]), Vec::<String>::new());
    }

    #[test]
    fn holdings_count_stash_character_and_merc_copies() {
        let id = item_data::FLAG_IDENTIFIED;
        let names = |n: &[&str]| n.iter().map(|&s| s.to_owned()).collect::<Vec<_>>();
        let tabs = [StashTab {
            kind: TabKind::Page(1),
            items: vec![
                item("Harlequin Crest", item_data::QUALITY_UNIQUE, id),
                item("Harlequin Crest", item_data::QUALITY_UNIQUE, 0), // unidentified: out
                item("Tal Rasha's Horadric Crest", item_data::QUALITY_SET, id),
                StashItem {
                    runeword: true,
                    ..item("Lawbringer", item_data::QUALITY_NORMAL, id)
                },
            ],
        }];
        let (owned, copies, merc) = (
            names(&["Harlequin Crest", "Lawbringer"]),
            names(&["Lawbringer"]),
            names(&["Harlequin Crest", "Andariel's Visage"]),
        );
        let holdings = Holdings {
            stash: &tabs,
            owned: &owned,
            runeword_copies: &copies,
            merc_worn: &merc,
        };
        assert_eq!(holdings.copies("harlequin crest", None), (1, 2), "own + merc's");
        assert_eq!(holdings.count("Harlequin Crest"), 3);
        assert_eq!(holdings.copies("Harlequin Crest", Some(ItemKind::Set)), (0, 2));
        assert_eq!(holdings.count("Lawbringer"), 2, "one per runeword copy");
        assert_eq!(holdings.count("Andariels Visage"), 1, "merc-worn only");
        assert_eq!(holdings.count("Tal Rasha's Horadric Crest"), 1);
        assert_eq!(holdings.count("Enigma"), 0);
    }

    #[test]
    fn shared_runes_pick_the_runeword_that_fits_the_base() {
        let hustle = ["r13", "r18", "r02"].map(str::to_owned); // Shael Ko Eld
        let name = |base| runeword_by_runes(&hustle, base).map(|w| w.name);
        assert_eq!(name(Some("7la")), Some("Hustle (weapon)"), "Feral Axe");
        assert_eq!(name(Some("utp")), Some("Hustle (armor)"), "Archon Plate");
        assert_eq!(name(None), Some("Hustle (armor)"), "no code: table order");
    }

    #[test]
    fn qualified_runeword_names_count_only_their_base() {
        let id = item_data::FLAG_IDENTIFIED;
        let tabs = [StashTab {
            kind: TabKind::Page(1),
            items: vec![StashItem {
                runeword: true,
                ..item("Hustle (armor)", item_data::QUALITY_NORMAL, id)
            }],
        }];
        let copies = vec!["Hustle (weapon)".to_owned()];
        let holdings = Holdings {
            stash: &tabs,
            owned: &copies,
            runeword_copies: &copies,
            merc_worn: &[],
        };
        assert_eq!(holdings.copies("Hustle (weapon)", None), (0, 1), "weapon only");
        assert_eq!(holdings.copies("Hustle (armor)", None), (1, 0), "armour only");
        assert_eq!(holdings.count("Hustle"), 2, "bare: both");
    }

    #[test]
    fn rune_sequences_name_their_runeword_in_socket_order() {
        let codes = |c: &[&str]| c.iter().map(|&s| s.to_owned()).collect::<Vec<_>>();
        // Live 2026-11-18: Elemental's body armour held r07 r05, the staff r03 r08.
        let stealth = runeword_by_runes(&codes(&["r07", "r05"]), None);
        assert_eq!(stealth.map(|w| w.name), Some("Stealth"));
        assert_eq!(stealth.and_then(runes_level), Some(17), "Tal 17 > Eth 15");
        let leaf = runeword_by_runes(&codes(&["r03", "r08"]), None);
        assert_eq!(leaf.map(|w| w.name), Some("Leaf"));
        assert_eq!(leaf.and_then(runes_level), Some(19));
        assert_eq!(
            runeword_by_runes(&codes(&["r08", "r09", "r07"]), None).map(|w| w.name),
            Some("Ancient's Pledge")
        );
        assert_eq!(
            runeword_by_runes(&codes(&["r05", "r07"]), None),
            None,
            "order matters"
        );
        assert_eq!(
            runeword_by_runes(&codes(&["r07"]), None),
            None,
            "a prefix is no runeword"
        );
        assert_eq!(
            runeword_by_runes(&codes(&["r07", "gcv"]), None),
            None,
            "a gem is no rune"
        );
        assert_eq!(runeword_by_runes(&[], None), None);
        // Hel's levelreq is 0, so Authority (Hel Shael Ral) needs Shael's 29.
        assert_eq!(
            crate::runewords_data::lookup("Authority").and_then(runes_level),
            Some(29)
        );
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
            stats: Vec::new(),
            item_data_head: String::new(),
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

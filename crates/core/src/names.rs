//! Item names shared by every part: the one name comparison ([`same_name`]), which table a
//! name comes from ([`ItemKind`]) and the display name of an identified unique or set item
//! ([`unique_set_name_parts`]).

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, PoisonError};

use serde::Deserialize;

use crate::game::ItemDrop;
use crate::item_names_data;
use crate::offsets::item_data;

/// Whether two item names are the same piece: ASCII case, surrounding spaces and
/// apostrophes are ignored, since the game data spells "Ancient's Pledge" and the builds
/// "Ancients' Pledge". The one name comparison for runewords, worn and owned pieces.
/// A trailing qualifier on one side only is ignored too: a bare "Hustle" is the same piece
/// as "Hustle (weapon)" and "Hustle (armor)", while those two stay different.
#[must_use]
pub fn same_name(a: &str, b: &str) -> bool {
    let bare = |s: &str| {
        s.trim()
            .chars()
            .filter(|&c| c != '\'')
            .map(|c| c.to_ascii_lowercase())
            .collect::<Vec<char>>()
    };
    let unqualified = |s: &str| {
        s.trim()
            .strip_suffix(')')
            .and_then(|s| s.rsplit_once(" ("))
            .map(|(name, _)| bare(name))
    };
    bare(a) == bare(b) || unqualified(a) == Some(bare(b)) || unqualified(b) == Some(bare(a))
}

/// What kind of item a name names: which table it comes from and how its copies are counted
/// ([`crate::stash::Holdings::copies`]). The farm list's `kind` (`config::FarmKind`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ItemKind {
    Runeword,
    Unique,
    Set,
}

impl ItemKind {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Runeword => "runeword",
            Self::Unique => "unique",
            Self::Set => "set",
        }
    }
}

/// The unique or set name (`index` string), if known: only for an identified item whose
/// file index is in the table of its quality. The index is verified for identified
/// uniques only (see `item_data::FILE_INDEX`); unidentified ones stay unknown (valuables name
/// rules then match as uncertain).
#[must_use]
pub fn identity(quality: u32, flags: u32, unique_index: Option<u32>) -> Option<&'static str> {
    if flags & item_data::FLAG_IDENTIFIED == 0 {
        return None;
    }
    let table = match quality {
        item_data::QUALITY_UNIQUE => item_names_data::UNIQUE_NAMES,
        item_data::QUALITY_SET => item_names_data::SET_NAMES,
        _ => return None,
    };
    let index = unique_index?;
    let row = table.binary_search_by_key(&index, |&(i, _)| i).ok()?;
    table.get(row).map(|&(_, name)| name)
}

/// The in-game display name of an identified unique/set item from its raw fields (`None`
/// otherwise: unidentified items carry no name index yet, non-unique/set items none at all).
/// `valuables::unique_set_name` takes an `ItemDrop`; the reader calls this one directly.
#[must_use]
pub fn unique_set_name_parts(quality: u32, flags: u32, unique_index: Option<u32>) -> Option<&'static str> {
    identity(quality, flags, unique_index).map(shown)
}

/// The in-game name of a unique/set `index` name: some internal keys differ (Piercerib is
/// shown as "Rogue's Bow"; see `item_names_data::DISPLAY_NAMES`).
#[must_use]
pub fn shown(index: &'static str) -> &'static str {
    item_names_data::DISPLAY_NAMES
        .binary_search_by_key(&index, |&(i, _)| i)
        .ok()
        .and_then(|row| item_names_data::DISPLAY_NAMES.get(row))
        .map_or(index, |&(_, name)| name)
}

/// The base item codes of the unique and set items named `name` (in-game or `index` spelling,
/// [`same_name`]); empty for any other name. Several rows can share a name across tables.
#[must_use]
pub fn named_base_codes(name: &str) -> Vec<&'static str> {
    named_bases(name).into_iter().map(|(_, code)| code).collect()
}

/// A unique or set row's quality and base code ([`named_bases`]).
pub type NamedBase = (u32, &'static str);

/// [`named_bases`], resolved once per name and kept for the process (the item plan asks for
/// every wanted piece on each evaluation, five times a second). The names come from the
/// playstyle's files, so the cache stays small; each entry is leaked once to hand out a
/// `'static` slice without a copy per call.
#[must_use]
pub fn named_bases_cached(name: &str) -> &'static [NamedBase] {
    static CACHE: LazyLock<Mutex<HashMap<String, &'static [NamedBase]>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));
    // A panic while holding the lock can't leave the map half-written (one insert), so a
    // poisoned lock is still usable.
    let mut cache = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(&bases) = cache.get(name) {
        return bases;
    }
    let bases: &'static [NamedBase] = Box::leak(named_bases(name).into_boxed_slice());
    cache.insert(name.to_owned(), bases);
    bases
}

/// [`named_base_codes`] with each code's quality (`item_data::QUALITY_UNIQUE` or
/// `QUALITY_SET`): what an unidentified drop of that item looks like.
#[must_use]
pub fn named_bases(name: &str) -> Vec<(u32, &'static str)> {
    let tables = [
        (
            item_data::QUALITY_UNIQUE,
            item_names_data::UNIQUE_NAMES,
            item_names_data::UNIQUE_CODES,
        ),
        (
            item_data::QUALITY_SET,
            item_names_data::SET_NAMES,
            item_names_data::SET_CODES,
        ),
    ];
    tables
        .into_iter()
        .flat_map(|(quality, names, codes)| {
            names
                .iter()
                .filter(|&&(_, index)| same_name(index, name) || same_name(shown(index), name))
                .filter_map(move |&(row, _)| {
                    let i = codes.binary_search_by_key(&row, |&(r, _)| r).ok()?;
                    codes.get(i).map(|&(_, code)| (quality, code))
                })
        })
        .collect()
}

/// The display name of an item: the unique or set name when known, else the base name with
/// `eth` and sockets (`"eth Crystal Sword 4os"`), from the core's item tables. The stash pages
/// are named with it ([`crate::stash::stash_item`]).
#[must_use]
pub fn display_name(item: &ItemDrop) -> String {
    if let Some(name) = unique_set_name_parts(item.quality, item.flags, item.unique_index) {
        return name.to_owned();
    }
    if let Some(name) = identity(item.quality, item.flags, item.unique_index) {
        return shown(name).to_owned();
    }
    let code = item.code.as_deref().unwrap_or("?");
    let base = crate::item_levels_data::armor_name(code)
        .or_else(|| crate::weapons_data::name(code))
        .or_else(|| crate::affixes_data::misc(code).map(|(_, name)| name))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_name_ignores_case_spaces_and_apostrophes() {
        assert!(same_name("Ancient's Pledge", "Ancients' Pledge"));
        assert!(same_name(" leaf", "Leaf"));
        assert!(!same_name("Lea", "Leaf"));
        assert!(same_name("Hustle (weapon)", "hustle"), "bare matches either base");
        assert!(same_name("Hustle", "Hustle (armor)"));
        assert!(!same_name("Hustle (weapon)", "Hustle (armor)"));
    }

    #[test]
    fn name_tables_are_sorted_by_row() {
        for table in [
            item_names_data::UNIQUE_NAMES,
            item_names_data::SET_NAMES,
            item_names_data::UNIQUE_CODES,
            item_names_data::SET_CODES,
        ] {
            assert!(
                table.windows(2).all(|w| matches!(w, [a, b] if a.0 < b.0)),
                "binary_search needs ascending, unique rows"
            );
            assert_ne!(table.len(), 0);
        }
    }

    #[test]
    fn named_items_have_their_base_codes() {
        // Ribcracker is a Quarterstaff (8ls), Sigon's Sabot (set) Greaves; a runeword has no row.
        assert_eq!(named_base_codes("Ribcracker"), ["8ls"]);
        assert_eq!(named_base_codes("Sigon's Sabot"), ["hbt"]);
        assert_eq!(named_base_codes("Spirit"), Vec::<&str>::new());
    }

    #[test]
    fn internal_names_are_shown_in_game_spelling() {
        assert_eq!(shown("Piercerib"), "Rogue's Bow");
        assert_eq!(shown("The Chieftan"), "The Chieftain");
        assert_eq!(shown("Gravenspine"), "Gravenspine", "same name stays");
        assert!(
            item_names_data::DISPLAY_NAMES.is_sorted_by(|a, b| a.0 < b.0),
            "sorted for binary search"
        );
    }
}

//! Item names: the one name comparison, name kinds and unique/set display names.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex, PoisonError};

use serde::Deserialize;

use crate::game::ItemDrop;
use crate::item_names_data;
use crate::offsets::item_data;

/// Whether two item names are the same piece, ignoring ASCII case, surrounding spaces and
/// apostrophes ("Ancient's Pledge" vs "Ancients' Pledge"), and a trailing "(qualifier)" on
/// one side only ("Hustle" matches "Hustle (weapon)", which differs from "Hustle (armor)").
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

/// Which table an item name comes from (the farm list's `kind`).
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

/// The unique or set `index` name of an identified item; unidentified items carry no
/// reliable file index.
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

/// The in-game display name of an identified unique/set item from its raw fields.
#[must_use]
pub fn unique_set_name_parts(quality: u32, flags: u32, unique_index: Option<u32>) -> Option<&'static str> {
    identity(quality, flags, unique_index).map(shown)
}

/// The in-game name of a unique/set `index` name (Piercerib is shown as "Rogue's Bow").
#[must_use]
pub fn shown(index: &'static str) -> &'static str {
    item_names_data::DISPLAY_NAMES
        .binary_search_by_key(&index, |&(i, _)| i)
        .ok()
        .and_then(|row| item_names_data::DISPLAY_NAMES.get(row))
        .map_or(index, |&(_, name)| name)
}

/// The base item codes of the unique and set items named `name`; several rows can share one.
#[must_use]
pub fn named_base_codes(name: &str) -> Vec<&'static str> {
    named_bases(name).into_iter().map(|(_, code)| code).collect()
}

pub type NamedBase = (u32, &'static str);

/// [`named_bases`], resolved once per name and leaked to hand out `'static` slices; the item
/// plan asks five times a second and the names come from the playstyle files, so it stays small.
#[must_use]
pub fn named_bases_cached(name: &str) -> &'static [NamedBase] {
    static CACHE: LazyLock<Mutex<HashMap<String, &'static [NamedBase]>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));
    // One insert can't leave the map half-written, so a poisoned lock is still usable.
    let mut cache = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(&bases) = cache.get(name) {
        return bases;
    }
    let bases: &'static [NamedBase] = Box::leak(named_bases(name).into_boxed_slice());
    cache.insert(name.to_owned(), bases);
    bases
}

/// [`named_base_codes`] with each code's quality: what an unidentified drop looks like.
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

/// The unique or set `index` name of a drop on base `code` when that base carries only one
/// item of the quality (an unidentified unique Shako is always Harlequin Crest).
#[must_use]
pub fn sole_name(quality: u32, code: &str) -> Option<&'static str> {
    let (names, codes) = match quality {
        item_data::QUALITY_UNIQUE => (item_names_data::UNIQUE_NAMES, item_names_data::UNIQUE_CODES),
        item_data::QUALITY_SET => (item_names_data::SET_NAMES, item_names_data::SET_CODES),
        _ => return None,
    };
    let mut rows = codes.iter().filter(|&&(_, c)| c == code).map(|&(row, _)| row);
    let row = rows.next()?;
    if rows.next().is_some() {
        return None;
    }
    let i = names.binary_search_by_key(&row, |&(r, _)| r).ok()?;
    names.get(i).map(|&(_, name)| name)
}

/// The base name of item `code` from the armor, weapon and misc tables.
#[must_use]
pub fn base_name(code: &str) -> Option<&'static str> {
    crate::item_levels_data::armor_name(code)
        .or_else(|| crate::weapons_data::name(code))
        .or_else(|| crate::affixes_data::misc(code).map(|(_, name)| name))
}

/// The display name of an item: the unique or set name when known, else the base name with
/// `eth` and sockets (`"eth Crystal Sword 4os"`).
#[must_use]
pub fn display_name(item: &ItemDrop) -> String {
    if let Some(name) = unique_set_name_parts(item.quality, item.flags, item.unique_index) {
        return name.to_owned();
    }
    if let Some(name) = identity(item.quality, item.flags, item.unique_index) {
        return shown(name).to_owned();
    }
    let code = item.code.as_deref().unwrap_or("?");
    let base = base_name(code).unwrap_or(code);
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
    fn sole_name_only_for_a_base_with_one_item() {
        let unique = item_data::QUALITY_UNIQUE;
        assert_eq!(sole_name(unique, "uap"), Some("Harlequin Crest"));
        assert_eq!(sole_name(unique, "cm1"), Some("Annihilus"));
        assert_eq!(sole_name(unique, "cm2"), Some("Hellfire Torch"));
        assert_eq!(sole_name(unique, "rin"), None, "many unique rings");
        assert_eq!(sole_name(unique, "cm3"), None, "Gheed's and the sunders");
        assert_eq!(sole_name(item_data::QUALITY_NORMAL, "uap"), None);
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

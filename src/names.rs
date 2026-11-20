//! Item names shared by every part: the one name comparison ([`same_name`]), which table a
//! name comes from ([`ItemKind`]) and the display name of an identified unique or set item
//! ([`unique_set_name_parts`]).

use serde::Deserialize;

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
pub fn shown(index: &'static str) -> &'static str {
    item_names_data::DISPLAY_NAMES
        .binary_search_by_key(&index, |&(i, _)| i)
        .ok()
        .and_then(|row| item_names_data::DISPLAY_NAMES.get(row))
        .map_or(index, |&(_, name)| name)
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
        for table in [item_names_data::UNIQUE_NAMES, item_names_data::SET_NAMES] {
            assert!(
                table.windows(2).all(|w| matches!(w, [a, b] if a.0 < b.0)),
                "binary_search needs ascending, unique rows"
            );
            assert_ne!(table.len(), 0);
        }
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

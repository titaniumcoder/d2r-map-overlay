//! The shared stash as read from unit memory (see `research/notes/stash-format.md` §3 and
//! `PLAN.md`): each shared tab is a player-type unit without an act, one per page, and its
//! items are ordinary page-4 item units whose `item_data::OWNER` is the tab unit's id. Only
//! the tab units whose `Inventory` names the current player as owner
//! ([`crate::offsets::inventory::OWNER_ID`]) are live; stale ones from earlier characters
//! linger and are excluded by that check (live 2026-11-15, Cathan's Seal moved pages 1–5).
//!
//! D2R loads the stash contents only once the stash has been opened in the game, and the tab
//! units vanish again later (UI closed, area transitions), so the session caches the captured
//! tabs for the rest of the game and drops the cache only when the player unit id changes
//! (user decision, 2026-11-17; see `session::Session`). A torn mid-load read (tabs present,
//! every tab still empty) never downgrades a cache that has items.

use serde::{Deserialize, Serialize};

use crate::game::ItemDrop;
use crate::offsets::item_data;
use crate::valuables;

/// One item of a shared-stash tab, named for the dashboard and the `BiS` ticks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StashItem {
    /// The unique or set display name when known ([`valuables::unique_set_name`]), else the
    /// base name with `eth` and sockets, like the keep list ([`valuables::display_name`]).
    pub name: String,
    /// Raw `ItemData` quality (2 normal, 4 magic, 5 set, 6 rare, 7 unique; see
    /// [`item_data::QUALITY`]).
    pub quality: u8,
    pub identified: bool,
    pub ethereal: bool,
    pub runeword: bool,
}

/// One shared-stash tab with its items. `index` counts the live tab units in unit-table
/// order (1-based): the mapping to the UI's tab number is NOT verified — the journal logs the
/// capture so a live check can confirm it (user has one screen; see `CLAUDE.md`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StashTab {
    pub index: u8,
    pub items: Vec<StashItem>,
}

/// The `StashItem` for a stored item's fields.
#[must_use]
pub fn stash_item(drop: &ItemDrop) -> StashItem {
    let name = valuables::unique_set_name(drop).map_or_else(|| valuables::display_name(drop), str::to_owned);
    StashItem {
        name,
        quality: u8::try_from(drop.quality).unwrap_or(u8::MAX),
        identified: drop.flags & item_data::FLAG_IDENTIFIED != 0,
        ethereal: drop.ethereal,
        runeword: drop.flags & item_data::FLAG_RUNEWORD != 0,
    }
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
            quality: u8::try_from(quality).unwrap_or(u8::MAX),
            identified: flags & item_data::FLAG_IDENTIFIED != 0,
            ethereal: false,
            runeword: false,
        }
    }

    #[test]
    fn stash_names_keeps_identified_unique_and_set_names() {
        let id = item_data::FLAG_IDENTIFIED;
        let tabs = [StashTab {
            index: 1,
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
}

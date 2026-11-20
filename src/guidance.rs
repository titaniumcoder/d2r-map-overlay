//! What the playstyle makes of one snapshot: the build's damage types, the `BiS` list, the
//! keep / sell / give-away lists and the ground-drop marks. The reader returns raw items
//! ([`crate::game::Snapshot`]); live mode (and `--render`) computes this next to it and hands
//! both to the renderer and the dashboard.

use std::collections::HashMap;

use crate::game::Snapshot;
use crate::{build, config, valuables};

/// The valuables highlight of each marked ground drop, by the drop's unit id
/// ([`crate::game::ItemDrop::id`]); drops without a mark aren't in it.
pub type Marks = HashMap<u32, valuables::Highlight>;

/// The playstyle's results for one snapshot. Kept apart from the reader's [`Snapshot`];
/// compared with it to skip re-rendering an unchanged frame.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Guidance {
    /// The character's build from the config ([`config::Config::damages_for_player`]), primary
    /// damage first; colours the info line's monster levels ([`build::rate`]).
    pub damages: Vec<build::Damage>,
    /// The character's `BiS` build and gear list ([`config::Config::resolved_bis`]); the
    /// dashboard's chase list.
    pub bis: Option<config::Bis>,
    /// Inventory items worth keeping ([`valuables::keep_list`]).
    pub keep: Vec<valuables::Keep>,
    /// Stored items to sell ([`valuables::spare_lists`]).
    pub sell: Vec<valuables::Spare>,
    /// Stored items to give away.
    pub give_away: Vec<valuables::Spare>,
    /// The ground-drop rings ([`valuables::apply`]).
    pub marks: Marks,
}

impl Guidance {
    /// The keep, sell and give-away lists of the snapshot's owned items
    /// ([`Snapshot::owned_items`]); the other fields stay empty.
    pub fn with_item_lists(snap: &Snapshot) -> Self {
        let (sell, give_away) = valuables::spare_lists(&snap.owned_items);
        Self {
            keep: valuables::keep_list(&snap.owned_items),
            sell,
            give_away,
            ..Self::default()
        }
    }
}

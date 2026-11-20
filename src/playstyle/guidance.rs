//! What the playstyle makes of one snapshot ([`super::Playstyle`]): the area rating, the `BiS`
//! list, the sell / move / keep / give-away plan of the player's items and the ground-drop
//! marks. The reader returns raw items ([`crate::game::Snapshot`]); live mode (and `--render`)
//! computes this next to it and hands both to the renderer and the dashboard.

use std::collections::HashMap;

use crate::{build, config, stash_plan, valuables};

/// The valuables highlight of each marked ground drop, by the drop's unit id
/// ([`crate::game::ItemDrop::id`]); drops without a mark aren't in it.
pub type Marks = HashMap<u32, valuables::Highlight>;

/// The playstyle's results for one snapshot. Kept apart from the reader's [`crate::game::Snapshot`];
/// compared with it to skip re-rendering an unchanged frame.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Guidance {
    /// How the current area suits the character's build ([`build::rate`] of its damage types,
    /// [`config::Config::damages_for_player`]); colours the info line's monster levels.
    pub rating: build::Rating,
    /// The character's `BiS` build and gear list ([`config::Config::resolved_bis`]); the
    /// dashboard's chase list.
    pub bis: Option<config::Bis>,
    /// What to do with each of the player's items, merged and sorted for the dashboard
    /// ([`stash_plan::plan`]).
    pub items: Vec<stash_plan::Advice>,
    /// The ground-drop rings ([`valuables::apply`]).
    pub marks: Marks,
}

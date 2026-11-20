//! What the playstyle makes of one snapshot ([`super::Playstyle`]): the area rating, the `BiS`
//! list, the sell / move / keep / give-away plan of the player's items and the ground-drop
//! marks. The reader returns raw items ([`crate::game::Snapshot`]); live mode (and `--render`)
//! computes this next to it and hands both to the renderer and the dashboard.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::valuables::{Category, Tier};
use crate::{build, config, stash_plan};

/// The mark of each marked ground drop, by the drop's unit id
/// ([`crate::game::ItemDrop::id`]); drops without a mark aren't in it.
pub type Marks = HashMap<u32, Mark>;

/// What a ground drop is marked for ([`crate::playstyle::marks`]); the map draws each kind in
/// its own style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MarkKind {
    /// For the player's own builds: a base for an open runeword need, a build piece.
    Keep,
    /// A farm-list unique or set still missing (a keep too, drawn with a star).
    Farm,
    /// Worth trading: a valuables rule ([`crate::valuables`]).
    Trade,
}

/// Why a ground drop is marked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mark {
    pub kind: MarkKind,
    /// A trade mark's valuables tier and category (the ring colour); `None` for keep and farm.
    pub trade: Option<(Tier, Category)>,
    /// The valuables rule's name, the piece's name, or "<runeword> base".
    pub rule: String,
    /// The match rests on what the overlay can't check (an unidentified unique's name, the
    /// item's stats such as an unknown enhanced defense): drawn dashed.
    pub uncertain: bool,
}

/// The playstyle's results for one snapshot. Kept apart from the reader's [`crate::game::Snapshot`];
/// compared with it to skip re-rendering an unchanged frame.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Guidance {
    /// How the current area suits the character's build ([`build::rate`] of its damage types,
    /// [`crate::playstyle::config::Settings::damages_for_player`]); colours the info line's monster levels.
    pub rating: build::Rating,
    /// The character's `BiS` build and gear list ([`crate::playstyle::config::Settings::resolved_bis`]); the
    /// dashboard's chase list.
    pub bis: Option<config::Bis>,
    /// What to do with each of the player's items, merged and sorted for the dashboard
    /// ([`stash_plan::plan`]).
    pub items: Vec<stash_plan::Advice>,
    /// The ground-drop rings ([`crate::playstyle::marks::apply`]).
    pub marks: Marks,
}

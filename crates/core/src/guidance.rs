//! What a playstyle makes of one snapshot ([`crate::playstyle::Playstyle`]): the area rating,
//! the `BiS` list, the sell / move / keep / give-away plan of the player's items and the
//! ground-drop marks, with the small types they are made of (valuables [`Tier`] and
//! [`Category`], an item's [`Spot`] and [`Action`], the [`Bis`] selection). The reader returns
//! raw items ([`crate::game::Snapshot`]); live mode (and `--render`) computes this next to it
//! and hands both to the renderer, the journal and the dashboard.

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

use crate::build;

/// The mark of each marked ground drop, by the drop's unit id
/// ([`crate::game::ItemDrop::id`]); drops without a mark aren't in it.
pub type Marks = HashMap<u32, Mark>;

/// What a ground drop is marked for (`d2r_playstyle_config::marks`); the map draws each kind in
/// its own style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MarkKind {
    /// For the player's own builds: a base for an open runeword need, a build piece.
    Keep,
    /// A farm-list unique or set still missing (a keep too, drawn with a star).
    Farm,
    /// Worth trading: a valuables rule (`d2r_playstyle_config::valuables`).
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
    /// e.g. `Settings::damages_for_player` in `d2r-playstyle-config`); colours the info line's
    /// monster levels.
    pub rating: build::Rating,
    /// The character's `BiS` build and gear list (`Settings::resolved_bis` there); the
    /// dashboard's chase list.
    pub bis: Option<Bis>,
    /// What to do with each of the player's items, merged and sorted for the dashboard
    /// (`d2r_playstyle_config::stash_plan::plan`).
    pub items: Vec<Advice>,
    /// The ground-drop rings (`d2r_playstyle_config::marks::apply`).
    pub marks: Marks,
}

/// How much a drop is worth. Declared lowest first, so the derived order is "worse < better"
/// (and the best ring is drawn last, on top).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Tier {
    B,
    A,
    S,
}

/// What a drop is valuable for: selling (`trade`, tradable runeword bases included) or a runeword
/// base for the player's own characters (`twink`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    Trade,
    Twink,
}

/// Where an item is: on the character (inventory, Horadric Cube, personal stash) or on a
/// shared-stash page (1-based, the game's UI order).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Spot {
    Inventory,
    Cube,
    Stash,
    Page(u8),
}

impl Spot {
    /// The short label shown after the item name: `inv`, `cube`, `stash`, `page 4`.
    #[must_use]
    pub fn label(self) -> String {
        match self {
            Self::Inventory => "inv".to_owned(),
            Self::Cube => "cube".to_owned(),
            Self::Stash => "stash".to_owned(),
            Self::Page(n) => format!("page {n}"),
        }
    }
}

/// What a kept item is kept for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Side {
    /// The player's own builds: shared pages 1–2.
    Own,
    /// Trading: shared pages 3–4.
    Trade,
}

impl Side {
    /// The shared pages of the side, as shown: `1–2` / `3–4`.
    #[must_use]
    pub const fn pages(self) -> &'static str {
        match self {
            Self::Own => "1–2",
            Self::Trade => "3–4",
        }
    }
}

/// What to do with an item. Declared in the dashboard's order (the derived `Ord` sorts by it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Action {
    Sell,
    /// Kept, but on the wrong place: move it to the side's pages.
    Move(Side),
    /// Kept where it is.
    Keep(Side),
    GiveAway,
}

/// One line of the dashboard's Sell / Keep panel: identical items (same name, spot and action)
/// merged with their count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Advice {
    pub name: String,
    pub spot: Spot,
    pub action: Action,
    /// Why a scored magic / rare item is sold ("no build stats", "worse than worn").
    pub reason: Option<&'static str>,
    pub count: u32,
}

/// A character's `BiS` selection and (legacy) inline gear list: `[bis.<name>]` (user request,
/// 2026-11-17). Since the build files exist, the webapp writes only `build` here; the inline
/// `slots`/`merc_slots` stay for hand-written entries and win over the selected build's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bis {
    /// The selected build's slug (lowercased build name from `builds/*.toml`, e.g.
    /// "blood boil"); empty when the entry is inline-only or nothing is selected.
    pub build: String,
    /// Slot name → pieces in chase order (`"Name@<required level>"`; the guide shows the
    /// highest-level one the character can currently use, so a slot can graduate from Spirit
    /// to Enigma as the character levels).
    pub slots: BTreeMap<String, Vec<String>>,
    /// The mercenary's slot name → pieces.
    pub merc_slots: BTreeMap<String, Vec<String>>,
}

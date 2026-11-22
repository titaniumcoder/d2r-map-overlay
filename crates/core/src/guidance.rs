//! What a playstyle makes of one snapshot: the area rating, the `BiS` list, the plan for the
//! player's items and the ground-drop marks.

use std::collections::{BTreeMap, HashMap};

use serde::{Deserialize, Serialize};

use crate::build;

/// Marked ground drops by unit id.
pub type Marks = HashMap<u32, Mark>;

/// What a ground drop is marked for; the map draws each kind in its own style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum MarkKind {
    /// For the player's own builds.
    Keep,
    /// A farm-list unique or set still missing.
    Farm,
    Trade,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mark {
    pub kind: MarkKind,
    /// A trade mark's tier and category; `None` for keep and farm.
    pub trade: Option<(Tier, Category)>,
    /// The valuables rule's name, the piece's name, or "<runeword> base".
    pub rule: String,
    /// The map tag beside the ring, at most 4 characters; empty for none.
    pub label: String,
    /// The match rests on what the overlay can't check (an unidentified name, unknown stats).
    pub uncertain: bool,
}

/// The playstyle's results for one snapshot.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Guidance {
    /// How the current area suits the character's build.
    pub rating: build::Rating,
    pub bis: Option<Bis>,
    /// What to do with each of the player's items, merged and sorted.
    pub items: Vec<Advice>,
    pub marks: Marks,
}

/// How much a drop is worth; declared lowest first, so the derived order is worse < better.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Tier {
    B,
    A,
    S,
}

/// Selling (`trade`) or a runeword base for the player's own characters (`twink`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Category {
    Trade,
    Twink,
}

/// Where an item is; `Page` is a 1-based shared-stash page.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Spot {
    Inventory,
    Cube,
    Stash,
    Page(u8),
}

impl Spot {
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Side {
    /// The player's own builds: shared pages 1–2.
    Own,
    /// Trading: shared pages 3–4.
    Trade,
}

impl Side {
    #[must_use]
    pub const fn pages(self) -> &'static str {
        match self {
            Self::Own => "1–2",
            Self::Trade => "3–4",
        }
    }
}

/// The stash window's tabs for stackables; in memory the three are one "advanced" tab unit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Tab {
    Gems,
    Materials,
    Runes,
}

impl Tab {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Gems => "gems",
            Self::Materials => "mats",
            Self::Runes => "runes",
        }
    }
}

/// What to do with an item, declared in the dashboard's order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Action {
    Sell,
    /// Kept, but on the wrong pages.
    Move(Side),
    /// A gem, rune or material outside its stash tab.
    Stow(Tab),
    Keep(Side),
    GiveAway,
}

/// One line of the Sell / Keep panel; identical items merged with their count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Advice {
    pub name: String,
    pub spot: Spot,
    pub action: Action,
    /// Why a scored magic / rare item is sold.
    pub reason: Option<&'static str>,
    pub count: u32,
}

/// A character's `BiS` selection; hand-written inline `slots` win over the selected build's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bis {
    /// The selected build's slug; empty when nothing is selected.
    pub build: String,
    /// Slot name → pieces in chase order, as `"Name@<required level>"`.
    pub slots: BTreeMap<String, Vec<String>>,
    pub merc_slots: BTreeMap<String, Vec<String>>,
}

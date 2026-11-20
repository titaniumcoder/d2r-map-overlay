//! The ground-drop marks (`PLAN.md`, plugin refactor step 10): what each drop on the map is
//! good for, in three kinds the core draws in its own style ([`MarkKind`]).
//! - **Farm** / **Keep**: a unique or set the player's builds still want, by name, or by
//!   quality and base while unidentified (uncertain); farm when it is a still-missing farm-list
//!   entry. A white base for an open runeword need, better than the base kept for it. The same
//!   rules as the item plan's "Keep · own" ([`stash_plan::Wants`]).
//! - **Trade**: the valuables rules of the enabled tiers ([`valuables::best`]); a white base of
//!   a twink rule below S is own gear, not a trade good (as in the item plan), so it is marked
//!   only as a keep. A superior base is uncertain (dashed) while its enhanced defense (armor) or
//!   enhanced damage (weapon) is unknown: the reader reads no stats of ground items, and the
//!   roll decides its value.
//! - An unidentified unique or set matched to a wanted piece only by its base is a guess (rule
//!   "Raven Frost?"): an S or A trade match on the same drop keeps its trade ring.
//!
//! Magic and rare drops get no keep mark: their stats aren't known before pickup.

use crate::config;
use crate::game::ItemDrop;
use crate::guidance::{Mark, MarkKind, Marks};
use crate::item_stats::stat;
use crate::items::DropKind;
use crate::offsets::item_data;
use crate::socket_bases;
use crate::valuables::{self, Tier};

use super::stash_plan::{self, Wants};

/// The marks of `drops` by drop id, from the valuables tiers `enabled` and the plan's `wants`;
/// removes the drops that are only shown for a mark ([`DropKind::Base`]) when none applies.
/// The reader keeps every base with a code, so this is the only filter on them.
pub fn apply(drops: &mut Vec<ItemDrop>, enabled: &config::Valuables, wants: &Wants) -> Marks {
    // White bases: twink rules below S are own gear, not trade goods.
    let mut for_bases = enabled.clone();
    for_bases.twink.retain(|&t| t == Tier::S);
    let mut marks = Marks::new();
    drops.retain(|d| {
        let Some(mark) = mark(d, (enabled, &for_bases), wants) else {
            return d.kind != DropKind::Base;
        };
        marks.insert(d.id, mark);
        true
    });
    marks
}

/// The mark of one drop: a wanted piece first (the player's builds win over trade, as in the
/// item plan), then an own base (never a trade-worthy one), then trade. A piece matched only
/// by its base (unidentified: a guess) yields to an S or A trade match: the guess must not hide
/// a stronger ring.
/// `enabled`: the tiers for most drops, and for white bases.
fn mark(
    drop: &ItemDrop,
    (enabled, for_bases): (&config::Valuables, &config::Valuables),
    wants: &Wants,
) -> Option<Mark> {
    let tiers = if stash_plan::plain_base(drop) {
        for_bases
    } else {
        enabled
    };
    let trade = || {
        valuables::best(drop, tiers).map(|h| Mark {
            kind: MarkKind::Trade,
            trade: Some((h.tier, h.category)),
            rule: h.rule,
            uncertain: h.uncertain || roll_unknown(drop),
        })
    };
    if let Some((rule, farm, uncertain)) = wants.piece(drop) {
        if uncertain && let Some(t) = trade().filter(|t| t.trade.is_some_and(|(tier, _)| tier >= Tier::A)) {
            return Some(t);
        }
        return Some(Mark {
            kind: if farm { MarkKind::Farm } else { MarkKind::Keep },
            trade: None,
            rule,
            uncertain,
        });
    }
    if let Some(word) = wants.base(drop) {
        return Some(Mark {
            kind: MarkKind::Keep,
            trade: None,
            rule: format!("{word} base"),
            uncertain: false,
        });
    }
    trade()
}

/// A superior base whose enhanced roll wasn't read: enhanced defense on an armor, helm or
/// shield, enhanced damage on a weapon. The roll decides what the base is worth.
fn roll_unknown(drop: &ItemDrop) -> bool {
    const ROLLS: [u16; 3] = [
        stat::DEFENSE_PERCENT,
        stat::MAX_DAMAGE_PERCENT,
        stat::MIN_DAMAGE_PERCENT,
    ];
    drop.quality == item_data::QUALITY_SUPERIOR
        && drop.code.as_deref().is_some_and(socket_bases::socketable)
        && !drop.stats.iter().any(|s| ROLLS.contains(&s.id))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn white(code: &str, quality: u32, sockets: u8) -> ItemDrop {
        ItemDrop {
            id: 1,
            code: Some(code.into()),
            kind: DropKind::Base,
            quality,
            flags: item_data::FLAG_IDENTIFIED,
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
    fn superior_trade_bases_are_dashed_while_their_roll_is_unknown() {
        let enabled = config::Valuables::default();
        let uncertain = |d: ItemDrop| {
            let mut drops = vec![d];
            apply(&mut drops, &enabled, &Wants::default())
                .get(&1)
                .map(|m| m.uncertain)
        };
        // Monarch 4os (armor, enhanced defense) and Berserker Axe 5os (weapon, enhanced damage).
        for code in ["uit", "7wa"] {
            let sockets = if code == "uit" { 4 } else { 5 };
            assert_eq!(
                uncertain(white(code, item_data::QUALITY_NORMAL, sockets)),
                Some(false),
                "{code}"
            );
            assert_eq!(
                uncertain(white(code, item_data::QUALITY_SUPERIOR, sockets)),
                Some(true),
                "{code}"
            );
        }
        let mut read = white("uit", item_data::QUALITY_SUPERIOR, 4);
        read.stats = vec![crate::item_stats::ItemStat::from_raw(
            0,
            stat::DEFENSE_PERCENT,
            15,
            false,
        )];
        assert_eq!(uncertain(read), Some(false), "the roll was read");
    }
}

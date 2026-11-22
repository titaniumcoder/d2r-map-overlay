//! The ground-drop marks: keep / farm from the builds' wants ([`stash_plan::Wants`]), trade from
//! the valuables rules ([`valuables::best`]). Magic and rare drops get no keep mark: their stats
//! aren't read before pickup.

use crate::config;
use crate::labels;
use crate::valuables::{self, Tier};
use d2r_core::game::ItemDrop;
use d2r_core::guidance::{Mark, MarkKind, Marks};
use d2r_core::item_stats::stat;
use d2r_core::items::DropKind;
use d2r_core::offsets::{item_data, stat_list};
use d2r_core::shop::{Shop, ShopItem};
use d2r_core::socket_bases;

use crate::stash_plan::{self, Wants};

/// The marks of `drops` by drop id; removes the unmarked [`DropKind::Base`] drops (the reader
/// keeps every base, this is their only filter).
pub fn apply(drops: &mut Vec<ItemDrop>, enabled: &config::Valuables, wants: &Wants) -> Marks {
    let for_bases = tiers_for_white_bases(enabled);
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

/// The mark each item of the open trade window would get as a ground drop, in window order:
/// a runeword base the builds want, or a valuables match. Store items are identified, so the
/// stat rules decide (a +3 wand). The window sells no uniques or sets, so no piece marks.
#[must_use]
pub fn shop_marks(shop: &Shop, enabled: &config::Valuables, wants: &Wants) -> Vec<Option<Mark>> {
    let for_bases = tiers_for_white_bases(enabled);
    shop.items
        .iter()
        .map(|item| mark(&as_drop(item), (enabled, &for_bases), wants))
        .collect()
}

/// A store item as the drop the rules judge; the socket count is its stat.
fn as_drop(item: &ShopItem) -> ItemDrop {
    let sockets = item
        .stats
        .iter()
        .find(|s| s.id == stat_list::NUM_SOCKETS)
        .and_then(|s| u8::try_from(s.value).ok())
        .unwrap_or(0);
    let kind = d2r_core::items::classify(
        item.quality,
        item.code.as_deref().and_then(d2r_core::items::code_bytes),
    );
    ItemDrop {
        cell: None,
        id: 0,
        code: item.code.clone(),
        kind: kind.unwrap_or(DropKind::Base),
        quality: item.quality,
        flags: item_data::FLAG_IDENTIFIED | item_data::FLAG_IN_STORE,
        ethereal: false,
        sockets,
        unique_index: None,
        stats: item.stats.clone(),
        item_data_head: String::new(),
        x: 0,
        y: 0,
    }
}

/// `enabled` without the twink tiers below S: such a white base is own gear, not a trade good.
fn tiers_for_white_bases(enabled: &config::Valuables) -> config::Valuables {
    let mut for_bases = enabled.clone();
    for_bases.twink.retain(|&t| t == Tier::S);
    for_bases
}

/// The mark of one drop: a wanted piece, then an own base, then trade. A piece guessed from an
/// unidentified base yields to an S or A trade match.
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
            label: labels::label(drop),
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
            label: labels::label(drop),
            uncertain,
        });
    }
    if let Some(word) = wants.base(drop) {
        return Some(Mark {
            kind: MarkKind::Keep,
            trade: None,
            rule: format!("{word} base"),
            label: labels::label(drop),
            uncertain: false,
        });
    }
    trade()
}

/// A superior base whose enhanced defense / damage roll wasn't read: the roll decides its worth.
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
            cell: None,
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
    fn store_items_are_judged_by_their_read_stats() {
        use d2r_core::guidance::Tier;
        use d2r_core::item_stats::ItemStat;
        let wand = |stats: Vec<ItemStat>| {
            let mut item = d2r_core::shop::shop_item(Some("wnd"), item_data::QUALITY_MAGIC, 1, 0, 0);
            item.stats = stats;
            item
        };
        // +3 to the necromancer's Poison and Bone tab (class 2, tab 1).
        let plus3 = ItemStat {
            id: stat::SKILL_TAB,
            param: 2 * 8 + 1,
            value: 3,
            base: false,
        };
        let shop = Shop {
            npc_txt: 0,
            npc: None,
            items: vec![wand(vec![plus3]), wand(Vec::new())],
        };
        let marks = shop_marks(&shop, &config::Valuables::default(), &Wants::default());
        let seen: Vec<Option<(MarkKind, Option<Tier>)>> = marks
            .iter()
            .map(|m| m.as_ref().map(|m| (m.kind, m.trade.map(|(t, _)| t))))
            .collect();
        assert_eq!(
            seen,
            [Some((MarkKind::Trade, Some(Tier::A))), None],
            "a +3 wand sells well, an identified plain one doesn't"
        );
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
        read.stats = vec![d2r_core::item_stats::ItemStat::from_raw(
            0,
            stat::DEFENSE_PERCENT,
            15,
            false,
        )];
        assert_eq!(uncertain(read), Some(false), "the roll was read");
    }
}

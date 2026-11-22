//! The buff tracker (PLAN.md, "Backlog: buff tracker"): which of the player's active states
//! ([`crate::game::Player::states`]) the dashboard's Buffs panel shows, and how.
//!
//! [`BUFFS`] is the curated set with display names: good states first (the druid's own, the
//! barbarian warcries, auras received, shrines, potions), then bad ones (curses and effects
//! on the player). Any other active state is shown muted by its states.txt name
//! ([`crate::states_data::STATES`]), so a live session shows what else turns up; [`HIDDEN`]
//! are the per-hit / per-cast internals that would only flicker.
//!
//! Stacks (Feral Rage) and time left are not read yet: [`Line`] carries them as `None` until
//! the buff's own stat list and the game's frame counter are known.

use serde::Serialize;

use crate::states_data::STATES;

/// Whether a state helps or hurts the player; [`Kind::Other`] for states outside [`BUFFS`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Good,
    Bad,
    Other,
}

/// One curated state: its id (states.txt `*ID`, the state bit), states.txt `state` name (a test
/// checks it against the generated table) and the name shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Buff {
    pub state: u32,
    pub internal: &'static str,
    pub name: &'static str,
    pub kind: Kind,
}

const fn good(state: u32, internal: &'static str, name: &'static str) -> Buff {
    Buff {
        state,
        internal,
        name,
        kind: Kind::Good,
    }
}

const fn bad(state: u32, internal: &'static str, name: &'static str) -> Buff {
    Buff {
        state,
        internal,
        name,
        kind: Kind::Bad,
    }
}

/// The states worth showing, in panel order within each kind.
pub const BUFFS: &[Buff] = &[
    // Druid.
    good(120, "feralrage", "Feral Rage"),
    good(139, "wolf", "Werewolf"),
    good(140, "bear", "Werebear"),
    good(149, "oaksage", "Oak Sage"),
    good(148, "wolverine", "Heart of Wolverine"),
    good(147, "barbs", "Spirit of Barbs"),
    good(151, "cyclonearmor", "Cyclone Armor"),
    good(144, "hurricane", "Hurricane"),
    good(145, "armageddon", "Armageddon"),
    // Warcries (Call to Arms, a barbarian).
    good(32, "battleorders", "Battle Orders"),
    good(51, "battlecommand", "Battle Command"),
    good(26, "shout", "Shout"),
    // Auras received (mercenary, runewords, a paladin). Holy Freeze, Holy Fire, Holy Shock
    // and Conviction are offensive auras: their state may sit on the caster only.
    good(33, "might", "Might"),
    good(43, "holywind", "Holy Freeze"),
    good(35, "holyfire", "Holy Fire"),
    good(46, "holyshock", "Holy Shock"),
    good(34, "prayer", "Prayer"),
    good(37, "defiance", "Defiance"),
    good(48, "meditation", "Meditation"),
    good(49, "fanaticism", "Fanaticism"),
    good(28, "conviction", "Conviction"),
    good(36, "thorns", "Thorns"),
    good(40, "blessedaim", "Blessed Aim"),
    good(42, "concentration", "Concentration"),
    good(41, "stamina", "Vigor"),
    good(8, "resistall", "Salvation"),
    good(3, "resistfire", "Resist Fire"),
    good(4, "resistcold", "Resist Cold"),
    good(5, "resistlight", "Resist Lightning"),
    good(45, "cleansing", "Cleansing"),
    good(47, "sanctuary", "Sanctuary"),
    good(50, "redemption", "Redemption"),
    // Shrines (states.txt flags these as curses; they are buffs).
    good(128, "shrine_armor", "Armor shrine"),
    good(129, "shrine_combat", "Combat shrine"),
    good(131, "shrine_resist_fire", "Resist Fire shrine"),
    good(132, "shrine_resist_cold", "Resist Cold shrine"),
    good(130, "shrine_resist_lightning", "Resist Lightning shrine"),
    good(133, "shrine_resist_poison", "Resist Poison shrine"),
    good(134, "shrine_skill", "Skill shrine"),
    good(135, "shrine_mana_regen", "Mana Recharge shrine"),
    good(136, "shrine_stamina", "Stamina shrine"),
    good(137, "shrine_experience", "Experience shrine"),
    // Potions.
    good(100, "healthpot", "Healing potion"),
    good(106, "manapot", "Mana potion"),
    good(179, "thawing", "Thawing potion"),
    good(178, "antidote", "Antidote potion"),
    good(180, "staminapot", "Stamina potion"),
    // Curses on the player. Life Tap is the player's good curse on monsters; cast on the
    // player it heals the monsters hitting them, so it counts as bad here.
    bad(9, "amplifydamage", "Amplify Damage"),
    bad(60, "decrepify", "Decrepify"),
    bad(61, "lowerresist", "Lower Resist"),
    bad(55, "ironmaiden", "Iron Maiden"),
    bad(19, "weaken", "Weaken"),
    bad(23, "dimvision", "Dim Vision"),
    bad(59, "confuse", "Confuse"),
    bad(57, "attract", "Attract"),
    bad(56, "terror", "Terror"),
    bad(58, "lifetap", "Life Tap"),
    bad(89, "battlecry", "Battle Cry"),
    // Effects on the player.
    bad(2, "poison", "Poisoned"),
    bad(11, "cold", "Chilled"),
    bad(1, "freeze", "Frozen"),
    bad(21, "stunned", "Stunned"),
    bad(24, "slowed", "Slowed"),
    bad(189, "desecrated", "Desecrated (Terror Zone)"),
];

/// Internal states set per hit, cast or step (`justhit`, `skilldelay`, `cooldown`,
/// `skill_move`, `just_portaled`, `skill_channeling`): not shown, and their changes don't
/// write a journal `buffs` event (they would on nearly every tick in a fight).
pub const HIDDEN: &[u32] = &[18, 86, 102, 121, 185, 216];

/// `states` without the [`HIDDEN`] ones: the set whose changes count (journal `buffs` event,
/// the reader's research dump).
#[must_use]
pub fn shown(states: &[u32]) -> Vec<u32> {
    states.iter().copied().filter(|id| !HIDDEN.contains(id)).collect()
}

/// The states.txt name of state `id`; `None` past the table's end.
#[must_use]
pub fn state_name(id: u32) -> Option<&'static str> {
    STATES.get(usize::try_from(id).ok()?).copied()
}

/// The curated entry of state `id`, if it has one.
#[must_use]
pub fn buff(id: u32) -> Option<&'static Buff> {
    BUFFS.iter().find(|b| b.state == id)
}

/// One line of the Buffs panel. `stacks` and `seconds_left` are `None` until the reader can
/// read them (the buff's own stat list and the game frame counter; see PLAN.md).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Line {
    pub state: u32,
    pub name: String,
    pub kind: Kind,
    pub stacks: Option<u32>,
    pub seconds_left: Option<u32>,
}

/// The panel lines for the active `states`: the good ones in [`BUFFS`] order, then the bad
/// ones in [`BUFFS`] order, then any other non-[`HIDDEN`] state by id as `state <id> <name>`.
#[must_use]
pub fn lines(states: &[u32]) -> Vec<Line> {
    let curated = |kind| {
        BUFFS
            .iter()
            .filter(move |b| b.kind == kind && states.contains(&b.state))
            .map(|b| Line {
                state: b.state,
                name: b.name.to_owned(),
                kind: b.kind,
                stacks: None,
                seconds_left: None,
            })
    };
    let mut others: Vec<u32> = states
        .iter()
        .copied()
        .filter(|&id| buff(id).is_none() && !HIDDEN.contains(&id))
        .collect();
    others.sort_unstable();
    others.dedup();
    let others = others.into_iter().map(|id| Line {
        state: id,
        name: format!("state {id} {}", state_name(id).unwrap_or("?")),
        kind: Kind::Other,
        stacks: None,
        seconds_left: None,
    });
    curated(Kind::Good)
        .chain(curated(Kind::Bad))
        .chain(others)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curated_ids_match_the_generated_table() {
        for b in BUFFS {
            assert_eq!(state_name(b.state), Some(b.internal), "{} ({})", b.name, b.state);
        }
        let mut ids: Vec<u32> = BUFFS.iter().map(|b| b.state).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), BUFFS.len(), "a state listed twice");
        assert!(BUFFS.iter().all(|b| b.kind != Kind::Other));
        assert!(HIDDEN.iter().all(|&id| buff(id).is_none()));
    }

    #[test]
    fn table_lookup() {
        assert_eq!(state_name(0), Some("none"));
        assert_eq!(state_name(189), Some("desecrated"));
        assert_eq!(state_name(120), Some("feralrage"));
        assert_eq!(state_name(100_000), None);
        assert_eq!(buff(32).map(|b| b.name), Some("Battle Orders"));
        assert_eq!(buff(61).map(|b| b.kind), Some(Kind::Bad));
        assert_eq!(buff(7), None, "playerbody is not curated");
    }

    #[test]
    fn lines_order_good_bad_other() {
        // Unsorted input with a curse, two buffs, an unknown state, a hidden one and a
        // duplicate.
        let l = lines(&[61, 165, 86, 32, 120, 7, 165]);
        let got: Vec<(u32, &str, Kind)> = l.iter().map(|l| (l.state, l.name.as_str(), l.kind)).collect();
        assert_eq!(
            got,
            [
                (120, "Feral Rage", Kind::Good),
                (32, "Battle Orders", Kind::Good),
                (61, "Lower Resist", Kind::Bad),
                (7, "state 7 playerbody", Kind::Other),
                (165, "state 165 itemset1", Kind::Other),
            ]
        );
        assert!(l.iter().all(|l| l.stacks.is_none() && l.seconds_left.is_none()));
        assert_eq!(lines(&[]), []);
        assert_eq!(
            lines(&[400]).first().map(|l| l.name.as_str()),
            Some("state 400 ?"),
            "past the table"
        );
    }
}

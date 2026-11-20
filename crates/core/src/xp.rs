//! How much experience an area's monsters give a character: the D2 level-difference rule, used to
//! grey out area names on the map that give (almost) none (user request, 2026-11-18).
//!
//! Source: Arreat Summit, <https://classic.battle.net/diablo2exp/basics/experience.shtml>; exact
//! factors in 256ths from <https://d2r-experience-calculator.pages.dev/calculations> (they round
//! to the Arreat percentages). No D2R patch up to Reign of the Warlock is known to change them.
//!
//! - Monster at most 5 levels from the character: 100%.
//! - Monster 6–10+ levels below: 81 / 62 / 43 / 24 / 5% (any character level).
//! - Monster above, character below level 25: 6–10+ levels above give 88 / 68 / 36 / 15 / 2%.
//!   This is what makes Act 5 Normal (monster level 24+) worthless up to clvl 15 (9+ levels
//!   below the monsters: 15% or less). There is no separate Act 5 rule.
//! - Monster above, character level 25 or more: `clvl / mlvl`, from the first level of difference.
//!
//! The clvl 70+ penalty (976/1024 at 70 down to 5/1024 at 99) is left out on purpose: it is the
//! same factor in every area, so it says nothing about where to go, and from clvl 86 on (below
//! 25%) it would grey every area.

use crate::info::TOWNS;
use crate::levels_data::LEVELS;

/// An area whose experience factor ([`multiplier`]) is below this gives "no or very little
/// experience" and its name is drawn grey; at or above it, white. 0.25 keeps a monster 9 levels
/// below (24%) and 9 above a low character (15%) grey, 8 levels either way (43% / 36%) white.
pub const LOW_XP: f64 = 0.25;

/// Below this character level, monsters above give the fixed table; from it on, `clvl / mlvl`.
const SCALED_FROM: u8 = 25;

/// Factor in 256ths for a monster `n` = 6..=10 levels below the character (index `n − 6`; more
/// than 10 counts as 10).
const BELOW: [u32; 5] = [207, 159, 110, 61, 13];
/// Factor in 256ths for a monster `n` = 6..=10 levels above a character below [`SCALED_FROM`].
const ABOVE_LOW_CLVL: [u32; 5] = [225, 174, 92, 38, 5];

/// The share (0..=1) of a monster's base experience a character of `clvl` gets for killing one of
/// `mlvl`, without party and clvl 70+ factors.
#[must_use]
pub fn multiplier(clvl: u8, mlvl: u8) -> f64 {
    // Index into the 6..=10 tables; differences beyond 10 clamp to the last entry.
    let step = |diff: u8| usize::from(diff.min(10) - 6);
    let in_256ths =
        |table: &[u32; 5], diff: u8| f64::from(table.get(step(diff)).copied().unwrap_or(256)) / 256.0;
    if clvl >= mlvl {
        match clvl - mlvl {
            0..=5 => 1.0,
            d => in_256ths(&BELOW, d),
        }
    } else if clvl >= SCALED_FROM {
        f64::from(clvl) / f64::from(mlvl)
    } else {
        match mlvl - clvl {
            1..=5 => 1.0,
            d => in_256ths(&ABOVE_LOW_CLVL, d),
        }
    }
}

/// True if the area `level_id` gives a character of `clvl` in `difficulty` (0 Normal … 2 Hell)
/// almost no experience ([`multiplier`] of its monster level below [`LOW_XP`]). False when
/// anything is unknown and for areas without monsters (towns, monster level 0).
#[must_use]
pub fn is_low(level_id: u32, difficulty: Option<u8>, clvl: Option<u8>) -> bool {
    let (Some(d), Some(clvl)) = (difficulty, clvl) else {
        return false;
    };
    if TOWNS.contains(&level_id) {
        return false;
    }
    LEVELS
        .iter()
        .find(|&&(id, _, _)| id == level_id)
        .and_then(|(_, _, lvls)| lvls.get(usize::from(d)).copied())
        .is_some_and(|mlvl| mlvl > 0 && multiplier(clvl, mlvl) < LOW_XP)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pct(clvl: u8, mlvl: u8) -> f64 {
        (multiplier(clvl, mlvl) * 1000.0).round() / 10.0
    }

    #[test]
    fn within_five_levels_is_full() {
        for (c, m) in [(10, 10), (10, 5), (10, 15), (30, 25), (1, 6)] {
            assert!((multiplier(c, m) - 1.0).abs() < f64::EPSILON, "{c} vs {m}");
        }
    }

    #[test]
    fn monsters_below_follow_the_arreat_table() {
        let got: Vec<f64> = (6..=12).map(|d| pct(40, 40 - d)).collect();
        assert_eq!(got, [80.9, 62.1, 43.0, 23.8, 5.1, 5.1, 5.1]);
        assert!((pct(12, 1) - 5.1).abs() < 1e-9, "same table below clvl 25");
    }

    #[test]
    fn monsters_above_a_low_character_follow_the_fixed_table() {
        let got: Vec<f64> = (6..=12).map(|d| pct(10, 10 + d)).collect();
        assert_eq!(got, [87.9, 68.0, 35.9, 14.8, 2.0, 2.0, 2.0]);
    }

    #[test]
    fn monsters_above_from_clvl_25_scale_by_level_ratio() {
        assert!(
            (multiplier(25, 26) - 25.0 / 26.0).abs() < 1e-12,
            "no 5-level grace"
        );
        assert!((multiplier(30, 60) - 0.5).abs() < 1e-12);
        assert!(
            (multiplier(24, 34) - 5.0 / 256.0).abs() < 1e-12,
            "clvl 24 still uses the table"
        );
    }

    #[test]
    fn low_xp_areas() {
        // Bloody Foothills (110), Normal mlvl 24: a clvl 10 Amazon gets 2%.
        assert!(is_low(110, Some(0), Some(10)));
        // clvl 16 is 8 below: 36%, white; clvl 15 is 9 below: 15%, grey.
        assert!(!is_low(110, Some(0), Some(16)));
        assert!(is_low(110, Some(0), Some(15)));
        // From clvl 25 on, above-level areas never grey out in practice.
        assert!(!is_low(110, Some(2), Some(25)));
        // Blood Moor (2), Normal mlvl 1: grey from clvl 10 (24%); clvl 9 is 8 above it (43%).
        assert!(is_low(2, Some(0), Some(10)));
        assert!(!is_low(2, Some(0), Some(9)));
    }

    #[test]
    fn towns_and_unknowns_stay_neutral() {
        assert!(!is_low(1, Some(0), Some(90)), "Rogue Encampment");
        assert!(!is_low(109, Some(2), Some(1)), "Harrogath");
        assert!(!is_low(110, None, Some(10)));
        assert!(!is_low(110, Some(0), None));
        assert!(!is_low(9999, Some(0), Some(10)));
        assert!(!is_low(110, Some(7), Some(10)), "unknown difficulty");
    }
}

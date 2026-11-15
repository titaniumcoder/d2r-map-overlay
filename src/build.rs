//! How well the current character's build fits the area: from Nightmare on, the info line's
//! monster-level part turns red where too many monster types are immune to the build's damage
//! and green where none is immune to its primary damage (user request, 2026-11-15).
//!
//! The build is the damage types from the config's `[builds]` (see [`crate::config::Builds`]),
//! primary first. The area's immunities are the generated `levels_data::IMMUNITIES` table:
//! per level and difficulty, the distinct monster types that spawn and how many are immune
//! (resist >= 100) to each damage type.

use crate::info::TOWNS;
use crate::levels_data::IMMUNITIES;

/// A damage type, in the order of the `IMMUNITIES` counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Damage {
    Physical,
    Fire,
    Cold,
    Lightning,
    Poison,
    Magic,
}

impl Damage {
    pub const ALL: [Self; 6] = [
        Self::Physical,
        Self::Fire,
        Self::Cold,
        Self::Lightning,
        Self::Poison,
        Self::Magic,
    ];

    pub const fn name(self) -> &'static str {
        match self {
            Self::Physical => "physical",
            Self::Fire => "fire",
            Self::Cold => "cold",
            Self::Lightning => "lightning",
            Self::Poison => "poison",
            Self::Magic => "magic",
        }
    }

    /// The damage type called `name` (case and surrounding spaces ignored).
    pub fn from_name(name: &str) -> Option<Self> {
        let wanted = name.trim().to_lowercase();
        Self::ALL.into_iter().find(|d| d.name() == wanted)
    }

    /// Index into an `IMMUNITIES` count array: the declaration order.
    const fn index(self) -> usize {
        self as usize
    }

    /// Bit of this type in an `IMMUNITIES` immunity mask.
    const fn bit(self) -> u8 {
        1 << self as u8
    }
}

/// How the area suits the build.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Rating {
    /// Normal, a town, no build, or neither good nor bad: the usual colour.
    #[default]
    Neutral,
    /// Red: see [`BAD_PRIMARY_SHARE`].
    Bad,
    /// Green: no monster type is immune to the primary damage.
    Good,
}

/// Bad from this share of the area's monster types immune to the primary damage on.
pub const BAD_PRIMARY_SHARE: f64 = 0.35;
/// Bad above this share of types immune to every damage of the build (i.e. any such type).
pub const BAD_UNCOVERED_SHARE: f64 = 0.0;
/// Good up to this share of types immune to the primary damage (i.e. none).
pub const GOOD_PRIMARY_SHARE: f64 = 0.0;

/// Rates level `level_id` in `difficulty` (0 Normal, 1 Nightmare, 2 Hell) for `build` (primary
/// damage first). Neutral in Normal, for an unknown difficulty or level, in towns, in areas
/// without monsters and without a build.
pub fn rate(level_id: u32, difficulty: Option<u8>, build: &[Damage]) -> Rating {
    let Some(&primary) = build.first() else {
        return Rating::Neutral;
    };
    let Some(d) = difficulty.filter(|&d| d >= 1).map(usize::from) else {
        return Rating::Neutral;
    };
    if TOWNS.contains(&level_id) {
        return Rating::Neutral;
    }
    let Some(&(types, immune, masks)) = IMMUNITIES
        .iter()
        .find(|&&(id, _)| id == level_id)
        .and_then(|(_, per_difficulty)| per_difficulty.get(d))
    else {
        return Rating::Neutral;
    };
    rate_counts(types, immune, masks, primary, build)
}

/// [`rate`] on one `IMMUNITIES` entry: `types` monster types, `immune` per damage type, the
/// immunity bitmask of each immune type in `masks`.
fn rate_counts(types: u8, immune: [u8; 6], masks: &[u8], primary: Damage, build: &[Damage]) -> Rating {
    if types == 0 {
        return Rating::Neutral;
    }
    let share = |n: usize| {
        // At most 20 types (10 spawn columns, plus 10 unique ones in Normal): never saturates.
        let n = u8::try_from(n).unwrap_or(u8::MAX);
        f64::from(n) / f64::from(types)
    };
    let primary_share = share(usize::from(immune.get(primary.index()).copied().unwrap_or(0)));
    let build_mask = build.iter().fold(0u8, |m, d| m | d.bit());
    let uncovered_share = share(masks.iter().filter(|&&m| m & build_mask == build_mask).count());
    if primary_share >= BAD_PRIMARY_SHARE || uncovered_share > BAD_UNCOVERED_SHARE {
        Rating::Bad
    } else if primary_share <= GOOD_PRIMARY_SHARE {
        Rating::Good
    } else {
        Rating::Neutral
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::levels_data::AreaImmunities;
    use Damage::{Cold, Fire, Lightning, Physical, Poison};

    const VEX: [Damage; 3] = [Lightning, Fire, Physical];

    fn row(level_id: u32) -> [AreaImmunities; 3] {
        IMMUNITIES
            .iter()
            .find(|&&(id, _)| id == level_id)
            .map_or([(0, [0; 6], &[] as &[u8]); 3], |&(_, r)| r)
    }

    #[test]
    fn generated_table_has_known_immunities() {
        assert!(IMMUNITIES.len() > 130, "one row per level");
        // City of the Damned (106), Nightmare: 7 types, one lightning immune (bighead5, ResLi(N)
        // 100); none in Normal.
        let [normal, nightmare, hell] = row(106);
        assert_eq!(normal.1, [0; 6]);
        assert_eq!((nightmare.0, nightmare.1), (7, [0, 0, 0, 1, 0, 0]));
        assert_eq!(nightmare.2, &[8], "lightning bit only");
        assert!(
            hell.1.get(3).is_some_and(|&n| n > 0),
            "lightning immunes in Hell too"
        );
        // Counts and masks agree everywhere, and never exceed the type count.
        for &(id, per_difficulty) in IMMUNITIES {
            for (types, immune, masks) in per_difficulty {
                for (i, &n) in immune.iter().enumerate() {
                    let from_masks = masks.iter().filter(|&&m| m & (1 << i) != 0).count();
                    assert_eq!(usize::from(n), from_masks, "level {id}");
                    assert!(n <= types, "level {id}");
                }
            }
        }
    }

    #[test]
    fn damage_names() {
        assert_eq!(Damage::from_name(" Lightning "), Some(Lightning));
        assert_eq!(Damage::from_name("poison"), Some(Poison));
        assert_eq!(Damage::from_name("holy"), None);
        assert!(
            Damage::ALL
                .iter()
                .all(|&d| Damage::from_name(d.name()) == Some(d))
        );
    }

    #[test]
    fn normal_towns_and_no_build_are_neutral() {
        assert_eq!(rate(106, Some(0), &VEX), Rating::Neutral, "Normal");
        assert_eq!(rate(106, None, &VEX), Rating::Neutral, "unknown difficulty");
        assert_eq!(rate(103, Some(2), &VEX), Rating::Neutral, "Pandemonium Fortress");
        assert_eq!(rate(83, Some(2), &[]), Rating::Neutral, "no build");
        assert_eq!(rate(9999, Some(1), &VEX), Rating::Neutral, "unknown level");
    }

    #[test]
    fn rating_rules() {
        // 4 types: 1 cold immune (25 %, below the bad share), another fire+cold immune.
        assert_eq!(
            rate_counts(4, [0, 0, 1, 0, 0, 0], &[4], Fire, &[Fire]),
            Rating::Good
        );
        assert_eq!(
            rate_counts(4, [0, 0, 1, 0, 0, 0], &[4], Cold, &[Cold, Fire]),
            Rating::Neutral
        );
        // Bad: 2 of 4 (50 %) immune to the primary.
        assert_eq!(
            rate_counts(4, [0, 0, 2, 0, 0, 0], &[4, 4], Cold, &[Cold, Fire]),
            Rating::Bad
        );
        // Exactly at the threshold: 7 of 20 = 0.35.
        assert_eq!(
            rate_counts(20, [0, 0, 7, 0, 0, 0], &[4; 7], Cold, &[Cold]),
            Rating::Bad
        );
        // Uncovered: one type immune to both of the build's damage types, though only 1 of 10.
        assert_eq!(
            rate_counts(10, [0, 1, 1, 0, 0, 0], &[6], Cold, &[Cold, Fire]),
            Rating::Bad
        );
        // The primary has no immunes, but one type is immune to all (a single-type build).
        assert_eq!(
            rate_counts(10, [0, 1, 0, 0, 0, 0], &[2], Cold, &[Cold]),
            Rating::Good
        );
        assert_eq!(
            rate_counts(0, [0; 6], &[], Cold, &[Cold]),
            Rating::Neutral,
            "no monsters"
        );
    }

    #[test]
    fn vex_in_nightmare() {
        // Travincal: no immunes in Nightmare.
        assert_eq!(rate(83, Some(1), &VEX), Rating::Good);
        // City of the Damned: 1 of 7 lightning immune (14 %), but not to fire: neutral.
        assert_eq!(rate(106, Some(1), &VEX), Rating::Neutral);
        // Hell City of the Damned: 4 of 7 lightning immune.
        assert_eq!(rate(106, Some(2), &VEX), Rating::Bad);
    }
}

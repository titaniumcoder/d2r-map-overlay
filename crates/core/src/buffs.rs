//! The player's states as the journal records them: states.txt names and the per-hit internals
//! left out.

use crate::states_data::STATES;

/// Internal per-hit/cast/step states (`justhit`, `skilldelay`, `cooldown`, `skill_move`,
/// `just_portaled`, `skill_channeling`): not journaled, as they flicker every tick.
pub const HIDDEN: &[u32] = &[18, 86, 102, 121, 185, 216];

/// `states` without the [`HIDDEN`] ones.
#[must_use]
pub fn shown(states: &[u32]) -> Vec<u32> {
    states.iter().copied().filter(|id| !HIDDEN.contains(id)).collect()
}

#[must_use]
pub fn state_name(id: u32) -> Option<&'static str> {
    STATES.get(usize::try_from(id).ok()?).copied()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_lookup() {
        assert_eq!(state_name(0), Some("none"));
        assert_eq!(state_name(189), Some("desecrated"));
        assert_eq!(state_name(120), Some("feralrage"));
        assert_eq!(state_name(100_000), None);
    }

    #[test]
    fn hidden_states_are_dropped() {
        assert_eq!(shown(&[32, 86, 120, 18]), [32, 120]);
    }
}

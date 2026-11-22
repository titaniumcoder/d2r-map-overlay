//! An item's stats as the reader delivers them, and the itemstatcost.txt ids the program reads.

use serde::{Deserialize, Serialize};

/// One stat-list entry of an item (`{u16 param, u16 stat id, u32 value}`) with a plain value.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemStat {
    /// itemstatcost.txt row.
    pub id: u16,
    /// The stat's parameter: the class of `+class skills`, the skill of `+single skill`, the
    /// tab of `+skill tab`; 0 for most stats.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub param: u16,
    pub value: i32,
    /// A base value (defense, damage, durability) that ED% scales, not a modifier.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub base: bool,
}

#[expect(
    clippy::trivially_copy_pass_by_ref,
    reason = "serde's skip_serializing_if passes a reference"
)]
const fn is_zero(v: &u16) -> bool {
    *v == 0
}

impl ItemStat {
    /// The entry from its raw words. Values are signed; life, mana and stamina (ids 6..=11,
    /// `ValShift` 8) are stored in 1/256 units.
    #[must_use]
    pub const fn from_raw(param: u16, id: u16, raw: u32, base: bool) -> Self {
        let value = raw.cast_signed();
        let value = if id >= 6 && id <= 11 { value >> 8 } else { value };
        Self {
            id,
            param,
            value,
            base,
        }
    }

    /// The stats of an item's aggregated array, which already has ED% applied: defense and the
    /// damage pairs in `base_ids` become base values, ED% entries are dropped, and so are damage
    /// stats the base lacks (the game writes a flat bonus to every pair; keeping them would make a
    /// two-hander look one-handed).
    #[must_use]
    pub fn from_totals(entries: &[(u16, u16, u32)], base_ids: &[u16]) -> Vec<Self> {
        const DAMAGE: [u16; 6] = [
            stat::MIN_DAMAGE,
            stat::MAX_DAMAGE,
            stat::MIN_DAMAGE_2H,
            stat::MAX_DAMAGE_2H,
            stat::THROW_MIN,
            stat::THROW_MAX,
        ];
        const ED: [u16; 3] = [
            stat::DEFENSE_PERCENT,
            stat::MAX_DAMAGE_PERCENT,
            stat::MIN_DAMAGE_PERCENT,
        ];
        entries
            .iter()
            .filter_map(|&(param, id, raw)| {
                let damage = DAMAGE.contains(&id);
                if ED.contains(&id) || (damage && !base_ids.contains(&id)) {
                    return None;
                }
                Some(Self::from_raw(param, id, raw, damage || id == stat::DEFENSE))
            })
            .collect()
    }
}

/// itemstatcost.txt stat ids the reader and the score read.
pub mod stat {
    pub const STRENGTH: u16 = 0;
    pub const ENERGY: u16 = 1;
    pub const DEXTERITY: u16 = 2;
    pub const VITALITY: u16 = 3;
    pub const MAX_LIFE: u16 = 7;
    pub const MAX_MANA: u16 = 9;
    /// Enhanced defense %.
    pub const DEFENSE_PERCENT: u16 = 16;
    /// Enhanced damage %, applied to the max and the min damage.
    pub const MAX_DAMAGE_PERCENT: u16 = 17;
    pub const MIN_DAMAGE_PERCENT: u16 = 18;
    pub const ATTACK_RATING: u16 = 19;
    /// One-handed damage.
    pub const MIN_DAMAGE: u16 = 21;
    pub const MAX_DAMAGE: u16 = 22;
    /// Two-handed damage.
    pub const MIN_DAMAGE_2H: u16 = 23;
    pub const MAX_DAMAGE_2H: u16 = 24;
    pub const DEFENSE: u16 = 31;
    pub const RES_FIRE: u16 = 39;
    pub const RES_LIGHTNING: u16 = 41;
    pub const RES_COLD: u16 = 43;
    pub const RES_POISON: u16 = 45;
    /// Elemental damage (min, max) pairs: fire, lightning, magic, cold.
    pub const ELEMENTAL: [(u16, u16); 4] = [(48, 49), (50, 51), (52, 53), (54, 55)];
    pub const LIFE_LEECH: u16 = 60;
    pub const MANA_LEECH: u16 = 62;
    pub const REPLENISH_LIFE: u16 = 74;
    pub const MAGIC_FIND: u16 = 80;
    /// +class skills; param = class.
    pub const CLASS_SKILLS: u16 = 83;
    /// Requirements -%.
    pub const REQUIREMENTS_PERCENT: u16 = 91;
    pub const IAS: u16 = 93;
    pub const FRW: u16 = 96;
    /// +skill (any class; "oskill"); param = skill.
    pub const NONCLASS_SKILL: u16 = 97;
    pub const FHR: u16 = 99;
    pub const FCR: u16 = 105;
    /// +skill (own class); param = skill.
    pub const SINGLE_SKILL: u16 = 107;
    pub const ALL_SKILLS: u16 = 127;
    /// Thrown damage.
    pub const THROW_MIN: u16 = 159;
    pub const THROW_MAX: u16 = 160;
    /// +skill tab; param = class × 8 + tab.
    pub const SKILL_TAB: u16 = 188;
    /// Skill charges ("Level 3 Teleport (12/12 charges)"); param = skill << 6 | level.
    pub const CHARGED_SKILL: u16 = 204;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn raw_values_are_signed_and_life_is_shifted() {
        assert_eq!(ItemStat::from_raw(0, 68, 0xFFFF_FFFB, true).value, -5);
        assert_eq!(ItemStat::from_raw(0, stat::MAX_LIFE, 20 << 8, false).value, 20);
        assert_eq!(ItemStat::from_raw(0, stat::RES_FIRE, 15, false).value, 15);
    }
}

//! Hand-kept object tables shared by the reader and the map.

/// Quest objects used up once operated: seen in a non-neutral mode, their marker goes away.
pub const CONSUMABLE_QUEST_OBJECTS: &[u32] = &[392, 393, 394, 395, 396];

#[must_use]
pub fn is_consumable_quest_object(txt: u32) -> bool {
    CONSUMABLE_QUEST_OBJECTS.contains(&txt)
}

/// The super chest's object preset (D2R objects.txt `PlaceUniqueChest`); PLAN.md "Super chests".
pub const SUPER_CHEST_PRESET: u32 = 584;

/// Levels whose super chests are drawn: Lower Kurast, Kurast Bazaar, Upper Kurast.
pub const SUPER_CHEST_LEVELS: std::ops::RangeInclusive<u32> = 79..=81;

/// The chest objects a super chest preset spawns as outside Travincal in Act 3 (jungle chest,
/// jungle medium chest left); seen opened, they hide the preset next to them.
pub const SUPER_CHEST_OBJECTS: &[u32] = &[181, 183];

#[must_use]
pub fn is_super_chest_object(txt: u32) -> bool {
    SUPER_CHEST_OBJECTS.contains(&txt)
}

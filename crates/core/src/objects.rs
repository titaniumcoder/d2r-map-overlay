//! Hand-kept object tables shared by the reader and the map.

/// Quest objects used up once operated: seen in a non-neutral mode, their marker goes away.
pub const CONSUMABLE_QUEST_OBJECTS: &[u32] = &[392, 393, 394, 395, 396];

#[must_use]
pub fn is_consumable_quest_object(txt: u32) -> bool {
    CONSUMABLE_QUEST_OBJECTS.contains(&txt)
}

//! Hand-kept object tables shared by the reader and the map (the generated ones are in
//! [`crate::objects_data`]).

/// Quest objects of [`crate::render::QUEST_OBJECTS`] that are used up once operated: their
/// marker goes away for the game when the live object (loaded near the player) is seen in a
/// non-neutral mode (see [`crate::session`]).
pub const CONSUMABLE_QUEST_OBJECTS: &[u32] = &[392, 393, 394, 395, 396];

/// True if object txt id `txt` is in [`CONSUMABLE_QUEST_OBJECTS`].
pub fn is_consumable_quest_object(txt: u32) -> bool {
    CONSUMABLE_QUEST_OBJECTS.contains(&txt)
}

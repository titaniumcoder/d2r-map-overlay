//! Which sounds one live frame earns: a new marked drop on the map, once per drop unit id. The
//! seen ids only grow per character, so a drop that flickers away for a tick never sounds twice.
//! A gem or skull never sounds (user, 2026-11-22: "sound exceptions: gems because funny enough,
//! there the game makes a sound itself").

use std::collections::HashSet;

use serde_json::Value;

use super::Sound;

/// What the overlay has seen of one character; a new one starts over.
#[derive(Debug)]
pub struct Memory {
    who: Option<String>,
    /// The unit ids of the marked drops seen.
    seen: HashSet<String>,
    /// The next frame only records its drops.
    quiet: bool,
}

impl Default for Memory {
    fn default() -> Self {
        Self {
            who: None,
            seen: HashSet::new(),
            quiet: true,
        }
    }
}

impl Memory {
    /// The game is in the menu or gone: the ids stay, the first frame back only records.
    pub const fn pause(&mut self) {
        self.quiet = true;
    }

    /// The sounds of one live frame (the dashboard's live JSON), in [`Sound::ALL`] order: its
    /// `drops` (every drop the minimap rings) not seen before, a `farm` mark as
    /// [`Sound::FarmDrop`], any other as [`Sound::MarkedDrop`]; the drop ids in `silent` (the
    /// gems) never sound. The first frame of a character, and the first after [`Self::pause`],
    /// only records.
    pub fn frame(&mut self, live: &Value, silent: &[u32]) -> Vec<Sound> {
        let who = live.pointer("/player/name").and_then(Value::as_str);
        if who != self.who.as_deref() {
            *self = Self {
                who: who.map(str::to_owned),
                ..Self::default()
            };
        }
        let quiet = std::mem::replace(&mut self.quiet, false);
        let drops = live
            .get("drops")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        let (mut farm, mut marked) = (false, false);
        let is_silent = |drop: &Value| {
            drop.get("id")
                .and_then(Value::as_u64)
                .and_then(|id| u32::try_from(id).ok())
                .is_some_and(|id| silent.contains(&id))
        };
        for drop in drops.iter().filter(|d| !is_silent(d)) {
            if self.seen.insert(text(drop.get("id"))) {
                if drop.get("kind").and_then(Value::as_str) == Some("farm") {
                    farm = true;
                } else {
                    marked = true;
                }
            }
        }
        if quiet {
            return Vec::new();
        }
        Sound::ALL
            .into_iter()
            .filter(|s| match s {
                Sound::FarmDrop => farm,
                Sound::MarkedDrop => marked,
            })
            .collect()
    }
}

/// A JSON value as a key: a string as is, anything else as JSON (`2`, `null`).
fn text(value: Option<&Value>) -> String {
    match value {
        Some(Value::String(s)) => s.clone(),
        Some(v) => v.to_string(),
        None => "null".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn frame(drops: &Value) -> Value {
        json!({ "live": true, "player": { "name": "Fury" }, "drops": drops })
    }

    impl Memory {
        /// [`Memory::frame`] without silent drops.
        fn hear(&mut self, live: &Value) -> Vec<Sound> {
            self.frame(live, &[])
        }
    }

    /// A memory past its quiet first frame.
    fn awake() -> Memory {
        let mut mem = Memory::default();
        assert_eq!(mem.hear(&frame(&json!([]))), []);
        mem
    }

    #[test]
    fn the_first_frame_only_records() {
        let mut mem = Memory::default();
        let r = frame(&json!([{ "id": 7, "kind": "farm", "tier": null }]));
        assert_eq!(mem.hear(&r), [], "quiet first frame");
        assert_eq!(mem.hear(&r), [], "already seen");
        let r = frame(&json!([{ "id": 7, "kind": "farm" }, { "id": 8, "kind": "farm" }]));
        assert_eq!(mem.hear(&r), [Sound::FarmDrop]);
    }

    #[test]
    fn every_marked_drop_sounds_once_per_unit() {
        let mut mem = awake();
        for (id, kind, tier) in [
            (1, "keep", None),
            (2, "trade", Some("B")),
            (3, "trade", Some("S")),
        ] {
            let r = frame(&json!([{ "id": id, "kind": kind, "tier": tier }]));
            assert_eq!(mem.hear(&r), [Sound::MarkedDrop], "{kind} {tier:?}");
            assert_eq!(mem.hear(&r), [], "once");
        }
        let farm = frame(&json!([{ "id": 4, "kind": "farm" }]));
        assert_eq!(mem.hear(&farm), [Sound::FarmDrop]);
        assert_eq!(mem.hear(&frame(&json!([]))), [], "the drop gone");
        assert_eq!(mem.hear(&farm), [], "a flicker back never sounds twice");
        assert_eq!(
            mem.hear(&json!({ "player": { "name": "Fury" } })),
            [],
            "no drops key"
        );
    }

    #[test]
    fn a_gem_never_sounds_even_on_the_farm_list() {
        let mut mem = awake();
        let r = frame(&json!([{ "id": 5, "kind": "farm" }, { "id": 6, "kind": "keep" }]));
        assert_eq!(mem.frame(&r, &[5, 6]), [], "both gems");
        let r = frame(&json!([
            { "id": 5, "kind": "farm" },
            { "id": 6, "kind": "keep" },
            { "id": 7, "kind": "trade", "tier": "S" },
        ]));
        assert_eq!(
            mem.frame(&r, &[5, 6]),
            [Sound::MarkedDrop],
            "only the new non-gem"
        );
    }

    #[test]
    fn a_busy_frame_sounds_each_kind_once_farm_first() {
        let mut mem = awake();
        let r = frame(&json!([
            { "id": 1, "kind": "keep" },
            { "id": 2, "kind": "trade", "tier": "A" },
            { "id": 3, "kind": "farm" },
            { "id": 4, "kind": "farm" },
        ]));
        assert_eq!(mem.hear(&r), [Sound::FarmDrop, Sound::MarkedDrop]);
    }

    #[test]
    fn a_pause_or_a_new_character_is_quiet_and_a_pause_keeps_the_ids() {
        let mut mem = awake();
        let one = frame(&json!([{ "id": 1, "kind": "keep" }]));
        let two = frame(&json!([{ "id": 1, "kind": "keep" }, { "id": 2, "kind": "keep" }]));
        assert_eq!(mem.hear(&one), [Sound::MarkedDrop]);
        mem.pause();
        assert_eq!(mem.hear(&two), [], "the first frame back records 2");
        assert_eq!(mem.hear(&two), [], "1 stays seen across the pause");
        let mut other = two.clone();
        if let Some(name) = other.pointer_mut("/player/name") {
            *name = json!("Summoner");
        }
        assert_eq!(mem.hear(&other), [], "a new character starts quiet");
        assert_eq!(mem.hear(&one), [], "back to Fury: a new memory again");
        assert_eq!(mem.hear(&two), [Sound::MarkedDrop]);
    }
}

//! Which sounds one live frame earns: every sound keys its events by stable ids whose seen sets
//! only grow per character, so a key that flickers away for a tick never sounds twice.

use std::collections::{HashMap, HashSet};

use serde_json::Value;

use super::Sound;

/// What the overlay has seen of one character; a new one starts over.
#[derive(Debug)]
pub struct Memory {
    who: Option<String>,
    seen: HashMap<Sound, HashSet<String>>,
    /// The worth-farming bosses seen per difficulty name.
    worth: HashMap<String, HashSet<String>>,
    /// `"<difficulty>|<boss>"` of the seen bosses missing from the previous frame.
    absent: HashSet<String>,
    stage: Option<String>,
    next_stage: Option<String>,
    /// The next frame only records its keys.
    quiet: bool,
}

impl Default for Memory {
    fn default() -> Self {
        Self {
            who: None,
            seen: HashMap::new(),
            worth: HashMap::new(),
            absent: HashSet::new(),
            stage: None,
            next_stage: None,
            quiet: true,
        }
    }
}

impl Memory {
    /// The game is in the menu or gone: the keys stay, the first frame back only records.
    pub const fn pause(&mut self) {
        self.quiet = true;
    }

    /// The sounds of one live frame (the dashboard's live JSON), in [`Sound::ALL`] order. The
    /// first frame of a character, and the first after [`Self::pause`], only records.
    pub fn frame(&mut self, live: &Value) -> Vec<Sound> {
        let who = live.pointer("/player/name").and_then(Value::as_str);
        if who != self.who.as_deref() {
            *self = Self {
                who: who.map(str::to_owned),
                ..Self::default()
            };
        }
        let quiet = std::mem::replace(&mut self.quiet, false);
        self.events(live, quiet)
    }

    /// Keys: drop unit ids (farm marks; trade marks of tier S / A), farm entries
    /// `for|name|copies` once held (a further copy of a multi-copy entry is a new key),
    /// `<stage>><next stage>` when the switch groups are all met or the stage switched on to its
    /// own next, and `<difficulty>|<boss>` for a worth-farming boss gone two frames in a row.
    fn events(&mut self, r: &Value, quiet: bool) -> Vec<Sound> {
        let list = |pointer: &str| {
            r.pointer(pointer)
                .and_then(Value::as_array)
                .map_or(&[][..], Vec::as_slice)
        };
        let drops = list("/drops");
        let drop_keys = |kind: &str, tiers: &[&str]| -> Vec<String> {
            drops
                .iter()
                .filter(|d| d.get("kind").and_then(Value::as_str) == Some(kind))
                .filter(|d| {
                    tiers.is_empty()
                        || tiers
                            .iter()
                            .any(|t| d.get("tier").and_then(Value::as_str) == Some(t))
                })
                .map(|d| format!("d{}", text(d.get("id"))))
                .collect()
        };
        let mut keys: HashMap<Sound, Vec<String>> = HashMap::from([
            (Sound::FarmDrop, drop_keys("farm", &[])),
            (Sound::TradeDrop, drop_keys("trade", &["S", "A"])),
            (
                Sound::Upgrade,
                list("/upgrades").iter().map(|u| text(u.get("key"))).collect(),
            ),
            (
                Sound::Pickup,
                list("/farm/held")
                    .iter()
                    .map(|h| {
                        format!(
                            "{}|{}|{}",
                            text(h.get("for")),
                            text(h.get("name")),
                            text(h.get("copies"))
                        )
                    })
                    .collect(),
            ),
            (Sound::SwitchReady, self.switch_keys(r)),
            (Sound::BossStop, self.boss_stop_keys(r, quiet)),
        ]);
        let mut out = Vec::new();
        for sound in Sound::ALL {
            let seen = self.seen.entry(sound).or_default();
            let mut fresh = false;
            for key in keys.remove(&sound).unwrap_or_default() {
                fresh |= seen.insert(key);
            }
            if fresh && !quiet {
                out.push(sound);
            }
        }
        out
    }

    /// The stage's next is ready, or the stage moved on to its own next (an automatic switch; a
    /// build picked by hand on the config page doesn't sound).
    fn switch_keys(&mut self, r: &Value) -> Vec<String> {
        let Some(stage) = r.get("stage").filter(|s| s.is_object()) else {
            return Vec::new();
        };
        let slug = text(stage.get("slug"));
        let next = stage.get("next").filter(|n| n.is_object());
        let mut keys = Vec::new();
        if let Some(next) = next
            && next.get("ready").and_then(Value::as_bool) == Some(true)
        {
            keys.push(format!("{slug}>{}", text(next.get("slug"))));
        }
        if let Some(was) = &self.stage
            && *was != slug
            && self.next_stage.as_deref() == Some(slug.as_str())
        {
            keys.push(format!("{was}>{slug}"));
        }
        self.next_stage = next.map(|n| text(n.get("slug")));
        self.stage = Some(slug);
        keys
    }

    /// A boss seen worth farming in this difficulty and missing two frames in a row (one in a
    /// quiet frame, which only records it).
    fn boss_stop_keys(&mut self, r: &Value, quiet: bool) -> Vec<String> {
        let Some(worth) = r.pointer("/farm/worth") else {
            return Vec::new();
        };
        let Some(difficulty) = worth
            .get("difficulty")
            .and_then(Value::as_str)
            .filter(|d| !d.is_empty())
        else {
            return Vec::new();
        };
        let now: HashSet<String> = worth
            .get("bosses")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice)
            .iter()
            .map(|b| text(b.get("boss")))
            .collect();
        let was = self.worth.entry(difficulty.to_owned()).or_default();
        let mut keys = Vec::new();
        let mut absent = HashSet::new();
        for boss in was.iter().filter(|b| !now.contains(*b)) {
            let key = format!("{difficulty}|{boss}");
            if quiet || self.absent.contains(&key) {
                keys.push(key.clone());
            }
            absent.insert(key);
        }
        self.absent = absent;
        was.extend(now);
        keys
    }
}

/// A JSON value as a key part: a string as is, anything else as JSON (`2`, `null`).
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

    fn frame(extra: Value) -> Value {
        let mut r = json!({ "live": true, "player": { "name": "Fury" } });
        if let (Some(obj), Value::Object(extra)) = (r.as_object_mut(), extra) {
            obj.extend(extra);
        }
        r
    }

    /// A memory past its quiet first frame.
    fn awake() -> Memory {
        let mut mem = Memory::default();
        assert_eq!(mem.frame(&frame(json!({}))), []);
        mem
    }

    #[test]
    fn the_first_frame_only_records() {
        let mut mem = Memory::default();
        let r = frame(json!({ "drops": [{ "id": 7, "kind": "farm", "tier": null }] }));
        assert_eq!(mem.frame(&r), [], "quiet first frame");
        assert_eq!(mem.frame(&r), [], "already seen");
        let r = frame(json!({ "drops": [{ "id": 7, "kind": "farm" }, { "id": 8, "kind": "farm" }] }));
        assert_eq!(mem.frame(&r), [Sound::FarmDrop]);
    }

    #[test]
    fn drops_sound_once_per_unit_and_trade_only_for_s_and_a() {
        let mut mem = awake();
        let r = frame(json!({ "drops": [
            { "id": 1, "kind": "trade", "tier": "B" },
            { "id": 2, "kind": "farm", "tier": null },
        ] }));
        assert_eq!(mem.frame(&r), [Sound::FarmDrop], "a B trade mark is silent");
        assert_eq!(mem.frame(&frame(json!({}))), [], "the drop gone");
        assert_eq!(mem.frame(&r), [], "a flicker back never sounds twice");
        let r = frame(json!({ "drops": [{ "id": 3, "kind": "trade", "tier": "A" }] }));
        assert_eq!(mem.frame(&r), [Sound::TradeDrop]);
    }

    #[test]
    fn upgrades_and_pickups_key_on_their_ids() {
        let mut mem = awake();
        let held = |copies: u32| json!({ "farm": { "held": [{ "for": "Fury", "name": "Grief", "copies": copies }] } });
        assert_eq!(mem.frame(&frame(held(1))), [Sound::Pickup]);
        assert_eq!(mem.frame(&frame(held(1))), []);
        assert_eq!(
            mem.frame(&frame(held(2))),
            [Sound::Pickup],
            "a further copy counts again"
        );
        let up = frame(json!({ "upgrades": [{ "key": "helm:Lore" }] }));
        assert_eq!(mem.frame(&up), [Sound::Upgrade]);
        assert_eq!(mem.frame(&up), []);
    }

    #[test]
    fn switch_ready_and_the_automatic_switch_sound_once() {
        let mut mem = awake();
        let stage = |slug: &str, next: Option<(&str, bool)>| {
            frame(json!({ "stage": {
                "slug": slug,
                "next": next.map(|(s, ready)| json!({ "slug": s, "ready": ready })),
            } }))
        };
        assert_eq!(mem.frame(&stage("summoner", Some(("fury", false)))), []);
        assert_eq!(
            mem.frame(&stage("summoner", Some(("fury", true)))),
            [Sound::SwitchReady]
        );
        assert_eq!(
            mem.frame(&stage("fury", None)),
            [],
            "the switch it announced is the same key"
        );
        let mut mem = awake();
        mem.frame(&stage("summoner", Some(("fury", false))));
        assert_eq!(
            mem.frame(&stage("fury", None)),
            [Sound::SwitchReady],
            "switched on to its next"
        );
        let mut mem = awake();
        mem.frame(&stage("summoner", Some(("fury", false))));
        assert_eq!(mem.frame(&stage("other", None)), [], "a build picked by hand");
    }

    #[test]
    fn a_worth_boss_gone_two_frames_in_a_row_sounds_once() {
        let mut mem = awake();
        let worth = |bosses: &[&str]| {
            frame(json!({ "farm": { "worth": {
                "difficulty": "Hell",
                "bosses": bosses.iter().map(|b| json!({ "boss": b })).collect::<Vec<_>>(),
            } } }))
        };
        assert_eq!(mem.frame(&worth(&["Andariel", "Mephisto"])), []);
        assert_eq!(mem.frame(&worth(&["Mephisto"])), [], "one frame gone");
        assert_eq!(
            mem.frame(&worth(&["Andariel", "Mephisto"])),
            [],
            "back: a flicker"
        );
        assert_eq!(mem.frame(&worth(&["Mephisto"])), []);
        assert_eq!(mem.frame(&worth(&["Mephisto"])), [Sound::BossStop], "gone twice");
        assert_eq!(mem.frame(&worth(&["Mephisto"])), [], "once");
    }

    #[test]
    fn a_pause_or_a_new_character_is_quiet_and_a_pause_keeps_the_keys() {
        let mut mem = awake();
        let one = frame(json!({ "upgrades": [{ "key": "a" }] }));
        let two = frame(json!({ "upgrades": [{ "key": "a" }, { "key": "b" }] }));
        assert_eq!(mem.frame(&one), [Sound::Upgrade]);
        mem.pause();
        assert_eq!(mem.frame(&two), [], "the first frame back records b");
        assert_eq!(mem.frame(&two), [], "a stays seen across the pause");
        let mut other = two.clone();
        if let Some(name) = other.pointer_mut("/player/name") {
            *name = json!("Summoner");
        }
        assert_eq!(mem.frame(&other), [], "a new character starts quiet");
        let three = frame(json!({ "upgrades": [{ "key": "c" }] }));
        assert_eq!(mem.frame(&three), [], "back to Fury: a new memory again");
        let four = frame(json!({ "upgrades": [{ "key": "c" }, { "key": "d" }] }));
        assert_eq!(mem.frame(&four), [Sound::Upgrade]);
    }

    #[test]
    fn a_busy_frame_lists_its_sounds_in_priority_order() {
        let mut mem = awake();
        let r = frame(json!({
            "drops": [{ "id": 1, "kind": "farm" }, { "id": 2, "kind": "trade", "tier": "S" }],
            "upgrades": [{ "key": "u" }],
            "farm": { "held": [{ "for": "Fury", "name": "Grief", "copies": 1 }] },
            "stage": { "slug": "a", "next": { "slug": "b", "ready": true } },
        }));
        assert_eq!(
            mem.frame(&r),
            [
                Sound::FarmDrop,
                Sound::SwitchReady,
                Sound::Upgrade,
                Sound::TradeDrop,
                Sound::Pickup
            ],
            "louder first, then the list order"
        );
    }
}

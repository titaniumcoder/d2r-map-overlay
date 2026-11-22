//! The overlay's sounds (user decision 2026-11-22: played by the overlay through the speakers,
//! whether or not the dashboard is open): which events of a live frame sound ([`Memory`]), the
//! tones ([`synth`]), the player thread, and the on/off switches the dashboard sets
//! (`sounds.json` in the data dir).

mod events;
mod player;
pub mod synth;

use std::collections::BTreeSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{OnceLock, PoisonError, RwLock};

use serde::{Deserialize, Serialize};

pub use events::Memory;
pub use player::play;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Sound {
    FarmDrop,
    SwitchReady,
    Upgrade,
    TradeDrop,
    Pickup,
    BossStop,
}

impl Sound {
    /// In priority order: the louder ones first.
    pub const ALL: [Self; 6] = [
        Self::FarmDrop,
        Self::SwitchReady,
        Self::Upgrade,
        Self::TradeDrop,
        Self::Pickup,
        Self::BossStop,
    ];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::FarmDrop => "farmDrop",
            Self::SwitchReady => "switchReady",
            Self::Upgrade => "upgrade",
            Self::TradeDrop => "tradeDrop",
            Self::Pickup => "pickup",
            Self::BossStop => "bossStop",
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::FarmDrop => "farm-list item on the ground",
            Self::SwitchReady => "ready to switch stage",
            Self::Upgrade => "upgrade available",
            Self::TradeDrop => "trade item (S / A) on the ground",
            Self::Pickup => "farm-list item picked up",
            Self::BossStop => "a boss is no longer worth farming",
        }
    }

    /// The two important ones play louder.
    #[must_use]
    pub const fn loud(self) -> bool {
        matches!(self, Self::FarmDrop | Self::SwitchReady)
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|s| s.name() == name)
    }
}

/// The dashboard's switches: all sounds (`on`) and the single sounds switched off by name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub on: bool,
    pub off: BTreeSet<String>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            on: true,
            off: BTreeSet::new(),
        }
    }
}

impl Settings {
    #[must_use]
    pub fn plays(&self, sound: Sound) -> bool {
        self.on && !self.off.contains(sound.name())
    }

    /// `{on, sounds: [{name, label, loud, on}]}` for the dashboard.
    #[must_use]
    pub fn json(&self) -> serde_json::Value {
        let sounds: Vec<_> = Sound::ALL
            .into_iter()
            .map(|s| {
                serde_json::json!({
                    "name": s.name(),
                    "label": s.label(),
                    "loud": s.loud(),
                    "on": !self.off.contains(s.name()),
                })
            })
            .collect();
        serde_json::json!({ "on": self.on, "sounds": sounds })
    }
}

/// `sounds.json` in the data dir; `None` if there is none.
#[must_use]
pub fn default_path() -> Option<PathBuf> {
    crate::journal::data_dir().map(|d| d.join("sounds.json"))
}

/// The defaults when the file is missing; an unreadable or invalid one is logged.
#[must_use]
pub fn load(path: &Path) -> Settings {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Settings::default(),
        Err(e) => {
            eprintln!("sounds {}: {e}; all sounds on", path.display());
            return Settings::default();
        }
    };
    serde_json::from_str(&text).unwrap_or_else(|e| {
        eprintln!("sounds {}: invalid ({e}); all sounds on", path.display());
        Settings::default()
    })
}

/// # Errors
/// Creating the data dir or writing the file failed.
pub fn save(path: &Path, settings: &Settings) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let text = serde_json::to_string_pretty(settings).map_err(io::Error::other)?;
    crate::web::write_atomic(path, &(text + "\n"))
}

/// The switches in use, loaded from [`default_path`] on first use.
static CURRENT: OnceLock<RwLock<Settings>> = OnceLock::new();

fn current_cell() -> &'static RwLock<Settings> {
    CURRENT.get_or_init(|| RwLock::new(default_path().map_or_else(Settings::default, |p| load(&p))))
}

#[must_use]
pub fn current() -> Settings {
    // A poisoned lock still holds whole settings: they are replaced in one assignment.
    current_cell()
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()
}

/// Changes the switches in use (the caller saved them).
pub fn set_current(settings: Settings) {
    *current_cell().write().unwrap_or_else(PoisonError::into_inner) = settings;
}

/// Sounds per frame; the rest of a busy frame is dropped (the louder come first).
const MAX_PER_FRAME: usize = 3;

/// Plays a frame's `sounds` (from [`Memory::frame`]), queued.
pub fn play_all(sounds: &[Sound]) {
    if sounds.is_empty() {
        return;
    }
    for sound in chosen(&current(), sounds) {
        play(sound);
    }
}

/// The first [`MAX_PER_FRAME`] of `sounds` that are switched on.
fn chosen(settings: &Settings, sounds: &[Sound]) -> Vec<Sound> {
    sounds
        .iter()
        .copied()
        .filter(|&s| settings.plays(s))
        .take(MAX_PER_FRAME)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::temp_dir;
    use anyhow::Result;

    #[test]
    fn a_frame_plays_the_first_three_switched_on() {
        let frame = [Sound::FarmDrop, Sound::SwitchReady, Sound::Upgrade, Sound::Pickup];
        let all = Settings::default();
        assert_eq!(
            chosen(&all, &frame),
            [Sound::FarmDrop, Sound::SwitchReady, Sound::Upgrade]
        );
        let no_farm = Settings {
            on: true,
            off: BTreeSet::from(["farmDrop".to_owned()]),
        };
        assert_eq!(
            chosen(&no_farm, &frame),
            [Sound::SwitchReady, Sound::Upgrade, Sound::Pickup],
            "a switched-off sound frees its place"
        );
        let muted = Settings { on: false, ..all };
        assert_eq!(chosen(&muted, &frame), []);
    }

    #[test]
    fn names_round_trip_and_are_unique() {
        for s in Sound::ALL {
            assert_eq!(Sound::from_name(s.name()), Some(s));
        }
        assert_eq!(Sound::from_name("chime"), None);
        assert_eq!(
            Sound::ALL.iter().filter(|s| s.loud()).count(),
            2,
            "farm drop and switch ready"
        );
    }

    #[test]
    fn settings_default_to_everything_on_and_round_trip() -> Result<()> {
        let dir = temp_dir("sounds")?;
        let path = dir.join("sounds.json");
        assert_eq!(load(&path), Settings::default(), "missing file");
        assert!(Sound::ALL.into_iter().all(|s| Settings::default().plays(s)));
        let settings = Settings {
            on: true,
            off: BTreeSet::from(["bossStop".to_owned()]),
        };
        save(&path, &settings)?;
        assert_eq!(load(&path), settings);
        assert!(!settings.plays(Sound::BossStop) && settings.plays(Sound::Pickup));
        let muted = Settings {
            on: false,
            ..settings
        };
        assert!(!muted.plays(Sound::FarmDrop), "the master switch");
        std::fs::write(&path, "{\"on\": false}")?;
        assert_eq!(
            load(&path),
            Settings {
                on: false,
                off: BTreeSet::new()
            },
            "missing keys default"
        );
        std::fs::write(&path, "not json")?;
        assert_eq!(load(&path), Settings::default(), "invalid file");
        std::fs::remove_dir_all(&dir)?;
        Ok(())
    }

    #[test]
    fn the_dashboard_json_lists_every_sound() {
        let settings = Settings {
            on: false,
            off: BTreeSet::from(["pickup".to_owned()]),
        };
        let json = settings.json();
        assert_eq!(json.get("on"), Some(&serde_json::json!(false)));
        let sounds = json
            .get("sounds")
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        assert_eq!(sounds.len(), Sound::ALL.len());
        assert_eq!(
            sounds.get(4),
            Some(&serde_json::json!({
                "name": "pickup", "label": "farm-list item picked up", "loud": false, "on": false
            }))
        );
    }
}

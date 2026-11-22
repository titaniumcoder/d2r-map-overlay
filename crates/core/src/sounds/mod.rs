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

/// A new drop the minimap marks with a ring (user decision 2026-11-22: "In principle whenever
/// something drops on the map that you mark, then you make a sound.").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Sound {
    /// A farm-list item; louder.
    FarmDrop,
    /// Any other marked drop (keep, trade).
    MarkedDrop,
}

impl Sound {
    /// In priority order: the louder first.
    pub const ALL: [Self; 2] = [Self::FarmDrop, Self::MarkedDrop];

    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::FarmDrop => "farmDrop",
            Self::MarkedDrop => "markedDrop",
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::FarmDrop => "farm-list drop",
            Self::MarkedDrop => "marked drop",
        }
    }

    #[must_use]
    pub const fn loud(self) -> bool {
        matches!(self, Self::FarmDrop)
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

/// The defaults when the file is missing; an unreadable or invalid one is logged. Names of
/// sounds that no longer exist are dropped.
#[must_use]
pub fn load(path: &Path) -> Settings {
    let mut settings = read(path);
    settings.off.retain(|name| Sound::from_name(name).is_some());
    settings
}

fn read(path: &Path) -> Settings {
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

/// Plays a frame's `sounds` (from [`Memory::frame`]) that are switched on, queued.
pub fn play_all(sounds: &[Sound]) {
    if sounds.is_empty() {
        return;
    }
    let settings = current();
    for &sound in sounds.iter().filter(|&&s| settings.plays(s)) {
        play(sound);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::temp_dir;
    use anyhow::Result;

    #[test]
    fn names_round_trip_and_are_unique() {
        for s in Sound::ALL {
            assert_eq!(Sound::from_name(s.name()), Some(s));
        }
        assert_eq!(Sound::from_name("chime"), None);
        assert_eq!(
            Sound::ALL.iter().filter(|s| s.loud()).collect::<Vec<_>>(),
            [&Sound::FarmDrop]
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
            off: BTreeSet::from(["markedDrop".to_owned()]),
        };
        save(&path, &settings)?;
        assert_eq!(load(&path), settings);
        assert!(!settings.plays(Sound::MarkedDrop) && settings.plays(Sound::FarmDrop));
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
        std::fs::write(
            &path,
            r#"{"on": true, "off": ["bossStop", "farmDrop", "pickup"]}"#,
        )?;
        assert_eq!(
            load(&path),
            Settings {
                on: true,
                off: BTreeSet::from(["farmDrop".to_owned()])
            },
            "the names of removed sounds are dropped"
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
            off: BTreeSet::from(["markedDrop".to_owned()]),
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
            sounds.get(1),
            Some(&serde_json::json!({
                "name": "markedDrop", "label": "marked drop", "loud": false, "on": false
            }))
        );
    }
}

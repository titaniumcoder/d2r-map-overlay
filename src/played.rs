//! When each character was last played, so the config page lists the recent ones first.
//!
//! `played.json` in the data dir maps the lowercased character name to its display name and
//! the unix time it was last seen in a game. [`Tracker`] records the live player each tick and
//! writes the file via a temp file and a rename, only when the character changed or at most
//! every [`WRITE_INTERVAL`]. Without the file (first start) it is seeded from D2R's own save
//! directory in the Proton prefix: online characters leave `<Name><account digits>.ctlo`
//! (key bindings, rewritten on leaving a game), offline ones `<Name>.d2s`; their modification
//! times are the recency. A missing or corrupt file only means no order: the page then keeps
//! the config order.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, UNIX_EPOCH};
use std::{fs, io};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Least time between two writes for the same character.
pub const WRITE_INTERVAL: Duration = Duration::from_secs(60);

/// One character's last sighting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// The name as the game spells it.
    pub name: String,
    /// Unix seconds.
    pub t: u64,
}

/// Lowercased character name → last sighting.
pub type Played = BTreeMap<String, Entry>;

/// `played.json` in the data dir ([`crate::journal::data_dir`]).
#[must_use]
pub fn default_path() -> Option<PathBuf> {
    crate::journal::data_dir().map(|d| d.join("played.json"))
}

/// The file's map; empty if it is missing or unreadable (logged, except a missing file).
#[must_use]
pub fn load(path: &Path) -> Played {
    read(path).unwrap_or_default()
}

/// `None` if the file is missing; `Some(empty)` if it can't be read or parsed (logged).
fn read(path: &Path) -> Option<Played> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return None,
        Err(e) => {
            eprintln!("played: reading {}: {e}", path.display());
            return Some(Played::new());
        }
    };
    Some(serde_json::from_str(&text).unwrap_or_else(|e| {
        eprintln!("played: parsing {} (ignored): {e}", path.display());
        Played::new()
    }))
}

/// Writes `played` to `path` through a temp file next to it.
fn write(path: &Path, played: &Played) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let tmp = path.with_extension("json.tmp");
    let json = serde_json::to_vec_pretty(played).context("serializing the played times")?;
    fs::write(&tmp, json).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("renaming {} to {}", tmp.display(), path.display()))
}

/// The character a save file belongs to: `Fulmina224078545.ctlo` → `Fulmina` (online names
/// carry the account digits; D2R names have no digits), `Wolfie.d2s` → `Wolfie`. `None` for
/// other files.
fn save_owner(file_name: &str) -> Option<&str> {
    let stem = file_name
        .strip_suffix(".ctlo")
        .or_else(|| file_name.strip_suffix(".d2s"))?;
    let name = stem.trim_end_matches(|c: char| c.is_ascii_digit());
    (!name.is_empty()).then_some(name)
}

/// A D2R character name: 2–15 ASCII letters, `-` or `_` (guards against a half-read name
/// during loading being persisted).
fn plausible_name(name: &str) -> bool {
    (2..=15).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphabetic() || b == b'-' || b == b'_')
}

/// Last-played times from the save files in `dir` (newest file per character); unreadable
/// entries are skipped.
fn seed_from_saves(dir: &Path) -> Played {
    let mut played = Played::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return played;
    };
    for entry in entries.filter_map(Result::ok) {
        let file_name = entry.file_name();
        let Some(name) = file_name.to_str().and_then(save_owner) else {
            continue;
        };
        let Some(t) = entry
            .metadata()
            .and_then(|m| m.modified())
            .ok()
            .and_then(|m| m.duration_since(UNIX_EPOCH).ok())
            .map(|d| d.as_secs())
        else {
            continue;
        };
        let slot = played.entry(name.to_lowercase()).or_insert_with(|| Entry {
            name: name.to_owned(),
            t,
        });
        slot.t = slot.t.max(t);
    }
    played
}

/// `WINEPREFIX` from a process's `/proc/<pid>/environ` bytes (NUL-separated `KEY=value`).
fn wineprefix(environ: &[u8]) -> Option<PathBuf> {
    environ
        .split(|&b| b == 0)
        .find_map(|var| var.strip_prefix(b"WINEPREFIX="))
        .and_then(|v| std::str::from_utf8(v).ok())
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// D2R's save directory under `prefix`: `drive_c/users/<user>/Saved Games/Diablo II
/// Resurrected` for the first user that has one (Proton's is `steamuser`).
fn saves_dir(prefix: &Path) -> Option<PathBuf> {
    let users = fs::read_dir(prefix.join("drive_c/users")).ok()?;
    users
        .filter_map(Result::ok)
        .map(|u| u.path().join("Saved Games/Diablo II Resurrected"))
        .find(|d| d.is_dir())
}

/// The seed for a missing file: the save files of the game process `pid` (its prefix from its
/// environment; same user, so no ptrace capability is needed). Empty when anything is missing.
#[must_use]
pub fn seed_from_process(pid: u32) -> Played {
    fs::read(format!("/proc/{pid}/environ"))
        .ok()
        .and_then(|env| wineprefix(&env))
        .and_then(|prefix| saves_dir(&prefix))
        .map_or_else(Played::new, |dir| seed_from_saves(&dir))
}

/// The last-played times and their file, fed by the live loop.
#[derive(Debug)]
pub struct Tracker {
    played: Played,
    /// `None`: no data dir, nothing is saved.
    path: Option<PathBuf>,
    /// The key and time of the last write.
    last_write: Option<(String, Instant)>,
    /// A write failed already: later failures are not logged again.
    failed: bool,
}

impl Tracker {
    /// Loads `path`; if it doesn't exist, takes `seed()` and writes it at once (so the seed
    /// runs only once).
    pub fn open(path: Option<PathBuf>, seed: impl FnOnce() -> Played) -> Self {
        let mut tracker = Self {
            played: Played::new(),
            path,
            last_write: None,
            failed: false,
        };
        let Some(path) = tracker.path.as_deref() else {
            return tracker;
        };
        if let Some(played) = read(path) {
            tracker.played = played;
        } else {
            tracker.played = seed();
            // An empty seed isn't written: the next start tries again (a `record` writes the
            // file anyway).
            if !tracker.played.is_empty() {
                let names: Vec<_> = tracker.played.values().map(|e| e.name.as_str()).collect();
                eprintln!("played: seeded from the save files: {names:?}");
                tracker.save();
            }
        }
        tracker
    }

    /// Notes `name` as seen at unix time `t` (ignored unless [`plausible_name`]); writes when the character differs from the last
    /// write's or [`WRITE_INTERVAL`] passed since it (`now`). An empty name is ignored.
    pub fn record(&mut self, name: &str, t: u64, now: Instant) {
        if !plausible_name(name) {
            return;
        }
        let key = name.to_lowercase();
        if let Some(entry) = self.played.get_mut(&key)
            && entry.name == name
        {
            entry.t = t;
        } else {
            self.played.insert(
                key.clone(),
                Entry {
                    name: name.to_owned(),
                    t,
                },
            );
        }
        if self
            .last_write
            .as_ref()
            .is_some_and(|(k, at)| *k == key && now.saturating_duration_since(*at) < WRITE_INTERVAL)
        {
            return;
        }
        self.last_write = Some((key, now));
        self.save();
    }

    fn save(&mut self) {
        let Some(path) = self.path.as_deref() else {
            return;
        };
        if let Err(e) = write(path, &self.played)
            && !std::mem::replace(&mut self.failed, true)
        {
            eprintln!("played: {e:#} (not saved; further errors not shown)");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::temp_dir;

    #[test]
    fn save_owner_strips_account_digits() {
        assert_eq!(save_owner("Fulmina224078545.ctlo"), Some("Fulmina"));
        assert_eq!(save_owner("Wolfie.d2s"), Some("Wolfie"));
        assert_eq!(save_owner("Fulmina224078545.keyo"), None);
        assert_eq!(save_owner("Settings.json"), None);
        assert_eq!(save_owner("123.ctlo"), None);
    }

    #[test]
    fn implausible_names_are_not_recorded() {
        let mut tracker = Tracker::open(None, Played::new);
        for name in ["", "X", "Fulmina\u{1}", "ThisNameIsWayTooLong", "Vex2"] {
            tracker.record(name, 1, Instant::now());
        }
        assert!(tracker.played.is_empty());
        tracker.record("Blood-Boil_x", 1, Instant::now());
        assert_eq!(tracker.played.len(), 1);
    }

    #[test]
    fn an_empty_seed_is_not_written() -> Result<()> {
        let dir = temp_dir("played-empty-seed")?;
        let path = dir.join("played.json");
        let _tracker = Tracker::open(Some(path.clone()), Played::new);
        assert!(!path.exists(), "the next start seeds again");
        Ok(())
    }

    #[test]
    fn wineprefix_is_read_from_environ() {
        let env = b"HOME=/home/u\0WINEPREFIX=/p/pfx/\0X=1\0";
        assert_eq!(wineprefix(env), Some(PathBuf::from("/p/pfx/")));
        assert_eq!(wineprefix(b"HOME=/home/u\0"), None);
    }

    #[test]
    fn seed_reads_the_save_files_of_a_prefix() -> Result<()> {
        let prefix = temp_dir("played-prefix")?;
        let saves = prefix.join("drive_c/users/steamuser/Saved Games/Diablo II Resurrected");
        fs::create_dir_all(&saves)?;
        fs::write(saves.join("Vex223650631.ctlo"), "")?;
        fs::write(saves.join("Vex223650631.keyo"), "")?;
        fs::write(saves.join("Settings.json"), "")?;
        let dir = saves_dir(&prefix).context("saves dir")?;
        let played = seed_from_saves(&dir);
        assert_eq!(played.keys().collect::<Vec<_>>(), ["vex"]);
        assert_eq!(played.get("vex").map(|e| e.name.as_str()), Some("Vex"));
        assert!(played.get("vex").is_some_and(|e| e.t > 0));
        Ok(())
    }

    #[test]
    fn round_trip_and_seed_only_once() -> Result<()> {
        let dir = temp_dir("played-roundtrip")?;
        let path = dir.join("played.json");
        let seed = || {
            Played::from([(
                "vex".to_owned(),
                Entry {
                    name: "Vex".to_owned(),
                    t: 5,
                },
            )])
        };
        let mut tracker = Tracker::open(Some(path.clone()), seed);
        assert!(path.exists(), "the seed is written at once");
        tracker.record("Fulmina", 9, Instant::now());
        let loaded = load(&path);
        assert_eq!(loaded.get("vex").map(|e| e.t), Some(5));
        assert_eq!(
            loaded.get("fulmina"),
            Some(&Entry {
                name: "Fulmina".to_owned(),
                t: 9
            })
        );
        // A second open reads the file, not the seed.
        let again = Tracker::open(Some(path), Played::new);
        assert_eq!(again.played, loaded);
        Ok(())
    }

    #[test]
    fn writes_are_throttled_per_character() -> Result<()> {
        let dir = temp_dir("played-throttle")?;
        let path = dir.join("played.json");
        let mut tracker = Tracker::open(Some(path.clone()), Played::new);
        let t0 = Instant::now();
        tracker.record("Vex", 1, t0);
        tracker.record("Vex", 2, t0 + WRITE_INTERVAL / 2);
        assert_eq!(
            load(&path).get("vex").map(|e| e.t),
            Some(1),
            "same character waits"
        );
        tracker.record("Fulmina", 3, t0 + WRITE_INTERVAL / 2);
        assert_eq!(
            load(&path).get("vex").map(|e| e.t),
            Some(2),
            "a switch writes at once"
        );
        tracker.record("Fulmina", 4, t0 + WRITE_INTERVAL);
        assert_eq!(
            load(&path).get("fulmina").map(|e| e.t),
            Some(3),
            "within the interval"
        );
        tracker.record("Fulmina", 5, t0 + WRITE_INTERVAL * 2);
        assert_eq!(load(&path).get("fulmina").map(|e| e.t), Some(5));
        Ok(())
    }

    #[test]
    fn a_corrupt_file_is_tolerated_and_not_reseeded() -> Result<()> {
        let dir = temp_dir("played-corrupt")?;
        fs::create_dir_all(&dir)?;
        let path = dir.join("played.json");
        fs::write(&path, "{not json")?;
        assert!(load(&path).is_empty());
        let mut tracker = Tracker::open(Some(path.clone()), || {
            Played::from([(
                "x".to_owned(),
                Entry {
                    name: "X".to_owned(),
                    t: 1,
                },
            )])
        });
        assert!(tracker.played.is_empty(), "a corrupt file is not a missing one");
        tracker.record("Vex", 7, Instant::now());
        assert_eq!(
            load(&path).get("vex").map(|e| e.t),
            Some(7),
            "the next write repairs it"
        );
        Ok(())
    }
}

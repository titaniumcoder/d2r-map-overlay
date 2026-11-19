//! When each character was last played, so the config page lists the recent ones first.
//!
//! `played.json` in the data dir maps the lowercased character name to its display name and
//! the unix time it was last seen in a game (and its class, when seen live). [`Tracker`] records
//! the live player each tick and merges that one entry into the file (via a temp file and a
//! rename), only when the character changed or at most every [`WRITE_INTERVAL`]. The config
//! page can [`remove`] a character; the tracker never writes back entries it only read, so a
//! removed character returns only when it is played again. Without the file (first start) it
//! is seeded from D2R's own save directory in the Proton prefix: online characters leave
//! `<Name><account digits>.ctlo` (key bindings, rewritten on leaving a game), offline ones
//! `<Name>.d2s`; their modification
//! times are the recency. A missing or corrupt file only means no order: the page then keeps
//! the config order.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant, UNIX_EPOCH};
use std::{fs, io};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Least time between two writes for the same character.
pub const WRITE_INTERVAL: Duration = Duration::from_secs(60);

/// Serialises every read-modify-write of the file: the live loop's [`Tracker`] and the web
/// thread's [`remove`] must not both start from the same old map.
static FILE_WRITE: Mutex<()> = Mutex::new(());

/// One character's last sighting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// The name as the game spells it.
    pub name: String,
    /// Unix seconds.
    pub t: u64,
    /// The class id (charstats row, see [`crate::config::CLASS_NAMES`]); `None` when only
    /// known from the save files or from a file written before the class was recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<u32>,
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
            class: None,
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

/// Removes `name` (any case) from the file at `path`; `Ok(false)` when it wasn't there (a
/// missing file stays missing, so the next start still seeds).
///
/// # Errors
/// Fails when the file can't be written.
pub fn remove(path: &Path, name: &str) -> Result<bool> {
    // A poisoned lock still guards the file; the map is re-read from disk anyway.
    let _guard = FILE_WRITE.lock().unwrap_or_else(PoisonError::into_inner);
    let Some(mut played) = read(path) else {
        return Ok(false);
    };
    let before = played.len();
    played.retain(|key, entry| !key.eq_ignore_ascii_case(name) && !entry.name.eq_ignore_ascii_case(name));
    if played.len() == before {
        return Ok(false);
    }
    write(path, &played)?;
    Ok(true)
}

/// The live player's last sighting and the file it is merged into, fed by the live loop.
///
/// Only the character seen in this game is held in memory; each save re-reads the file and
/// replaces just that entry, so a character [`remove`]d from the page meanwhile stays removed.
#[derive(Debug)]
pub struct Tracker {
    /// The lowercased key and entry of the character last recorded.
    current: Option<(String, Entry)>,
    /// `None`: no data dir, nothing is saved.
    path: Option<PathBuf>,
    /// The key and time of the last write.
    last_write: Option<(String, Instant)>,
    /// A write failed already: later failures are not logged again.
    failed: bool,
}

impl Tracker {
    /// Opens the tracker for `path`; if the file doesn't exist, takes `seed()` and writes it at
    /// once (so the seed runs only once).
    pub fn open(path: Option<PathBuf>, seed: impl FnOnce() -> Played) -> Self {
        let mut tracker = Self {
            current: None,
            path,
            last_write: None,
            failed: false,
        };
        let Some(path) = tracker.path.as_deref() else {
            return tracker;
        };
        let _guard = FILE_WRITE.lock().unwrap_or_else(PoisonError::into_inner);
        if read(path).is_none() {
            let seeded = seed();
            // An empty seed isn't written: the next start tries again (a `record` writes the
            // file anyway).
            if !seeded.is_empty() {
                let names: Vec<_> = seeded.values().map(|e| e.name.as_str()).collect();
                eprintln!("played: seeded from the save files: {names:?}");
                let result = write(path, &seeded);
                tracker.report(result);
            }
        }
        tracker
    }

    /// Notes `name` of class id `class` as seen at unix time `t` (ignored unless
    /// [`plausible_name`]); writes when the character differs from the last write's or
    /// [`WRITE_INTERVAL`] passed since it (`now`).
    pub fn record(&mut self, name: &str, class: Option<u32>, t: u64, now: Instant) {
        if !plausible_name(name) {
            return;
        }
        let key = name.to_lowercase();
        // A switch first saves the previous character's unsaved last sighting.
        if self.current.as_ref().is_some_and(|(k, _)| *k != key) {
            self.save();
        }
        self.current = Some((
            key.clone(),
            Entry {
                name: name.to_owned(),
                t,
                class,
            },
        ));
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

    /// Merges the current entry into the file as it is on disk now (a corrupt file is
    /// replaced by the entry alone).
    fn save(&mut self) {
        let (Some(path), Some((key, entry))) = (self.path.as_deref(), self.current.as_ref()) else {
            return;
        };
        let result = {
            let _guard = FILE_WRITE.lock().unwrap_or_else(PoisonError::into_inner);
            let mut played = read(path).unwrap_or_default();
            let class = entry.class.or_else(|| played.get(key).and_then(|old| old.class));
            played.insert(
                key.clone(),
                Entry {
                    class,
                    ..entry.clone()
                },
            );
            write(path, &played)
        };
        self.report(result);
    }

    /// Logs the first failed write only.
    fn report(&mut self, result: Result<()>) {
        if let Err(e) = result
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
            tracker.record(name, None, 1, Instant::now());
        }
        assert!(tracker.current.is_none());
        tracker.record("Blood-Boil_x", None, 1, Instant::now());
        assert!(tracker.current.is_some());
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
                    class: None,
                },
            )])
        };
        let mut tracker = Tracker::open(Some(path.clone()), seed);
        assert!(path.exists(), "the seed is written at once");
        tracker.record("Fulmina", Some(1), 9, Instant::now());
        let loaded = load(&path);
        assert_eq!(loaded.get("vex").map(|e| e.t), Some(5));
        assert_eq!(
            loaded.get("fulmina"),
            Some(&Entry {
                name: "Fulmina".to_owned(),
                t: 9,
                class: Some(1),
            })
        );
        // A second open reads the file, not the seed.
        let _again = Tracker::open(Some(path.clone()), || {
            Played::from([(
                "x".to_owned(),
                Entry {
                    name: "X".to_owned(),
                    t: 1,
                    class: None,
                },
            )])
        });
        assert_eq!(load(&path), loaded);
        Ok(())
    }

    #[test]
    fn writes_are_throttled_per_character() -> Result<()> {
        let dir = temp_dir("played-throttle")?;
        let path = dir.join("played.json");
        let mut tracker = Tracker::open(Some(path.clone()), Played::new);
        let t0 = Instant::now();
        tracker.record("Vex", None, 1, t0);
        tracker.record("Vex", None, 2, t0 + WRITE_INTERVAL / 2);
        assert_eq!(
            load(&path).get("vex").map(|e| e.t),
            Some(1),
            "same character waits"
        );
        tracker.record("Fulmina", None, 3, t0 + WRITE_INTERVAL / 2);
        assert_eq!(
            load(&path).get("vex").map(|e| e.t),
            Some(2),
            "a switch writes at once"
        );
        tracker.record("Fulmina", None, 4, t0 + WRITE_INTERVAL);
        assert_eq!(
            load(&path).get("fulmina").map(|e| e.t),
            Some(3),
            "within the interval"
        );
        tracker.record("Fulmina", None, 5, t0 + WRITE_INTERVAL * 2);
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
                    class: None,
                },
            )])
        });
        assert_eq!(
            fs::read_to_string(&path)?,
            "{not json",
            "a corrupt file is not a missing one"
        );
        tracker.record("Vex", None, 7, Instant::now());
        assert_eq!(
            load(&path).get("vex").map(|e| e.t),
            Some(7),
            "the next write repairs it"
        );
        Ok(())
    }

    #[test]
    fn a_removed_character_is_not_written_back_from_memory() -> Result<()> {
        let dir = temp_dir("played-remove")?;
        let path = dir.join("played.json");
        let mut tracker = Tracker::open(Some(path.clone()), Played::new);
        let t0 = Instant::now();
        tracker.record("Vex", Some(5), 1, t0);
        tracker.record("Fulmina", None, 2, t0);
        assert!(remove(&path, "VEX")?, "any case");
        assert!(!remove(&path, "vex")?, "already gone");
        // Fulmina is still played: its next save keeps Vex removed.
        tracker.record("Fulmina", None, 3, t0 + WRITE_INTERVAL);
        let played = load(&path);
        assert_eq!(played.keys().collect::<Vec<_>>(), ["fulmina"]);
        assert_eq!(played.get("fulmina").map(|e| e.t), Some(3));
        // Played again: back.
        tracker.record("Vex", None, 4, t0 + WRITE_INTERVAL);
        assert_eq!(
            load(&path).get("vex").map(|e| (e.t, e.class)),
            Some((4, None)),
            "a removed character's class is gone with it"
        );
        // Removing everything leaves an empty file, so the save-file seed doesn't return.
        remove(&path, "vex")?;
        remove(&path, "fulmina")?;
        assert!(load(&path).is_empty() && path.exists());
        assert!(!remove(&dir.join("missing.json"), "vex")?);
        Ok(())
    }

    #[test]
    fn the_class_is_kept_and_old_files_still_parse() -> Result<()> {
        let dir = temp_dir("played-class")?;
        fs::create_dir_all(&dir)?;
        let path = dir.join("played.json");
        fs::write(&path, r#"{"vex": {"name": "Vex", "t": 1}}"#)?;
        assert_eq!(load(&path).get("vex").map(|e| e.class), Some(None));
        let mut tracker = Tracker::open(Some(path.clone()), Played::new);
        tracker.record("Vex", Some(5), 2, Instant::now());
        assert_eq!(load(&path).get("vex").map(|e| e.class), Some(Some(5)));
        // A sighting without a class keeps the recorded one.
        tracker.record("Vex", None, 3, Instant::now() + WRITE_INTERVAL);
        assert_eq!(load(&path).get("vex").map(|e| (e.t, e.class)), Some((3, Some(5))));
        Ok(())
    }
}

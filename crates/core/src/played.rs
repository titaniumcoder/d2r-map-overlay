//! `played.json`: when each character was last played (so the config page lists recent ones
//! first) and the guide stage it last reached, which latches even while the stash is unread.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant, UNIX_EPOCH};
use std::{fs, io};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// Least time between two writes for the same character.
pub const WRITE_INTERVAL: Duration = Duration::from_secs(60);

/// Serialises every read-modify-write of the file (live loop and web thread).
static FILE_WRITE: Mutex<()> = Mutex::new(());

/// Locks [`FILE_WRITE`]; a poisoned lock still guards, since the map is re-read from disk.
fn lock_file() -> MutexGuard<'static, ()> {
    FILE_WRITE.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Bumped by every edit outside the tracker ([`remove`], [`clear_stage`]): a [`Tracker`] that
/// synced an older count re-reads its character's stage from the file.
static OUTSIDE_EDITS: AtomicU64 = AtomicU64::new(0);

/// One character's last sighting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Entry {
    /// The name as the game spells it.
    pub name: String,
    /// Unix seconds.
    pub t: u64,
    /// The class id (charstats row, see [`crate::config::CLASS_NAMES`]); `None` when only
    /// known from the save files.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<u32>,
    /// The slug of the guide stage the character last reached live ([`Tracker::reach`]);
    /// `None` before one was recorded.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stage: Option<String>,
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
#[must_use]
pub fn plausible_name(name: &str) -> bool {
    (2..=15).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphabetic() || b == b'-' || b == b'_')
}

/// Last-played times from the save files in the game's save directory `dir` (newest file per
/// character). Online characters leave `.ctlo` files, rewritten on leaving a game.
#[must_use]
pub fn seed_from_saves(dir: &Path) -> Played {
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
            stage: None,
        });
        slot.t = slot.t.max(t);
    }
    played
}

/// Removes `name` (any case) from the file at `path`; `Ok(false)` when it wasn't there (a
/// missing file stays missing, so the next start still seeds).
///
/// # Errors
/// Fails when the file can't be written.
pub fn remove(path: &Path, name: &str) -> Result<bool> {
    let _guard = lock_file();
    let Some(mut played) = read(path) else {
        return Ok(false);
    };
    let before = played.len();
    played.retain(|key, entry| !key.eq_ignore_ascii_case(name) && !entry.name.eq_ignore_ascii_case(name));
    if played.len() == before {
        return Ok(false);
    }
    write(path, &played)?;
    OUTSIDE_EDITS.fetch_add(1, Ordering::SeqCst);
    Ok(true)
}

/// Forgets the guide stage `name` (any case) reached, so the stage chain restarts from a new
/// build selection; `Ok(false)` when none was stored.
///
/// # Errors
/// Fails when the file can't be written.
pub fn clear_stage(path: &Path, name: &str) -> Result<bool> {
    let _guard = lock_file();
    let Some(mut played) = read(path) else {
        return Ok(false);
    };
    let mut cleared = false;
    for (key, entry) in &mut played {
        if key.eq_ignore_ascii_case(name) || entry.name.eq_ignore_ascii_case(name) {
            cleared |= entry.stage.take().is_some();
        }
    }
    if !cleared {
        return Ok(false);
    }
    write(path, &played)?;
    OUTSIDE_EDITS.fetch_add(1, Ordering::SeqCst);
    Ok(true)
}

/// The live player's last sighting and the file it is merged into, fed by the live loop.
/// Each save re-reads the file and replaces just that entry, so a [`remove`] sticks.
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
    /// The [`OUTSIDE_EDITS`] count the current entry's stage was read at.
    synced: u64,
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
            synced: 0,
        };
        let Some(path) = tracker.path.as_deref() else {
            return tracker;
        };
        let _guard = lock_file();
        if read(path).is_none() {
            let seeded = seed();
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
        if self.current.as_ref().is_some_and(|(k, _)| *k != key) {
            self.save();
        }
        let stage = self.take_stage(&key);
        self.current = Some((
            key.clone(),
            Entry {
                name: name.to_owned(),
                t,
                class,
                stage,
            },
        ));
        if self.wrote_recently(&key, now) {
            return;
        }
        self.last_write = Some((key, now));
        self.save();
    }

    /// The stage `key` reached: the in-memory one for the same character unless the file was
    /// edited outside since, else the file's.
    fn take_stage(&mut self, key: &str) -> Option<String> {
        let edits = OUTSIDE_EDITS.load(Ordering::SeqCst);
        let stage = match self.current.take() {
            Some((k, entry)) if k == key && (edits == self.synced || self.path.is_none()) => entry.stage,
            _ => self.stored_stage(key),
        };
        self.synced = edits;
        stage
    }

    fn wrote_recently(&self, key: &str, now: Instant) -> bool {
        self.last_write
            .as_ref()
            .is_some_and(|(k, at)| k == key && now.saturating_duration_since(*at) < WRITE_INTERVAL)
    }

    /// The guide stage `name` (any case) has reached, if recorded: what [`Tracker::record`]
    /// last read or [`Tracker::reach`] set for the current character.
    #[must_use]
    pub fn reached(&self, name: &str) -> Option<&str> {
        self.current
            .as_ref()
            .filter(|(key, _)| *key == name.to_lowercase())
            .and_then(|(_, entry)| entry.stage.as_deref())
    }

    /// Records that the current character `name` reached stage `slug` and saves at once when
    /// that is news. Ignored unless `name` is the character last [`Tracker::record`]ed.
    pub fn reach(&mut self, name: &str, slug: &str) {
        let Some((key, entry)) = self.current.as_mut() else {
            return;
        };
        if *key != name.to_lowercase() || entry.stage.as_deref() == Some(slug) {
            return;
        }
        entry.stage = Some(slug.to_owned());
        self.save_with(Some(slug));
    }

    /// `key`'s stage as stored in the file.
    fn stored_stage(&self, key: &str) -> Option<String> {
        let path = self.path.as_deref()?;
        let _guard = lock_file();
        read(path)?.remove(key)?.stage
    }

    /// Merges the current entry into the file as it is on disk now (a corrupt file is
    /// replaced by the entry alone), keeping the file's stage.
    fn save(&mut self) {
        self.save_with(None);
    }

    /// [`Tracker::save`], storing stage `reached` when given; otherwise the file's stage (the
    /// latch's source of truth) stays and becomes the in-memory one.
    fn save_with(&mut self, reached: Option<&str>) {
        let (Some(path), Some((key, entry))) = (self.path.as_deref(), self.current.as_mut()) else {
            return;
        };
        let result = {
            let _guard = lock_file();
            let mut played = read(path).unwrap_or_default();
            let old = played.get(key);
            let class = entry.class.or_else(|| old.and_then(|old| old.class));
            entry.stage = reached
                .map(str::to_owned)
                .or_else(|| old.and_then(|old| old.stage.clone()));
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
    fn seed_reads_the_save_files() -> Result<()> {
        let saves = temp_dir("played-saves")?;
        fs::create_dir_all(&saves)?;
        fs::write(saves.join("Vex223650631.ctlo"), "")?;
        fs::write(saves.join("Vex223650631.keyo"), "")?;
        fs::write(saves.join("Settings.json"), "")?;
        let played = seed_from_saves(&saves);
        assert_eq!(played.keys().collect::<Vec<_>>(), ["vex"]);
        assert_eq!(played.get("vex").map(|e| e.name.as_str()), Some("Vex"));
        assert!(played.get("vex").is_some_and(|e| e.t > 0));
        fs::remove_dir_all(&saves)?;
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
                    stage: None,
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
                stage: None,
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
                    stage: None,
                },
            )])
        });
        assert_eq!(load(&path), loaded);
        Ok(())
    }

    #[test]
    fn the_reached_stage_is_kept_and_survives_a_restart() -> Result<()> {
        let dir = temp_dir("played-stage")?;
        let path = dir.join("played.json");
        let t0 = Instant::now();
        let mut tracker = Tracker::open(Some(path.clone()), Played::new);
        tracker.record("Vex", Some(5), 1, t0);
        assert_eq!(tracker.reached("vex"), None);
        tracker.reach("Fulmina", "fury druid");
        assert_eq!(tracker.reached("Vex"), None, "only the current character");
        tracker.reach("Vex", "fury druid");
        assert_eq!(tracker.reached("VEX"), Some("fury druid"));
        assert_eq!(
            load(&path).get("vex").and_then(|e| e.stage.clone()).as_deref(),
            Some("fury druid"),
            "saved at once, without waiting for the write interval"
        );
        // Later sightings keep it, also across another character and a restart.
        tracker.record("Vex", Some(5), 2, t0 + WRITE_INTERVAL);
        tracker.record("Fulmina", None, 3, t0 + WRITE_INTERVAL);
        assert_eq!(tracker.reached("Fulmina"), None);
        let mut restarted = Tracker::open(Some(path.clone()), Played::new);
        restarted.record("Vex", None, 4, t0);
        assert_eq!(restarted.reached("Vex"), Some("fury druid"));
        assert_eq!(load(&path).get("vex").map(|e| (e.t, e.class)), Some((4, Some(5))));
        // The config page's delete drops it with the entry.
        assert!(remove(&path, "vex")?);
        let mut fresh = Tracker::open(Some(path.clone()), Played::new);
        fresh.record("Vex", None, 5, t0);
        assert_eq!(fresh.reached("Vex"), None);
        // An older file without the field still loads.
        fs::write(&path, r#"{"vex":{"name":"Vex","t":1}}"#)?;
        assert_eq!(load(&path).get("vex").map(|e| e.stage.clone()), Some(None));
        Ok(())
    }

    #[test]
    fn a_delete_resets_the_stage_of_the_running_tracker() -> Result<()> {
        let dir = temp_dir("played-stage-delete")?;
        let path = dir.join("played.json");
        let t0 = Instant::now();
        let mut tracker = Tracker::open(Some(path.clone()), Played::new);
        tracker.record("Vex", Some(5), 1, t0);
        tracker.reach("Vex", "fury druid");
        assert!(remove(&path, "Vex")?);
        // Still the same game: the next sighting re-reads the file and doesn't resurrect it.
        tracker.record("Vex", Some(5), 2, t0);
        assert_eq!(tracker.reached("Vex"), None);
        tracker.record("Vex", Some(5), 3, t0 + WRITE_INTERVAL);
        assert_eq!(
            load(&path).get("vex").map(|e| (e.t, e.stage.clone())),
            Some((3, None))
        );
        // A save never writes the in-memory copy back, even without a re-read in between.
        tracker.reach("Vex", "fury druid");
        fs::write(&path, r#"{"vex":{"name":"Vex","t":3}}"#)?;
        tracker.save();
        assert_eq!(load(&path).get("vex").map(|e| e.stage.clone()), Some(None));
        assert_eq!(tracker.reached("Vex"), None, "memory follows the file");
        Ok(())
    }

    #[test]
    fn a_new_selection_clears_the_stage() -> Result<()> {
        let dir = temp_dir("played-stage-clear")?;
        let path = dir.join("played.json");
        let t0 = Instant::now();
        let mut tracker = Tracker::open(Some(path.clone()), Played::new);
        tracker.record("Vex", Some(5), 1, t0);
        assert!(!clear_stage(&path, "vex")?, "nothing stored yet");
        tracker.reach("Vex", "fury druid");
        tracker.record("Fulmina", Some(1), 1, t0);
        tracker.reach("Fulmina", "nova");
        tracker.record("Vex", Some(5), 2, t0);
        assert!(clear_stage(&path, "VEX")?, "any case");
        let played = load(&path);
        assert_eq!(
            played.get("vex").map(|e| (e.t, e.stage.clone())),
            Some((2, None)),
            "entry kept"
        );
        assert_eq!(
            played.get("fulmina").and_then(|e| e.stage.as_deref()),
            Some("nova"),
            "other characters keep theirs"
        );
        tracker.record("Vex", Some(5), 3, t0);
        assert_eq!(tracker.reached("Vex"), None, "the running tracker sees it");
        // The chain restarts and records again.
        tracker.reach("Vex", "summoner");
        assert_eq!(
            load(&path).get("vex").and_then(|e| e.stage.clone()).as_deref(),
            Some("summoner")
        );
        assert!(!clear_stage(&dir.join("missing.json"), "vex")?);
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
                    stage: None,
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

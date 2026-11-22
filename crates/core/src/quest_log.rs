//! The quests each character finished, per difficulty, remembered in `quests.json`: the game
//! only exposes the current difficulty's flags. A merge only adds, so a done quest stays done.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::{fs, io};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

/// One character's finished quests per difficulty.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct QuestsDone {
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub normal: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub nightmare: BTreeSet<String>,
    #[serde(default, skip_serializing_if = "BTreeSet::is_empty")]
    pub hell: BTreeSet<String>,
}

impl QuestsDone {
    /// The quests done in `difficulty` (0 Normal, 1 Nightmare, 2 Hell); `None` beyond Hell.
    #[must_use]
    pub const fn get(&self, difficulty: u8) -> Option<&BTreeSet<String>> {
        match difficulty {
            0 => Some(&self.normal),
            1 => Some(&self.nightmare),
            2 => Some(&self.hell),
            _ => None,
        }
    }

    const fn get_mut(&mut self, difficulty: u8) -> Option<&mut BTreeSet<String>> {
        match difficulty {
            0 => Some(&mut self.normal),
            1 => Some(&mut self.nightmare),
            2 => Some(&mut self.hell),
            _ => None,
        }
    }

    /// Whether quest `name` is done in `difficulty` (case and apostrophes ignored).
    #[must_use]
    pub fn is_done(&self, difficulty: u8, name: &str) -> bool {
        self.get(difficulty)
            .is_some_and(|done| done.iter().any(|q| crate::names::same_name(q, name)))
    }

    /// Adds `other`'s quests; true if any was new.
    fn merge(&mut self, other: &Self) -> bool {
        let mut changed = false;
        for d in 0..3 {
            if let (Some(mine), Some(theirs)) = (self.get_mut(d), other.get(d)) {
                for quest in theirs {
                    changed |= mine.insert(quest.clone());
                }
            }
        }
        changed
    }
}

/// One character's entry in the file.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct Entry {
    name: String,
    #[serde(flatten)]
    done: QuestsDone,
}

type Log = BTreeMap<String, Entry>;

/// `quests.json` in the data dir ([`crate::journal::data_dir`]).
#[must_use]
pub fn default_path() -> Option<PathBuf> {
    crate::journal::data_dir().map(|d| d.join("quests.json"))
}

/// The file's map: empty when missing; `None` when it can't be read (logged), so nothing
/// overwrites it. A file that doesn't parse is moved aside to `quests.json.bad` (logged) and
/// read as empty: its history is kept for a look, never silently replaced.
fn read(path: &Path) -> Option<Log> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Some(Log::new()),
        Err(e) => {
            eprintln!("quests: reading {}: {e}", path.display());
            return None;
        }
    };
    match serde_json::from_str(&text) {
        Ok(log) => Some(log),
        Err(e) => {
            let bad = path.with_extension("json.bad");
            match fs::rename(path, &bad) {
                Ok(()) => {
                    eprintln!(
                        "quests: {} is corrupt ({e}); moved to {}",
                        path.display(),
                        bad.display()
                    );
                    Some(Log::new())
                }
                Err(re) => {
                    eprintln!(
                        "quests: {} is corrupt ({e}) and can't be moved aside ({re}); not written",
                        path.display()
                    );
                    None
                }
            }
        }
    }
}

fn write(path: &Path, log: &Log) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let json = serde_json::to_string_pretty(log).context("serializing the quest log")?;
    crate::web::write_atomic(path, &json).with_context(|| format!("writing {}", path.display()))
}

/// Consecutive observations of the same name and difficulty before they count: a name
/// half-read during loading, or a tick where the difficulty and the record disagree at a
/// difficulty change, never reach the file.
pub const STABLE_TICKS: u32 = 3;

/// The live character's quest memory and the file it is merged into, fed by the live loop.
#[derive(Debug)]
pub struct Memory {
    /// `None`: no data dir, the memory lasts as long as the process.
    path: Option<PathBuf>,
    /// The lowercased key and difficulty last observed, and how many times in a row.
    streak: Option<(String, Option<u8>, u32)>,
    /// The lowercased key and quests of the character in use (stable, [`STABLE_TICKS`]).
    current: Option<(String, QuestsDone)>,
    /// The difficulty a quest record was seen in for the current character (reset on a
    /// character or difficulty change): until then its quests are unknown.
    record_seen: Option<u8>,
    /// A write failed already: later failures are not logged again.
    failed: bool,
}

impl Memory {
    #[must_use]
    pub const fn open(path: Option<PathBuf>) -> Self {
        Self {
            path,
            streak: None,
            current: None,
            record_seen: None,
            failed: false,
        }
    }

    /// Notes the quest record `words` of character `name` in the current game's `difficulty`
    /// and returns everything remembered for that character. `None` while unknown: an
    /// implausible name, not yet [`STABLE_TICKS`] stable, or no record read yet in this
    /// difficulty for this character.
    pub fn observe(&mut self, name: &str, difficulty: Option<u8>, words: &[u16]) -> Option<&QuestsDone> {
        if !crate::played::plausible_name(name) {
            self.streak = None;
            return None;
        }
        let key = name.to_lowercase();
        let count = self.bump_streak(&key, difficulty);
        let difficulty = difficulty?;
        if count < STABLE_TICKS {
            return None;
        }
        if self.current.as_ref().is_none_or(|(k, _)| *k != key) {
            self.switch_to(key);
        }
        if self.record_seen.is_some_and(|d| d != difficulty) {
            self.record_seen = None;
        }
        if !words.is_empty() {
            self.note_record(name, difficulty, words);
        }
        self.record_seen?;
        self.current.as_ref().map(|(_, done)| done)
    }

    /// Counts `key` and `difficulty` seen once more in a row; returns the run length.
    fn bump_streak(&mut self, key: &str, difficulty: Option<u8>) -> u32 {
        let count = match self.streak.take() {
            Some((k, d, n)) if k == key && d == difficulty => n.saturating_add(1),
            _ => 1,
        };
        self.streak = Some((key.to_owned(), difficulty, count));
        count
    }

    /// Makes `key` the current character, with the quests the file remembers for it.
    fn switch_to(&mut self, key: String) {
        let stored = self
            .path
            .as_deref()
            .and_then(read)
            .and_then(|mut log| log.remove(&key))
            .map_or_else(QuestsDone::default, |e| e.done);
        self.current = Some((key, stored));
        self.record_seen = None;
    }

    /// Merges the quests `words` shows done in `difficulty`, saving when any is new.
    fn note_record(&mut self, name: &str, difficulty: u8, words: &[u16]) {
        self.record_seen = Some(difficulty);
        let mut seen = QuestsDone::default();
        if let Some(set) = seen.get_mut(difficulty) {
            set.extend(crate::quests::done(words).map(str::to_owned));
        }
        if self.current.as_mut().is_some_and(|(_, done)| done.merge(&seen)) {
            self.save(name);
        }
    }

    /// Merges the current character's quests into the file as it is on disk now (what the
    /// file holds beyond them is kept: a merge only adds). An unreadable file is left alone.
    fn save(&mut self, name: &str) {
        let (Some(path), Some((key, done))) = (self.path.as_deref(), self.current.as_mut()) else {
            return;
        };
        let Some(mut log) = read(path) else {
            return;
        };
        let entry = log.entry(key.clone()).or_default();
        name.clone_into(&mut entry.name);
        entry.done.merge(done);
        done.merge(&entry.done);
        if let Err(e) = write(path, &log)
            && !std::mem::replace(&mut self.failed, true)
        {
            eprintln!("quests: {e:#} (not saved; further errors not shown)");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::temp_dir;

    /// A record with only Den of Evil (word 1) done.
    fn den_done() -> Vec<u16> {
        let mut words = vec![0u16; 48];
        if let Some(w) = words.get_mut(1) {
            *w = 0x1001;
        }
        words
    }

    /// `observe` [`STABLE_TICKS`] times; the last result, cloned.
    fn settle(memory: &mut Memory, name: &str, difficulty: Option<u8>, words: &[u16]) -> Option<QuestsDone> {
        for _ in 1..STABLE_TICKS {
            memory.observe(name, difficulty, words);
        }
        memory.observe(name, difficulty, words).cloned()
    }

    #[test]
    fn normal_stays_remembered_while_playing_nightmare() -> Result<()> {
        let dir = temp_dir("quest-log")?;
        let path = dir.join("quests.json");
        let mut memory = Memory::open(Some(path.clone()));
        let done = settle(&mut memory, "Wolfie", Some(0), &den_done()).context("normal")?;
        assert!(done.is_done(0, "Den of Evil"));
        // Nightmare: the game shows nothing done there; Normal's stay.
        let done = settle(&mut memory, "Wolfie", Some(1), &[0; 48]).context("nightmare")?;
        assert!(done.is_done(0, "Den of Evil"));
        assert!(!done.is_done(1, "Den of Evil"));
        // A restart reads it back from the file, and another character has its own.
        let mut restarted = Memory::open(Some(path.clone()));
        let other = settle(&mut restarted, "Other", Some(1), &[0; 48]).context("other")?;
        assert!(other.normal.is_empty());
        let done = settle(&mut restarted, "WOLFIE", Some(1), &[0; 48]).context("restart")?;
        assert!(done.is_done(0, "den of evil"));
        let text = fs::read_to_string(&path)?;
        assert!(text.contains("\"name\": \"Wolfie\""), "{text}");
        fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[test]
    fn unknown_until_a_record_is_read() {
        let mut memory = Memory::open(None);
        assert_eq!(settle(&mut memory, "Wolfie", Some(0), &[]), None, "unread record");
        assert_eq!(
            settle(&mut memory, "Wolfie", None, &den_done()),
            None,
            "unknown difficulty"
        );
        assert!(settle(&mut memory, "Wolfie", Some(0), &den_done()).is_some());
        // Still known while the record is dropped (new game) in the same difficulty ...
        assert!(memory.observe("Wolfie", Some(0), &[]).is_some());
        // ... but not after a difficulty change, until that difficulty's record is read.
        assert_eq!(settle(&mut memory, "Wolfie", Some(1), &[]), None);
        assert!(settle(&mut memory, "Wolfie", Some(1), &[0; 48]).is_some());
        assert_eq!(memory.observe("X", Some(1), &[0; 48]), None, "implausible name");
    }

    #[test]
    fn a_one_tick_half_read_name_creates_no_entry() -> Result<()> {
        let dir = temp_dir("quest-log-flicker")?;
        let path = dir.join("quests.json");
        let mut memory = Memory::open(Some(path.clone()));
        assert_eq!(memory.observe("Wolf", Some(0), &den_done()), None);
        settle(&mut memory, "Wolfie", Some(0), &den_done()).context("stable")?;
        // A one-tick difficulty mismatch is not attributed either.
        assert_eq!(memory.observe("Wolfie", Some(1), &den_done()), None);
        let text = fs::read_to_string(&path)?;
        assert!(!text.contains("\"wolf\""), "{text}");
        assert!(!text.contains("nightmare"), "{text}");
        fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[test]
    fn a_corrupt_file_is_moved_aside() -> Result<()> {
        let dir = temp_dir("quest-log-corrupt")?;
        fs::create_dir_all(&dir)?;
        let path = dir.join("quests.json");
        fs::write(&path, "{ not json")?;
        let mut memory = Memory::open(Some(path.clone()));
        settle(&mut memory, "Wolfie", Some(0), &den_done()).context("stable")?;
        assert_eq!(fs::read_to_string(dir.join("quests.json.bad"))?, "{ not json");
        assert!(fs::read_to_string(&path)?.contains("Den of Evil"));
        fs::remove_dir_all(dir)?;
        Ok(())
    }

    #[test]
    fn without_a_file_the_memory_lasts_the_process() -> Result<()> {
        let mut memory = Memory::open(None);
        settle(&mut memory, "Wolfie", Some(2), &den_done()).context("hell")?;
        assert!(
            memory
                .observe("Wolfie", Some(2), &[])
                .is_some_and(|d| d.is_done(2, "Den of Evil"))
        );
        // Another character starts empty (and the first one is gone: no file).
        assert_eq!(
            settle(&mut memory, "Other", Some(0), &[0; 48]),
            Some(QuestsDone::default())
        );
        Ok(())
    }
}

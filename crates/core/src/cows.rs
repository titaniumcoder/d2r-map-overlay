//! Cow counter for the Moo Moo Farm (level [`COW_LEVEL`]), a fun request.
//!
//! **This run** ([`Tally`]): [`crate::session::Session`] counts each cow once by unit id, the
//! first time it is seen dead in level 39, and keeps the ids in its `GameState` (so an overlay
//! restart within a game keeps the count; a new game starts at 0). In level 39 the info line
//! then reads `Normal · Moo Moo Farm · kill count: 57 / victims alive: 23`.
//!
//! **Lifetime** ([`Stats`]): total, best run and runs (games in which the player entered level
//! 39), in `cows.json` in the data dir. Not shown for now. [`Counter`] loads it, folds each
//! snapshot's [`Update`] in and writes it via a temp file and a rename, at most every
//! [`SAVE_INTERVAL`] and only when it changed. Without the file (first start) it is seeded from
//! the journal: the journal only logged elite monsters, so `runs` is the number of distinct
//! games with a level-39 event, and total and best only count the elite cows logged killed.
//!
//! Known gap: kills in the last few seconds before an overlay crash may be in `cows.json` but
//! not yet in `game.json`, and are then counted again after the restart.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Instant;

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};

pub use crate::game_file::SAVE_INTERVAL;
use crate::session::Update;

/// The Moo Moo Farm.
pub const COW_LEVEL: u32 = 39;

/// monstats.txt rows of cows: `hellbovine` (391), and 735, the Cow King's live unit (journal,
/// two games: superunique + unique flags at the Cow King's preset in level 39).
const COW_TXTS: [u32; 2] = [391, 735];

#[must_use]
pub fn is_cow(txt: u32) -> bool {
    COW_TXTS.contains(&txt)
}

/// This run's counter, as drawn in the info line.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Tally {
    /// Cows counted dead in level 39 in this game.
    pub kills: u64,
    /// Living cows in the snapshot.
    pub alive: u32,
}

/// The fields of a journal line the seed needs (others are ignored).
#[derive(Debug, Deserialize)]
struct JournalLine {
    game: String,
    ev: String,
    level: Option<u32>,
    txt: Option<u32>,
    id: Option<u32>,
}

/// The lifetime stats in `cows.json`.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Stats {
    /// Cows killed over all runs.
    #[serde(default)]
    pub total: u64,
    /// Most cows killed in one run.
    #[serde(default)]
    pub best: u64,
    /// Games in which the player entered level 39.
    #[serde(default)]
    pub runs: u64,
}

impl Stats {
    /// Folds in one snapshot's `update`; `run` is this game's kill count after it. Returns
    /// whether anything changed.
    pub fn record(&mut self, update: &Update, run: u64) -> bool {
        let before = *self;
        if update.cow_run {
            self.runs += 1;
        }
        self.total += u64::from(update.new_cows);
        self.best = self.best.max(run);
        *self != before
    }

    /// Stats seeded from journal lines (JSON, see [`crate::journal`]): runs = distinct `game`
    /// values with an event in level 39; total and best from the cows logged `killed` there
    /// (elites only, each unit id once per game). Unparsable lines are skipped.
    pub fn seed<'a>(lines: impl IntoIterator<Item = &'a str>) -> Self {
        let mut games: HashSet<String> = HashSet::new();
        let mut killed: HashMap<String, HashSet<u32>> = HashMap::new();
        for line in lines {
            let Ok(ev) = serde_json::from_str::<JournalLine>(line) else {
                continue;
            };
            if ev.level != Some(COW_LEVEL) {
                continue;
            }
            if ev.ev == "killed"
                && ev.txt.is_some_and(is_cow)
                && let Some(id) = ev.id
            {
                killed.entry(ev.game.clone()).or_default().insert(id);
            }
            games.insert(ev.game);
        }
        let count = |ids: &HashSet<u32>| u64::try_from(ids.len()).unwrap_or(u64::MAX);
        Self {
            total: killed.values().map(count).sum(),
            best: killed.values().map(count).max().unwrap_or(0),
            runs: u64::try_from(games.len()).unwrap_or(u64::MAX),
        }
    }
}

/// `cows.json` in the data dir ([`crate::journal::data_dir`]).
#[must_use]
pub fn default_path() -> Option<PathBuf> {
    crate::journal::data_dir().map(|d| d.join("cows.json"))
}

/// [`Stats::seed`] over every `*.jsonl` file in `dir`; unreadable files are skipped.
fn seed_from_dir(dir: &Path) -> Stats {
    let Ok(entries) = fs::read_dir(dir) else {
        return Stats::default();
    };
    let texts: Vec<String> = entries
        .filter_map(Result::ok)
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|x| x == "jsonl"))
        .filter_map(|p| fs::read_to_string(p).ok())
        .collect();
    Stats::seed(texts.iter().flat_map(|t| t.lines()))
}

/// Writes `stats` to `path` through a temp file next to it.
fn write(path: &Path, stats: &Stats) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let tmp = path.with_extension("json.tmp");
    let json = serde_json::to_vec_pretty(stats).context("serializing the cow stats")?;
    fs::write(&tmp, json).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("renaming {} to {}", tmp.display(), path.display()))
}

/// The lifetime stats and their file.
#[derive(Debug)]
pub struct Counter {
    pub stats: Stats,
    /// `None`: no data dir, nothing is saved.
    path: Option<PathBuf>,
    /// Changed since the last write.
    dirty: bool,
    last_write: Option<Instant>,
    /// A write failed already: later failures are not logged again.
    failed: bool,
}

impl Counter {
    /// Loads `path`; if it doesn't exist, seeds the stats from the journal in `journal_dir` and
    /// writes them at once (so the seed runs only once). A file that can't be read or parsed
    /// is logged and the stats start at 0.
    pub fn open(path: Option<PathBuf>, journal_dir: Option<&Path>) -> Self {
        let mut counter = Self {
            stats: Stats::default(),
            path,
            dirty: false,
            last_write: None,
            failed: false,
        };
        let Some(path) = counter.path.as_deref() else {
            return counter;
        };
        match fs::read_to_string(path) {
            Ok(text) => match serde_json::from_str(&text) {
                Ok(stats) => counter.stats = stats,
                Err(e) => eprintln!("cows: parsing {} (starting at 0): {e}", path.display()),
            },
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                counter.stats = journal_dir.map_or_else(Stats::default, seed_from_dir);
                eprintln!("cows: seeded {:?} from the journal", counter.stats);
                counter.dirty = true;
                counter.flush();
            }
            Err(e) => eprintln!("cows: reading {}: {e}", path.display()),
        }
        counter
    }

    /// Folds in one snapshot's `update`; `run` is this game's kill count after it.
    pub fn record(&mut self, update: &Update, run: u64) {
        self.dirty |= self.stats.record(update, run);
    }

    /// Writes the stats if they changed and the last write was at least [`SAVE_INTERVAL`]
    /// before `now`.
    pub fn tick(&mut self, now: Instant) {
        if self
            .last_write
            .is_some_and(|t| now.saturating_duration_since(t) < SAVE_INTERVAL)
        {
            return;
        }
        self.last_write = Some(now);
        self.flush();
    }

    /// Writes the stats now if they changed.
    pub fn flush(&mut self) {
        let Some(path) = self.path.as_deref().filter(|_| self.dirty) else {
            return;
        };
        self.dirty = false;
        if let Err(e) = write(path, &self.stats)
            && !std::mem::replace(&mut self.failed, true)
        {
            eprintln!("cows: {e:#} (not saved; further errors not shown)");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::temp_dir;

    fn update(cow_run: bool, new_cows: u32) -> Update {
        Update {
            cow_run,
            new_cows,
            ..Update::default()
        }
    }

    #[test]
    fn best_and_total_follow_the_kills() {
        let mut s = Stats {
            total: 10,
            best: 4,
            runs: 2,
        };
        assert!(s.record(&update(true, 0), 0), "a new run");
        assert_eq!(s.runs, 3);
        assert!(s.record(&update(false, 3), 3));
        assert_eq!((s.total, s.best), (13, 4), "3 is below the best");
        assert!(s.record(&update(false, 2), 5));
        assert_eq!((s.total, s.best), (15, 5), "a new best");
        assert!(!s.record(&update(false, 0), 5), "nothing new");
    }

    #[test]
    fn seed_counts_games_in_level_39_and_logged_cow_kills() {
        let journal = [
            r#"{"t":1,"game":"a","ev":"arrival","level":39,"x":1,"y":1,"piece":null}"#,
            r#"{"t":2,"game":"a","ev":"killed","level":39,"id":7,"txt":391,"x":1,"y":1,"piece":0}"#,
            r#"{"t":3,"game":"a","ev":"killed","level":39,"id":7,"txt":391,"x":1,"y":1,"piece":0}"#,
            r#"{"t":4,"game":"a","ev":"killed","level":39,"id":8,"txt":391,"x":1,"y":1,"piece":0}"#,
            r#"{"t":5,"game":"b","ev":"killed","level":39,"id":9,"txt":410,"x":1,"y":1,"piece":0}"#,
            r#"{"t":6,"game":"c","ev":"killed","level":39,"id":9,"txt":391,"x":1,"y":1,"piece":0}"#,
            r#"{"t":7,"game":"d","ev":"arrival","level":4,"x":1,"y":1,"piece":null}"#,
            "not json",
        ];
        assert_eq!(
            Stats::seed(journal),
            Stats {
                total: 3,
                best: 2,
                runs: 3
            },
            "a trap (410) is no cow; game d never was in 39"
        );
        assert_eq!(Stats::seed([]), Stats::default());
    }

    #[test]
    fn a_missing_file_is_seeded_once_and_written() -> Result<()> {
        let dir = temp_dir("cows-seed")?;
        let journal = dir.join("journal");
        fs::create_dir_all(&journal)?;
        fs::write(
            journal.join("2026-11-14.jsonl"),
            "{\"t\":1,\"game\":\"a\",\"ev\":\"arrival\",\"level\":39,\"x\":1,\"y\":1}\n\
             {\"t\":2,\"game\":\"b\",\"ev\":\"arrival\",\"level\":39,\"x\":1,\"y\":1}\n",
        )?;
        let path = dir.join("cows.json");
        let mut counter = Counter::open(Some(path.clone()), Some(&journal));
        assert_eq!(counter.stats.runs, 2);
        assert!(path.exists(), "the seed is written at once");

        // A second open reads the file, not the journal.
        fs::write(
            journal.join("2026-11-15.jsonl"),
            "{\"t\":3,\"game\":\"c\",\"ev\":\"arrival\",\"level\":39,\"x\":1,\"y\":1}\n",
        )?;
        counter.record(&update(true, 4), 4);
        counter.flush();
        let again = Counter::open(Some(path), Some(&journal));
        assert_eq!(
            again.stats,
            Stats {
                total: 4,
                best: 4,
                runs: 3
            }
        );
        Ok(())
    }

    #[test]
    fn writes_are_throttled() -> Result<()> {
        let dir = temp_dir("cows-throttle")?;
        let path = dir.join("cows.json");
        let mut counter = Counter::open(Some(path.clone()), None);
        let t0 = Instant::now();
        counter.record(&update(true, 1), 1);
        counter.tick(t0);
        counter.record(&update(false, 1), 2);
        counter.tick(t0 + SAVE_INTERVAL / 2);
        let saved: Stats = serde_json::from_str(&fs::read_to_string(&path)?)?;
        assert_eq!(saved.total, 1, "second change waits");
        counter.tick(t0 + SAVE_INTERVAL);
        let saved: Stats = serde_json::from_str(&fs::read_to_string(&path)?)?;
        assert_eq!(saved.total, 2);
        Ok(())
    }
}

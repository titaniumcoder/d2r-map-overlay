//! The current game's [`GameState`] on disk (the filesystem side of [`Session`]'s memory), so an
//! overlay restart within a game keeps what was explored, killed and operated.
//!
//! One file, `game.json` in the cache dir, holds the latest game only (tagged with its player
//! id inside the state). It is written at most every [`SAVE_INTERVAL`] when the state changed,
//! via a temp file and a rename so a crash mid-write never leaves half a file. On start the
//! saved state is offered to the [`Session`], which uses it only if the first snapshot's player
//! id matches. Errors are logged once and otherwise ignored: the overlay works without the file.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use std::{fs, io};

use anyhow::{Context, Result};

use crate::offset_cache;
use crate::session::{GameState, Session};

/// Least time between two writes.
pub const SAVE_INTERVAL: Duration = Duration::from_secs(3);

/// `game.json` in the cache dir ([`offset_cache::cache_dir`]).
#[must_use]
pub fn default_path() -> Option<PathBuf> {
    offset_cache::cache_dir().map(|d| d.join("game.json"))
}

/// The saved state; `None` if there is none or it can't be read (logged, except a missing file).
#[must_use]
pub fn load(path: &Path) -> Option<GameState> {
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return None,
        Err(e) => {
            eprintln!("game state: reading {}: {e}", path.display());
            return None;
        }
    };
    serde_json::from_str(&text)
        .map_err(|e| eprintln!("game state: parsing {} (ignored): {e}", path.display()))
        .ok()
}

/// Writes `state` to `path` through a temp file next to it.
fn write(path: &Path, state: &GameState) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    let tmp = path.with_extension("json.tmp");
    let json = serde_json::to_vec(state).context("serializing the game state")?;
    fs::write(&tmp, json).with_context(|| format!("writing {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("renaming {} to {}", tmp.display(), path.display()))
}

/// Writes a [`Session`]'s state when it changed, throttled to [`SAVE_INTERVAL`].
#[derive(Debug)]
pub struct Saver {
    /// `None`: no cache dir, nothing is saved.
    path: Option<PathBuf>,
    last_write: Option<Instant>,
    /// A write failed already: later failures are not logged again.
    failed: bool,
}

impl Saver {
    pub const fn new(path: Option<PathBuf>) -> Self {
        Self {
            path,
            last_write: None,
            failed: false,
        }
    }

    /// Writes `session`'s state if it changed and the last write was at least
    /// [`SAVE_INTERVAL`] before `now`; a change seen earlier waits for a later call. Returns
    /// whether it wrote (or tried to).
    pub fn tick(&mut self, session: &mut Session, now: Instant) -> bool {
        let Some(path) = &self.path else { return false };
        if self
            .last_write
            .is_some_and(|t| now.saturating_duration_since(t) < SAVE_INTERVAL)
        {
            return false;
        }
        let Some(state) = session.take_changed() else {
            return false;
        };
        self.last_write = Some(now);
        if let Err(e) = write(path, state)
            && !std::mem::replace(&mut self.failed, true)
        {
            eprintln!("game state: {e:#} (not saved; further errors not shown)");
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::{Addr, Level, Player, Preset, PresetKind, Snapshot};
    use crate::testutil::temp_dir;

    fn snap(player_id: u32, level: u32, presets: Vec<Preset>) -> Snapshot {
        Snapshot {
            player: Player {
                addr: Addr(0x1000),
                id: player_id,
                name: "P".into(),
                class: 0,
                x: 30,
                y: 30,
                level_id: level,
                act: Some(Addr(0xa000)),
                char_level: None,
                build: vec![],
            },
            player_candidates: vec![],
            player_chosen_by: crate::game::ChosenBy::FirstCandidate,
            level: Level {
                id: level,
                bounds: None,
                rooms: vec![],
            },
            presets,
            room_links: vec![],
            border_links: vec![],
            monsters: vec![],
            ground_items: 0,
            drops: vec![],
            shrines: vec![],
            wells: vec![],
            operated_objects: vec![],
            inventory_items: 0,
            inventory_keep: vec![],
            sell: vec![],
            give_away: vec![],
            difficulty: None,
            terrorized: false,
            cows: None,
            trail: vec![],
            warnings: vec![],
        }
    }

    fn tile(txt: u32, x: u32) -> Preset {
        Preset {
            kind: PresetKind::Tile,
            txt,
            x,
            y: 100,
            entry: false,
        }
    }

    #[test]
    fn saved_state_restores_the_same_game_only() -> Result<()> {
        let dir = temp_dir("game-state")?;
        let path = dir.join("sub").join("game.json");
        assert_eq!(load(&path), None, "missing file");

        let mut s = Session::default();
        s.apply(&mut snap(0xdead_beef, 2, vec![tile(1, 100), tile(2, 200)]));
        let mut saver = Saver::new(Some(path.clone()));
        assert!(saver.tick(&mut s, Instant::now()));
        assert!(!path.with_extension("json.tmp").exists(), "temp file renamed");

        let state = load(&path).context("saved state")?;
        assert_eq!(state.player_id, 0xdead_beef);
        // A new overlay, same game: level 2 shows its tiles with nothing loaded.
        let mut s = Session::default();
        s.restore(state.clone());
        let mut same = snap(0xdead_beef, 2, vec![]);
        s.apply(&mut same);
        assert_eq!(same.presets, vec![tile(1, 100), tile(2, 200)]);
        // Another game: the saved state is dropped.
        let mut s = Session::default();
        s.restore(state);
        let mut other = snap(7, 2, vec![]);
        s.apply(&mut other);
        assert_eq!(other.presets, vec![]);

        fs::write(&path, "{ not json")?;
        assert_eq!(load(&path), None, "corrupt file ignored");
        fs::remove_dir_all(&dir)?;
        Ok(())
    }

    #[test]
    fn writes_are_throttled_and_only_on_change() -> Result<()> {
        let dir = temp_dir("game-state-throttle")?;
        let path = dir.join("game.json");
        let mut saver = Saver::new(Some(path.clone()));
        let mut s = Session::default();
        let t0 = Instant::now();
        assert!(!saver.tick(&mut s, t0), "no snapshot yet");
        s.apply(&mut snap(1, 2, vec![tile(1, 100)]));
        assert!(saver.tick(&mut s, t0));
        s.apply(&mut snap(1, 2, vec![tile(1, 100)]));
        assert!(!saver.tick(&mut s, t0 + SAVE_INTERVAL * 2), "unchanged: no write");

        let t1 = t0 + SAVE_INTERVAL * 2;
        s.apply(&mut snap(1, 2, vec![tile(2, 200)]));
        assert!(saver.tick(&mut s, t1));
        s.apply(&mut snap(1, 2, vec![tile(3, 300)]));
        assert!(!saver.tick(&mut s, t1 + Duration::from_secs(1)), "too soon");
        assert_eq!(load(&path).map(|g| g.player_id), Some(1));
        assert!(
            saver.tick(&mut s, t1 + SAVE_INTERVAL),
            "the pending change is written later"
        );
        let mut fresh = Session::default();
        fresh.restore(load(&path).context("saved")?);
        let mut back = snap(1, 2, vec![]);
        fresh.apply(&mut back);
        assert_eq!(back.presets.len(), 3);

        assert!(!Saver::new(None).tick(&mut s, t1), "no cache dir: never writes");
        fs::remove_dir_all(&dir)?;
        Ok(())
    }
}

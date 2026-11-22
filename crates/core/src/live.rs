//! Live mode reader thread: follows the game process, snapshots it ~5 times a second and sends
//! rendered frames to the UI loop. The overlay is hidden in towns and outside a game.

use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use smithay_client_toolkit::reexports::calloop::channel::Sender;

use crate::game::{self, Snapshot};
use crate::guidance::Guidance;
use crate::journal::Journal;
use crate::overlay::Msg;
use crate::playstyle::{EvaluationOf, Files, Playstyle};
use crate::reader::{Attach, GameReader};
use crate::session::Session;
use ab_glyph::FontVec;

use crate::{config, cows, game_file, info, journal, played, quest_log, render, web};

const TICK: Duration = Duration::from_millis(200);
const RETRY: Duration = Duration::from_secs(2);

#[derive(Debug)]
struct UiGone;

fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn send(tx: &Sender<Msg>, msg: Msg) -> Result<(), UiGone> {
    tx.send(msg).map_err(|_| UiGone)
}

/// Runs until the UI is gone. `player` picks among several player units; `files` are the
/// playstyle's, hot-reloaded when they change on disk.
#[allow(
    clippy::too_many_arguments,
    reason = "plain pass-through of main's settings; a struct for one caller adds nothing"
)]
pub fn run<P: Playstyle + 'static>(
    reader: &mut impl GameReader,
    playstyle: &mut P,
    tx: &Sender<Msg>,
    player: Option<&str>,
    size: (u32, u32),
    mut journal: Journal,
    config: &config::Config,
    files: &Files,
) {
    let web_files = files.clone();
    thread::spawn(move || web::serve::<P>(&web_files));
    let config_path = config::default_path();
    let mut watched = Watched::now(config_path.as_deref(), files);
    let mut config = config.clone();
    loop {
        if check_reload(&mut config, config_path.as_deref(), files, &mut watched) {
            playstyle.reload(files);
        }
        let (pid, follow) = match reader.attach() {
            Attach::Waiting => {
                thread::sleep(RETRY);
                continue;
            }
            Attach::Ready { pid } => (
                pid,
                follow_game(
                    reader,
                    playstyle,
                    tx,
                    player,
                    size,
                    &mut journal,
                    &mut config,
                    config_path.as_deref(),
                    files,
                ),
            ),
            Attach::Exited { pid } => (pid, Ok(())),
        };
        // `follow_game` reloads on its own; resync so the same change isn't reloaded twice.
        watched = Watched::now(config_path.as_deref(), files);
        web::clear_live();
        if follow.and_then(|()| send(tx, Msg::Hide)).is_err() {
            return;
        }
        eprintln!("D2R (pid {pid}) exited");
    }
}

fn modified_at(path: &std::path::Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// What the hot reload compares each tick. Per-file mtimes, since a directory's own mtime
/// misses in-place edits of a build file.
#[derive(Debug, Default, PartialEq)]
struct Watched {
    config: Option<SystemTime>,
    playstyle: Vec<(PathBuf, Option<SystemTime>)>,
}

impl Watched {
    fn now(config_path: Option<&Path>, files: &Files) -> Self {
        Self {
            config: config_path.and_then(modified_at),
            playstyle: files.fingerprint(),
        }
    }
}

/// Reloads `config` when `config.toml` changed on disk. True when it or a playstyle file
/// changed: the caller then reloads the playstyle. The new state is recorded even when the
/// reload fails, so a broken file is reported once per change.
fn check_reload(
    config: &mut config::Config,
    config_path: Option<&Path>,
    files: &Files,
    watched: &mut Watched,
) -> bool {
    let now = Watched::now(config_path, files);
    if *watched == now {
        return false;
    }
    let config_changed = watched.config != now.config;
    *watched = now;
    if let Some(path) = config_path.filter(|_| config_changed) {
        reload_config(config, path);
    }
    true
}

/// A file that can't be read or parsed keeps the current config.
fn reload_config(config: &mut config::Config, path: &Path) {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) => {
            eprintln!("config {}: {e}; keeping the current one", path.display());
            return;
        }
    };
    match config::parse(&text) {
        Ok((loaded, warnings)) => {
            for w in warnings {
                eprintln!("config {}: {w}", path.display());
            }
            if loaded.playstyle != config.playstyle {
                eprintln!(
                    "config: playstyle {:?} takes effect after a restart (running {:?})",
                    loaded.playstyle, config.playstyle
                );
            }
            eprintln!("config reloaded from {}", path.display());
            *config = loaded;
        }
        Err(e) => {
            eprintln!(
                "config {}: invalid, keeping the current one:\n{e}",
                path.display()
            );
        }
    }
}

/// Publishes the dashboard JSON and the gear state the config page annotates from.
fn publish_web<'a, P: Playstyle>(
    snap: &game::Snapshot,
    playstyle: &'a P,
    evaluation: &'a EvaluationOf<'_, P>,
    width: u32,
    quests: &[u16],
) {
    let blocks = render::text_blocks(snap, &evaluation.guidance, width, info::font());
    web::publish_gear(web::GearInfo {
        worn: game::worn_names(&snap.equipped_items),
        owned: snap.owned_names.clone(),
        stash: snap.stash_names.clone(),
        equipped: snap.equipped.clone(),
        merc_equipped: snap.merc_equipped.clone(),
    });
    publish_dashboard(snap, playstyle, evaluation, &blocks, quests);
}

fn publish_dashboard<'a, P: Playstyle>(
    snap: &game::Snapshot,
    playstyle: &'a P,
    evaluation: &'a EvaluationOf<'_, P>,
    blocks: &render::TextBlocks,
    quests: &[u16],
) {
    let info = web::LiveInfo {
        player_name: &snap.player.name,
        class: snap.player.class,
        char_level: snap.player.char_level,
        difficulty: snap.difficulty,
        area: &info::level_name(snap.level.id)
            .map_or_else(|| format!("Level {}", snap.level.id), str::to_owned),
        blocks,
        held_materials: &snap.held_materials,
        white_bases: &snap.white_bases,
        quests,
        equipped: &snap.equipped,
        equipped_items: &snap.equipped_items,
        owned_items: &snap.owned_items,
        owned: &snap.owned_names,
        runeword_copies: &snap.runeword_copies,
        merc_worn: &snap.merc_worn,
        stash: &snap.stash,
        stash_owned: &snap.stash_names,
        skills: &snap.skills,
        shop: snap.shop.as_ref(),
        strength: snap.player.strength,
        dexterity: snap.player.dexterity,
        marks: &evaluation.guidance.marks,
        updated_at: now_secs(),
    };
    web::publish_live(web::live_json(&info, playstyle.panels(evaluation, &info)));
}

/// Snapshots at [`TICK`] until the process exits; `Err` if the UI is gone.
#[allow(
    clippy::too_many_arguments,
    reason = "plain pass-through of run's settings; a struct for one caller adds nothing"
)]
fn follow_game<P: Playstyle>(
    reader: &mut impl GameReader,
    playstyle: &mut P,
    tx: &Sender<Msg>,
    player: Option<&str>,
    (w, h): (u32, u32),
    journal: &mut Journal,
    config: &mut config::Config,
    config_path: Option<&Path>,
    files: &Files,
) -> Result<(), UiGone> {
    let mut watched = Watched::now(config_path, files);
    let mut session = Session::default();
    let state_path = game_file::default_path();
    if let Some(state) = state_path.as_deref().and_then(game_file::load) {
        session.restore(state);
    }
    let mut saver = game_file::Saver::new(state_path);
    let mut cow_stats = cows::Counter::open(cows::default_path(), journal::default_dir().as_deref());
    let mut last_played = played::Tracker::open(played::default_path(), || {
        reader
            .save_dir()
            .map_or_else(played::Played::new, |dir| played::seed_from_saves(&dir))
    });
    // The game shows only the current difficulty's quests; this remembers the others.
    let mut quest_memory = quest_log::Memory::open(quest_log::default_path());
    let mut dedup = Dedup::default();
    // The previous tick's wants, by player unit id: the drop marks are needed before the
    // session merge that the evaluation needs, so keep marks run one tick late.
    let mut wants = (None, P::Wants::default());
    loop {
        let start = Instant::now();
        if check_reload(config, config_path, files, &mut watched) {
            playstyle.reload(files);
        }
        match reader.snapshot(player, session.player_id()) {
            Ok(mut snap) => {
                if wants.0 != Some(snap.player.id) {
                    wants = (None, P::Wants::default());
                }
                let marks = playstyle.mark_drops(&mut snap.drops, &wants.1);
                // Before `apply` and the shrine filter, which drop what the journal records.
                journal.observe(&snap, &marks);
                last_played.record(
                    &snap.player.name,
                    Some(snap.player.class),
                    now_secs(),
                    Instant::now(),
                );
                let update = session.apply(&mut snap);
                config.shrines.retain_shown(&mut snap.shrines, snap.difficulty);
                let quests = reader.quest_flags(update.new_game);
                let quests_done = quest_memory.observe(&snap.player.name, snap.difficulty, quests);
                let mut evaluation =
                    playstyle.evaluate(&snap, last_played.reached(&snap.player.name), quests_done);
                if let Some(slug) = evaluation.reached {
                    last_played.reach(&snap.player.name, slug);
                }
                evaluation.guidance.marks = marks;
                wants = (Some(snap.player.id), std::mem::take(&mut evaluation.wants));
                if let Some(arrival) = update.arrival {
                    journal.arrival(&snap, arrival);
                }
                if update.new_game {
                    log_new_game(&snap);
                }
                cow_stats.record(&update, session.cow_kills());
                saver.tick(&mut session, Instant::now());
                publish_web(&snap, &*playstyle, &evaluation, w, quests);
                record_stash_cache(&update, journal);
                if let Some(msg) = tick_msg(&mut dedup, snap, evaluation.guidance, (w, h), info::font()) {
                    send(tx, msg)?;
                }
            }
            Err(e) => {
                if !reader.is_alive() {
                    cow_stats.flush();
                    return Ok(());
                }
                web::clear_live();
                reader.forget_quests();
                if dedup.hide(Hidden::NoGame) {
                    eprintln!("no game state (menu or loading?): {e:#}");
                    send(tx, Msg::Hide)?;
                }
            }
        }
        cow_stats.tick(Instant::now());
        thread::sleep(TICK.saturating_sub(start.elapsed()));
    }
}

fn log_new_game(snap: &Snapshot) {
    eprintln!(
        "new game (previous player no longer present): remembered markers and kills cleared; \
         player {:?} chosen by {} among {} candidates",
        snap.player.name,
        snap.player_chosen_by.label(),
        snap.player_candidates.len()
    );
}

fn record_stash_cache(update: &crate::session::Update, journal: &mut Journal) {
    if let Some((tabs, items)) = update.stash_captured {
        eprintln!("shared stash captured: {tabs} tabs, {items} items");
        journal.stash(true, tabs, items);
    }
    if update.stash_invalidated {
        eprintln!("shared stash cache dropped (new game)");
        journal.stash(false, 0, 0);
    }
}

/// The overlay hides in towns, elsewhere shows the map. `None`: the screen is up to date (or
/// rendering failed, logged).
fn tick_msg(
    dedup: &mut Dedup,
    snap: Snapshot,
    guidance: Guidance,
    (w, h): (u32, u32),
    font: Option<&FontVec>,
) -> Option<Msg> {
    if game::is_town(snap.level.id) {
        return dedup.hide(Hidden::Town).then_some(Msg::Hide);
    }
    let (snap, guidance) = dedup.changed(snap, guidance)?;
    match render::render_with(snap, guidance, w, h, font) {
        Ok(frame) => Some(Msg::Frame(frame)),
        Err(e) => {
            eprintln!("rendering failed: {e:#}");
            None
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hidden {
    Town,
    /// Menu or loading.
    NoGame,
}

/// Suppresses re-rendering an unchanged frame and repeating a [`Msg::Hide`].
#[derive(Debug, Default)]
struct Dedup {
    /// `None` while hidden.
    last: Option<(Snapshot, Guidance)>,
    hidden: Option<Hidden>,
}

impl Dedup {
    /// `None` if they equal the ones on screen.
    fn changed(&mut self, snap: Snapshot, guidance: Guidance) -> Option<(&Snapshot, &Guidance)> {
        self.hidden = None;
        let frame = (snap, guidance);
        if self.last.as_ref() == Some(&frame) {
            return None;
        }
        let (snap, guidance) = self.last.insert(frame);
        Some((snap, guidance))
    }

    /// True if this starts a run of hidden ticks for `why`.
    fn hide(&mut self, why: Hidden) -> bool {
        // After a hide, even an identical snapshot must be sent again.
        self.last = None;
        self.hidden.replace(why) != Some(why)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::{Context, Result};

    fn fixture() -> Result<Snapshot> {
        let json = include_str!("../../../tests/fixtures/jail1-snapshot.json");
        serde_json::from_str(json).context("parsing fixture")
    }

    fn speed() -> crate::game::SpeedStats {
        crate::game::SpeedStats {
            fcr: 22,
            ias: 0,
            fhr: 48,
            res_fire: 40,
            res_cold: 75,
            res_lightning: 10,
            res_poison: 75,
        }
    }

    #[test]
    fn a_town_hides_once() -> Result<()> {
        let mut d = Dedup::default();
        let mut town = fixture()?;
        town.level.id = 1;
        let font = info::font();
        assert!(matches!(
            tick_msg(&mut d, fixture()?, Guidance::default(), (200, 200), font),
            Some(Msg::Frame(_))
        ));
        assert!(matches!(
            tick_msg(&mut d, town, Guidance::default(), (200, 200), font),
            Some(Msg::Hide)
        ));
        let mut town = fixture()?;
        town.level.id = 1;
        assert!(
            tick_msg(&mut d, town, Guidance::default(), (200, 200), font).is_none(),
            "hidden once"
        );
        Ok(())
    }

    #[test]
    fn towns_hide_even_with_a_guide_and_the_field_shows_the_map() -> Result<()> {
        let font = info::font();
        let mut town = fixture()?;
        town.level.id = 1;
        town.player.speed = Some(speed());
        let mut d = Dedup::default();
        assert!(matches!(
            tick_msg(&mut d, town, Guidance::default(), (200, 200), font),
            Some(Msg::Hide)
        ));
        let h = 300;
        assert_eq!(render::map_height(h, None), h, "no font: no info band");
        let mut snap = fixture()?;
        snap.player.speed = Some(speed());
        let mut d = Dedup::default();
        assert!(matches!(
            tick_msg(&mut d, snap, Guidance::default(), (200, h), font),
            Some(Msg::Frame(_))
        ));
        Ok(())
    }

    #[test]
    fn identical_snapshots_are_suppressed() -> Result<()> {
        let mut d = Dedup::default();
        assert!(
            d.changed(fixture()?, Guidance::default()).is_some(),
            "first snapshot renders"
        );
        assert!(
            d.changed(fixture()?, Guidance::default()).is_none(),
            "identical one does not"
        );
        let mut moved = fixture()?;
        moved.player.x += 1;
        assert!(
            d.changed(moved, Guidance::default()).is_some(),
            "a moved player renders"
        );
        Ok(())
    }

    #[test]
    fn failures_hide_once_and_resend_after() -> Result<()> {
        let mut d = Dedup::default();
        assert!(d.changed(fixture()?, Guidance::default()).is_some());
        assert!(d.hide(Hidden::NoGame), "first failure hides");
        assert!(!d.hide(Hidden::NoGame), "later failures don't repeat the hide");
        assert!(
            d.changed(fixture()?, Guidance::default()).is_some(),
            "same snapshot after a hide is sent again"
        );
        assert!(d.changed(fixture()?, Guidance::default()).is_none());
        assert!(d.hide(Hidden::NoGame), "a new failure run hides again");
        Ok(())
    }

    #[test]
    fn towns_hide_quietly() -> Result<()> {
        let mut town = fixture()?;
        town.level.id = 1;
        assert!(game::is_town(town.level.id));
        assert!(!game::is_town(fixture()?.level.id), "Jail 1 is not a town");
        let mut d = Dedup::default();
        assert!(d.changed(fixture()?, Guidance::default()).is_some());
        assert!(d.hide(Hidden::Town), "entering town hides once");
        assert!(!d.hide(Hidden::Town), "staying in town sends nothing");
        assert!(d.hide(Hidden::NoGame));
        assert!(d.hide(Hidden::Town));
        assert!(
            d.changed(fixture()?, Guidance::default()).is_some(),
            "leaving town shows the map again"
        );
        Ok(())
    }

    #[test]
    fn config_and_playstyle_changes_are_noticed() -> Result<()> {
        let dir = crate::testutil::temp_dir("hot-reload")?;
        let shipped = dir.join("shipped");
        std::fs::create_dir_all(shipped.join("builds"))?;
        let path = dir.join("config.toml");
        std::fs::write(&path, "[shrines]\nshow = [\"skill\"]\n")?;
        let files = Files {
            shipped: Some(shipped.clone()),
            ..Files::default()
        };
        let mut cfg = config::load_from(&path);
        let mut watched = Watched::now(Some(&path), &files);
        assert!(
            !check_reload(&mut cfg, Some(&path), &files, &mut watched),
            "nothing changed"
        );
        // Explicit mtimes: two writes in the same filesystem tick would look unchanged.
        let touch = |file: &Path, secs: u64| -> Result<()> {
            let at = SystemTime::UNIX_EPOCH + Duration::from_secs(secs);
            std::fs::File::options()
                .write(true)
                .open(file)?
                .set_modified(at)?;
            Ok(())
        };
        let farm = shipped.join("farm.toml");
        std::fs::write(&farm, "")?;
        assert!(
            check_reload(&mut cfg, Some(&path), &files, &mut watched),
            "farm.toml created"
        );
        assert!(!check_reload(&mut cfg, Some(&path), &files, &mut watched), "once");
        let build = shipped.join("builds/a.toml");
        std::fs::write(&build, "name = \"A\"\n")?;
        assert!(
            check_reload(&mut cfg, Some(&path), &files, &mut watched),
            "build added"
        );
        touch(&build, 1_000)?;
        assert!(
            check_reload(&mut cfg, Some(&path), &files, &mut watched),
            "build edited"
        );
        assert_eq!(cfg.shrines.show, vec![12], "the core config untouched");
        std::fs::write(&path, "[shrines]\nshow = [\"gem\"]\n")?;
        touch(&path, 2_000)?;
        assert!(check_reload(&mut cfg, Some(&path), &files, &mut watched));
        assert_eq!(cfg.shrines.show, vec![18]);
        std::fs::write(&path, "[shrines\n")?;
        touch(&path, 3_000)?;
        assert!(check_reload(&mut cfg, Some(&path), &files, &mut watched));
        assert_eq!(cfg.shrines.show, vec![18], "kept");
        std::fs::remove_dir_all(&dir)?;
        Ok(())
    }
}

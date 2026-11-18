//! Live mode reader thread: follows the game process and sends rendered frames to the UI loop.
//!
//! Waits for D2R, opens its memory, locates the image and unit table once per process, then
//! snapshots and renders ~5 times a second. Each snapshot is folded into a [`Session`]
//! (markers seen, superunique kills; saved by [`game_file`] across overlay restarts); unchanged
//! results are not re-rendered or sent. The overlay is hidden in towns and outside a game. The item-code table is searched once per process, in a
//! background thread, when the first item lies on the ground.

use std::io;
use std::path::Path;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::{Duration, Instant, SystemTime};

use smithay_client_toolkit::reexports::calloop::channel::Sender;

use crate::game::{self, Snapshot};
use crate::journal::Journal;
use crate::overlay::Msg;
use crate::process::ProcMem;
use crate::session::Session;
use ab_glyph::FontVec;

use crate::{
    config, cows, game_file, image, info, items, journal, offset_cache, process, render, valuables, web,
};

/// Snapshot interval (~5 Hz).
const TICK: Duration = Duration::from_millis(200);
/// Wait between attempts to find the game or open its memory.
const RETRY: Duration = Duration::from_secs(2);

/// Wait before the next unit-table attempt after `failures` failed ones: 2 s, 5 s, then 10 s.
const fn table_retry_delay(failures: u32) -> Duration {
    match failures {
        0 | 1 => Duration::from_secs(2),
        2 => Duration::from_secs(5),
        _ => Duration::from_secs(10),
    }
}

/// The UI loop dropped its end of the channel: stop.
#[derive(Debug)]
struct UiGone;

/// Unix seconds right now (`0` before the epoch, which no live clock is).
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

fn send(tx: &Sender<Msg>, msg: Msg) -> Result<(), UiGone> {
    tx.send(msg).map_err(|_| UiGone)
}

/// Runs until the UI is gone. `player` picks among several player units; `size` is the frame size;
/// `journal` records what is seen (see [`crate::journal`]); `config` picks the highlighted drop tiers
/// and the shrines drawn.
pub fn run(
    tx: &Sender<Msg>,
    player: Option<&str>,
    size: (u32, u32),
    mut journal: Journal,
    config: &config::Config,
) {
    // The companion web page: selects each character's build in a browser (user decision,
    // 2026-11-17).
    thread::spawn(crate::web::serve);
    let config_path = config::default_path();
    let mut watched = Watched::now(config_path.as_deref());
    let cache = offset_cache::default_path();
    // The last open error printed: a repeat isn't printed again, a different one is.
    let mut open_err = String::new();
    let mut waiting_logged = false;
    let mut config = config.clone();
    loop {
        // Hot-reload while no game is attached (webapp edits, hand edits); during a session
        // `follow_game` reloads on its own tick loop.
        check_reload(&mut config, config_path.as_deref(), &mut watched);
        let Ok(pid) = process::find_d2r_pid() else {
            if !waiting_logged {
                eprintln!("waiting for D2R to start");
                waiting_logged = true;
            }
            thread::sleep(RETRY);
            continue;
        };
        let mem = match ProcMem::open(pid) {
            Ok(mem) => mem,
            Err(e) => {
                let msg = format!("{e:#}");
                if msg != open_err {
                    eprintln!("{msg}");
                    if is_permission_denied(&e) {
                        eprintln!("{}", ptrace_hint());
                    }
                    open_err = msg;
                }
                thread::sleep(RETRY);
                continue;
            }
        };
        open_err.clear();
        eprintln!("attached to D2R (pid {pid})");
        waiting_logged = false;
        let follow = find_table(pid, &mem, cache.as_deref()).map_or(Ok(()), |table| {
            follow_game(
                tx,
                pid,
                &mem,
                table,
                player,
                size,
                &mut journal,
                &mut config,
                config_path.as_deref(),
            )
        });
        // `follow_game` may have reloaded on its own: resync so the next outer check doesn't
        // reload the same file twice.
        watched = Watched::now(config_path.as_deref());
        // The process is gone (or never got a table): hide until the next one is in a game.
        // The dashboard keeps the last snapshot, marked offline.
        web::clear_live();
        if follow.and_then(|()| send(tx, Msg::Hide)).is_err() {
            return;
        }
        eprintln!("D2R (pid {pid}) exited");
    }
}

/// The file's last modification time, if readable.
fn modified_at(path: &std::path::Path) -> Option<std::time::SystemTime> {
    std::fs::metadata(path).and_then(|m| m.modified()).ok()
}

/// The watched state behind the hot reload: `config.toml`'s mtime and the `builds/`
/// directory's per-file `(name, mtime)` list. The builds dir is re-checked with a cheap
/// `read_dir` every tick (user decision, 2026-11-17): a directory's own mtime would miss
/// in-place edits of a build file, while this notices both those and added/removed files.
#[derive(Debug, Default, PartialEq)]
struct Watched {
    config: Option<SystemTime>,
    builds: Option<Vec<(String, SystemTime)>>,
}

impl Watched {
    /// The current on-disk state of `config_path` and its `builds/` directory.
    fn now(config_path: Option<&Path>) -> Self {
        Self {
            config: config_path.and_then(modified_at),
            builds: config_path
                .map(config::builds_dir_for)
                .and_then(|dir| builds_state(&dir)),
        }
    }
}

/// The builds dir's `(file name, mtime)` pairs sorted by name, or `None` when the directory
/// doesn't exist (no builds — also the state a later creation is detected against).
fn builds_state(dir: &Path) -> Option<Vec<(String, SystemTime)>> {
    let entries = std::fs::read_dir(dir).ok()?;
    let mut state = Vec::new();
    for entry in entries.flatten() {
        let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else {
            continue;
        };
        state.push((entry.file_name().to_string_lossy().into_owned(), modified));
    }
    state.sort();
    Some(state)
}

/// Hot-reloads `config` when `config.toml` or its `builds/` directory changed on disk (webapp
/// edits, hand edits, agent-authored build files). A failed reload (unreadable, invalid TOML)
/// logs and keeps the current config, so a half-written or broken file can never blank the
/// overlay's settings; the watched state is still recorded, so the error is logged once per
/// change, not once per tick.
fn check_reload(config: &mut config::Config, config_path: Option<&std::path::Path>, watched: &mut Watched) {
    let Some(path) = config_path else { return };
    let now = Watched::now(config_path);
    if *watched == now {
        return;
    }
    *watched = now;
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) => {
            eprintln!("config {}: {e}; keeping the current one", path.display());
            return;
        }
    };
    match config::parse(&text) {
        Ok((mut loaded, warnings)) => {
            for w in warnings {
                eprintln!("config {}: {w}", path.display());
            }
            loaded.build_defs = config::load_builds(&config::builds_dir_for(path));
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

/// Finds the unit table, retrying with [`table_retry_delay`] while the process lives (the
/// structural search needs a game in progress). Only the first attempt reads the whole image;
/// retries read just its header and `.data`. `None` once the process is gone.
fn find_table(pid: u32, mem: &ProcMem, cache: Option<&Path>) -> Option<u64> {
    let mut last_err = String::new();
    let mut failures = 0u32;
    while is_alive(pid) {
        let found = process::read_maps(pid).and_then(|maps| {
            let img = image::find_image(&maps, mem)?;
            img.unit_table(mem, cache, failures == 0)
        });
        match found {
            Ok(table) => {
                eprintln!(
                    "unit table at {:#x} (rva {:#x}, via {:?})",
                    table.addr, table.rva, table.source
                );
                return Some(table.addr);
            }
            Err(e) => {
                let msg = format!("{e:#}");
                if msg != last_err {
                    eprintln!("unit table not found yet, retrying: {msg}");
                    last_err = msg;
                }
            }
        }
        failures = failures.saturating_add(1);
        thread::sleep(table_retry_delay(failures));
    }
    None
}

/// Publishes one tick's dashboard JSON ([`web::live_json`]) from the snapshot, the dashboard's
/// text blocks, the guide stage and the quest flags.
fn publish_dashboard(
    snap: &game::Snapshot,
    blocks: &render::TextBlocks,
    stage: Option<config::Stage<'_>>,
    quests: &[u16],
) {
    web::publish_live(web::live_json(&web::LiveInfo {
        player_name: &snap.player.name,
        class: snap.player.class,
        char_level: snap.player.char_level,
        difficulty: snap.difficulty,
        area: &info::level_name(snap.level.id)
            .map_or_else(|| format!("Level {}", snap.level.id), str::to_owned),
        blocks,
        bis: snap.player.bis.as_ref(),
        build: stage.map(|s| s.build),
        stage,
        held_materials: &snap.held_materials,
        white_bases: &snap.white_bases,
        quests,
        equipped: &snap.equipped,
        equipped_items: &snap.equipped_items,
        owned: &snap.owned_names,
        stash: &snap.stash,
        stash_owned: &snap.stash_names,
        skills: &snap.skills,
        shop: snap.shop.as_ref(),
        strength: snap.player.strength,
        dexterity: snap.player.dexterity,
        updated_at: now_secs(),
    }));
}

/// Snapshots at [`TICK`] until the process exits; `Ok(())` then, `Err` if the UI is gone.
#[allow(
    clippy::too_many_arguments,
    reason = "plain pass-through of run's settings; a struct for one caller adds nothing"
)]
fn follow_game(
    tx: &Sender<Msg>,
    pid: u32,
    mem: &ProcMem,
    table: u64,
    player: Option<&str>,
    (w, h): (u32, u32),
    journal: &mut Journal,
    config: &mut config::Config,
    config_path: Option<&Path>,
) -> Result<(), UiGone> {
    let mut watched = Watched::now(config_path);
    let mut session = Session::default();
    let state_path = game_file::default_path();
    if let Some(state) = state_path.as_deref().and_then(game_file::load) {
        session.restore(state);
    }
    let mut saver = game_file::Saver::new(state_path);
    let mut cow_stats = cows::Counter::open(cows::default_path(), journal::default_dir().as_deref());
    let mut dedup = Dedup::default();
    let mut items_table = ItemsTable::NotSearched;
    let mut quest_flags = QuestFlags::default();
    loop {
        let start = Instant::now();
        // Hot-reload here, not only in `run`: this loop is where every consumer (valuables,
        // shrines, builds, bis) reads `config`, and it only returns when D2R exits.
        check_reload(config, config_path, &mut watched);
        // The previous player id (or game.json's on a restart) keeps the choice sticky.
        match game::snapshot(mem, table, items_table.base(), player, session.player_id()) {
            Ok(mut snap) => {
                // Codes are needed for ground drops and for the inventory keep list.
                if snap.ground_items > 0 || snap.inventory_items > 0 {
                    items_table.search(pid);
                }
                // Highlights first: the journal records them with the drop.
                valuables::apply(&mut snap.drops, &config.valuables);
                // Before `apply` and the shrine filter: they drop monster presets and shrines
                // the journal wants to see.
                journal.observe(&snap);
                let update = session.apply(&mut snap);
                config.shrines.retain_shown(&mut snap.shrines, snap.difficulty);
                // Damages: the character's own `[builds]` entry, else the selected build's,
                // else its class's. `BiS`: inline slots when present, else the selected
                // build's (user decision, 2026-11-17).
                snap.player.build = config
                    .damages_for_player(&snap.player.name, Some(snap.player.class), snap.player.char_level)
                    .to_vec();
                let (class, level) = (Some(snap.player.class), snap.player.char_level);
                let stage = config.stage(&snap.player.name, class, level);
                snap.player.bis = config.resolved_bis(&snap.player.name, class, level);
                if let Some(arrival) = update.arrival {
                    journal.arrival(&snap, arrival);
                }
                if update.new_game {
                    eprintln!(
                        "new game (previous player no longer present): remembered markers and kills cleared; \
                         player {:?} chosen by {} among {} candidates",
                        snap.player.name,
                        snap.player_chosen_by.label(),
                        snap.player_candidates.len()
                    );
                }
                cow_stats.record(&update, session.cow_kills());
                saver.tick(&mut session, Instant::now());
                // The webapp's live dashboard gets the level, keep, sell, give-away and BiS
                // blocks the overlay no longer draws (2026-11-17 text-stack migration);
                // serialization of a few short lines at 5 Hz is cheap. Cleared again on
                // snapshot errors (menu, loading) and game exit.
                let blocks = render::text_blocks(&snap, w, info::font());
                // The config page annotates its resolved `BiS` view with the live ✓ and the
                // equipped-quality pairs; published alongside the dashboard JSON.
                web::publish_gear(web::GearInfo {
                    worn: crate::game::worn_names(&snap.equipped_items),
                    owned: snap.owned_names.clone(),
                    stash: snap.stash_names.clone(),
                    equipped: snap.equipped.clone(),
                    merc_equipped: snap.merc_equipped.clone(),
                });
                let quests = quest_flags.read(pid, mem, update.new_game);
                publish_dashboard(&snap, &blocks, stage, quests);
                if let Some((tabs, items)) = update.stash_captured {
                    eprintln!("shared stash captured: {tabs} tabs, {items} items");
                    journal.stash(true, tabs, items);
                }
                if update.stash_invalidated {
                    eprintln!("shared stash cache dropped (new game)");
                    journal.stash(false, 0, 0);
                }
                if let Some(msg) = tick_msg(&mut dedup, snap, (w, h), info::font()) {
                    send(tx, msg)?;
                }
            }
            Err(e) => {
                if !is_alive(pid) {
                    cow_stats.flush();
                    return Ok(());
                }
                // Menu or loading: the dashboard keeps the last game, marked offline.
                web::clear_live();
                quest_flags.forget();
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

/// Wait between searches for the quest-flags global while it isn't found.
const QUEST_SEARCH_EVERY: Duration = Duration::from_secs(10);
/// Failed searches before giving up until the next game (each one reads `.data` and ~45k
/// headers on the tick thread, so a miss must not repeat forever).
const QUEST_SEARCH_TRIES: u32 = 3;

/// The quest-flags global of the attached process ([`image::Image::quest_flags_global`]):
/// searched during a game, at most every [`QUEST_SEARCH_EVERY`] and [`QUEST_SEARCH_TRIES`]
/// times per game, until found; then the record is read through it each tick (48 words,
/// cheap). The last good record is kept across a failed read, so used quest recipes don't
/// flicker back; it is dropped on a new game and when the snapshot fails (menu, exit).
#[derive(Debug, Default)]
struct QuestFlags {
    global: Option<u64>,
    last_search: Option<Instant>,
    /// Failed searches in this game.
    failures: u32,
    /// The last record read successfully.
    record: Vec<u16>,
    /// The last search error printed (a repeat isn't printed again).
    last_err: String,
}

impl QuestFlags {
    /// The current game's quest record (the last good one if this tick's read failed); empty
    /// until one was read. `new_game` resets the record and the search budget.
    fn read(&mut self, pid: u32, mem: &ProcMem, new_game: bool) -> &[u16] {
        if new_game {
            self.record.clear();
            self.failures = 0;
        }
        let due = self.last_search.is_none_or(|t| t.elapsed() >= QUEST_SEARCH_EVERY);
        if self.global.is_none() && self.failures < QUEST_SEARCH_TRIES && due {
            self.last_search = Some(Instant::now());
            let found = process::read_maps(pid)
                .and_then(|maps| image::find_image(&maps, mem)?.quest_flags_global(mem));
            match found {
                Ok(addr) => {
                    eprintln!("quest flags at {addr:#x}");
                    self.global = Some(addr);
                }
                Err(e) => {
                    self.failures += 1;
                    let msg = format!("{e:#}");
                    if msg != self.last_err {
                        eprintln!(
                            "quest flags not found ({}/{QUEST_SEARCH_TRIES} this game): {msg}",
                            self.failures
                        );
                        self.last_err = msg;
                    }
                }
            }
        }
        if let Some(words) = self.global.and_then(|g| crate::quests::read(mem, g).ok()) {
            self.record = words;
        }
        &self.record
    }

    /// Drops the record (no game: menu, loading, exit); the global and budget stay.
    fn forget(&mut self) {
        self.record.clear();
    }
}

/// The item-code table of the attached process ([`items::locate`]). The search reads gigabytes
/// (about a second or more), so it runs on its own thread with its own `/proc/<pid>/mem` handle
/// while the snapshots go on; until it's done, only unique and set drops are shown.
#[derive(Debug)]
enum ItemsTable {
    NotSearched,
    Searching(Receiver<Option<u64>>),
    /// Searched once: found or not, it isn't searched again for this process.
    Done(Option<u64>),
}

impl ItemsTable {
    /// Starts the search, unless it already ran or is running.
    fn search(&mut self, pid: u32) {
        if !matches!(self, Self::NotSearched) {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let spawned = thread::Builder::new().name("items-scan".into()).spawn(move || {
            let found = ProcMem::open(pid).map_or_else(
                |e| {
                    eprintln!("items table: {e:#}");
                    None
                },
                |mem| items::locate(pid, &mem),
            );
            // The reader may have moved on to another process: nobody to tell, that's fine.
            let _ = tx.send(found);
        });
        *self = match spawned {
            Ok(_) => Self::Searching(rx),
            Err(e) => {
                eprintln!("items table: could not start the search thread: {e}");
                Self::Done(None)
            }
        };
    }

    /// The table's address once the search found it.
    fn base(&mut self) -> Option<u64> {
        match self {
            Self::NotSearched => None,
            Self::Done(base) => *base,
            Self::Searching(rx) => match rx.try_recv() {
                Ok(base) => {
                    *self = Self::Done(base);
                    base
                }
                Err(TryRecvError::Empty) => None,
                // The search thread died without an answer (panicked): give up for this process.
                Err(TryRecvError::Disconnected) => {
                    *self = Self::Done(None);
                    None
                }
            },
        }
    }
}

/// What one snapshot's tick sends, if anything. The overlay hides in towns (the breakpoint
/// guide line it used to show there overlapped the game's terror-zone list and lives on the
/// dashboard; user request, 2026-11-17); elsewhere the map. `None`: the screen is up to date
/// (or rendering failed, logged).
fn tick_msg(dedup: &mut Dedup, snap: Snapshot, (w, h): (u32, u32), font: Option<&FontVec>) -> Option<Msg> {
    if game::is_town(snap.level.id) {
        return dedup.hide(Hidden::Town).then_some(Msg::Hide);
    }
    let snap = dedup.changed(snap)?;
    match render::render_with(snap, w, h, font) {
        Ok(frame) => Some(Msg::Frame(frame)),
        Err(e) => {
            eprintln!("rendering failed: {e:#}");
            None
        }
    }
}

/// Why the overlay is hidden.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Hidden {
    /// The player is in a town (quietly hidden).
    Town,
    /// No snapshot could be read (menu, loading).
    NoGame,
}

/// Suppresses repeated work: the same snapshot twice in a row is not re-rendered, and a run of
/// hidden ticks (for the same reason) sends one [`Msg::Hide`].
#[derive(Debug, Default)]
struct Dedup {
    /// The last snapshot sent as a frame; `None` while hidden.
    last: Option<Snapshot>,
    /// Why the previous tick hid the overlay, if it did (a hide was already sent).
    hidden: Option<Hidden>,
}

impl Dedup {
    /// The snapshot to render, or `None` if it equals the one on screen.
    fn changed(&mut self, snap: Snapshot) -> Option<&Snapshot> {
        self.hidden = None;
        if self.last.as_ref() == Some(&snap) {
            return None;
        }
        Some(self.last.insert(snap))
    }

    /// Records a hidden tick; true if this starts a run for `why` (send a hide, log the reason).
    fn hide(&mut self, why: Hidden) -> bool {
        // Forget the frame: after the hide, even an identical snapshot must be sent again.
        self.last = None;
        self.hidden.replace(why) != Some(why)
    }
}

fn is_alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
}

/// True if any cause in the chain is EPERM/EACCES (both map to `PermissionDenied`).
fn is_permission_denied(e: &anyhow::Error) -> bool {
    e.chain()
        .filter_map(|c| c.downcast_ref::<io::Error>())
        .any(|io| io.kind() == io::ErrorKind::PermissionDenied)
}

fn ptrace_hint() -> String {
    let exe =
        std::env::current_exe().map_or_else(|_| "d2r-map-overlay".to_owned(), |p| p.display().to_string());
    format!(
        "reading the game's memory needs CAP_SYS_PTRACE; grant it once per build with:\n  \
         sudo setcap cap_sys_ptrace=ep {exe}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::{Context, Result};

    fn fixture() -> Result<Snapshot> {
        let json = include_str!("../tests/fixtures/jail1-snapshot.json");
        serde_json::from_str(json).context("parsing fixture")
    }

    /// A readable speed stat list: the breakpoint guide's input (see [`crate::game::SpeedStats`]).
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
            tick_msg(&mut d, fixture()?, (200, 200), font),
            Some(Msg::Frame(_))
        ));
        assert!(matches!(
            tick_msg(&mut d, town, (200, 200), font),
            Some(Msg::Hide)
        ));
        let mut town = fixture()?;
        town.level.id = 1;
        assert!(tick_msg(&mut d, town, (200, 200), font).is_none(), "hidden once");
        Ok(())
    }

    #[test]
    fn towns_hide_even_with_a_guide_and_the_field_shows_the_map() -> Result<()> {
        let font = info::font();
        // The guide line moved to the dashboard (it overlapped the game's terror-zone list):
        // a town hides even when the breakpoint guide could be shown.
        let mut town = fixture()?;
        town.level.id = 1;
        town.player.speed = Some(speed());
        let mut d = Dedup::default();
        assert!(matches!(
            tick_msg(&mut d, town, (200, 200), font),
            Some(Msg::Hide)
        ));
        let h = 300;
        assert_eq!(render::map_height(h, None), h, "no font: no info band");
        let mut snap = fixture()?;
        snap.player.speed = Some(speed());
        let mut d = Dedup::default();
        assert!(matches!(
            tick_msg(&mut d, snap, (200, h), font),
            Some(Msg::Frame(_))
        ));
        Ok(())
    }

    #[test]
    fn identical_snapshots_are_suppressed() -> Result<()> {
        let mut d = Dedup::default();
        assert!(d.changed(fixture()?).is_some(), "first snapshot renders");
        assert!(d.changed(fixture()?).is_none(), "identical one does not");
        let mut moved = fixture()?;
        moved.player.x += 1;
        assert!(d.changed(moved).is_some(), "a moved player renders");
        Ok(())
    }

    #[test]
    fn failures_hide_once_and_resend_after() -> Result<()> {
        let mut d = Dedup::default();
        assert!(d.changed(fixture()?).is_some());
        assert!(d.hide(Hidden::NoGame), "first failure hides");
        assert!(!d.hide(Hidden::NoGame), "later failures don't repeat the hide");
        assert!(
            d.changed(fixture()?).is_some(),
            "same snapshot after a hide is sent again"
        );
        assert!(d.changed(fixture()?).is_none());
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
        assert!(d.changed(fixture()?).is_some());
        assert!(d.hide(Hidden::Town), "entering town hides once");
        assert!(!d.hide(Hidden::Town), "staying in town sends nothing");
        // Town → menu: a different reason, so the menu is logged and hidden (again, harmless).
        assert!(d.hide(Hidden::NoGame));
        assert!(d.hide(Hidden::Town));
        assert!(
            d.changed(fixture()?).is_some(),
            "leaving town shows the map again"
        );
        Ok(())
    }

    #[test]
    fn items_table_is_taken_once_found() {
        let mut t = ItemsTable::NotSearched;
        assert_eq!(t.base(), None);
        let (tx, rx) = mpsc::channel();
        t = ItemsTable::Searching(rx);
        assert_eq!(t.base(), None, "still searching");
        assert!(tx.send(Some(0x1234)).is_ok());
        assert_eq!(t.base(), Some(0x1234));
        drop(tx);
        assert_eq!(t.base(), Some(0x1234), "kept after the thread is gone");
        t.search(1);
        assert!(matches!(t, ItemsTable::Done(Some(0x1234))), "not searched again");

        let (tx, rx) = mpsc::channel::<Option<u64>>();
        let mut t = ItemsTable::Searching(rx);
        drop(tx);
        assert_eq!(t.base(), None);
        assert!(matches!(t, ItemsTable::Done(None)), "a dead search gives up");
    }

    #[test]
    fn table_retry_backs_off() {
        let secs: Vec<u64> = (1..=5).map(|n| table_retry_delay(n).as_secs()).collect();
        assert_eq!(secs, vec![2, 5, 10, 10, 10]);
        assert_eq!(table_retry_delay(u32::MAX).as_secs(), 10);
    }

    #[test]
    fn permission_denied_is_detected_through_context() {
        let e = anyhow::Error::from(io::Error::from(io::ErrorKind::PermissionDenied)).context("opening mem");
        assert!(is_permission_denied(&e));
        let e = anyhow::Error::from(io::Error::from(io::ErrorKind::NotFound)).context("opening mem");
        assert!(!is_permission_denied(&e));
    }
}

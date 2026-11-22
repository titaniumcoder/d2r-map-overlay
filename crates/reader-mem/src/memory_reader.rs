//! [`MemoryReader`]: the game layer's [`GameReader`], over any [`Platform`].

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};

use d2r_core::game::Snapshot;
use d2r_core::reader::{Attach, GameReader};
use d2r_core::{items, quests};

use crate::ui_panels;
use crate::ui_probe::{self, UiProbe};
use crate::{Platform, game, offset_cache};

/// Wait before the next unit-table attempt after `failures` failed ones: 2 s, 5 s, then 10 s.
const fn table_retry_delay(failures: u32) -> Duration {
    match failures {
        0 | 1 => Duration::from_secs(2),
        2 => Duration::from_secs(5),
        _ => Duration::from_secs(10),
    }
}

/// Reads the game through platform `P`. `Default` gives a detached reader using the offset
/// cache at [`offset_cache::default_path`].
#[derive(Debug)]
pub struct MemoryReader<P: Platform> {
    platform: P,
    /// The per-build unit-table offset cache (`None`: no cache dir).
    cache: Option<PathBuf>,
    attached: Option<Attached<P::Mem>>,
    /// "waiting for D2R" was printed and no process was found since.
    waiting_logged: bool,
    /// The last open error printed: a repeat isn't printed again, a different one is.
    open_err: String,
}

impl<P: Platform + Default> Default for MemoryReader<P> {
    fn default() -> Self {
        Self::new(P::default(), offset_cache::default_path())
    }
}

/// The game process this reader is attached to, with its located tables.
#[derive(Debug)]
struct Attached<M> {
    pid: u32,
    mem: M,
    /// Live address of the unit hash table.
    table: u64,
    items: ItemsTable,
    quests: QuestFlags,
    probe: Probe,
    /// The panel flags' address ([`ui_panels::address`]).
    panels: Option<u64>,
    /// The stash window was open at the last snapshot.
    stash_open: bool,
}

impl<M> Attached<M> {
    fn new(pid: u32, mem: M, table: u64, panels: Option<u64>) -> Self {
        Self {
            pid,
            mem,
            table,
            items: ItemsTable::NotSearched,
            quests: QuestFlags::new(),
            panels,
            stash_open: false,
            probe: if std::env::var_os(ui_probe::ENV).is_some() {
                Probe::Wanted
            } else {
                Probe::Off
            },
        }
    }
}

/// The research [`UiProbe`]: off unless [`ui_probe::ENV`] is set; opened on the first town
/// tick, off for good after a failure (logged).
#[derive(Debug)]
enum Probe {
    Off,
    Wanted,
    On(Box<UiProbe>),
}

impl Probe {
    fn tick<P: Platform>(&mut self, platform: &P, pid: u32, mem: &P::Mem, snap: &Snapshot) {
        if matches!(self, Self::Wanted) && d2r_core::game::is_town(snap.level.id) {
            let opened = platform.find_image(pid, mem).and_then(|image| {
                let dir = d2r_core::journal::data_dir().context("no data directory")?;
                UiProbe::open(image, mem, &dir)
            });
            *self = match opened {
                Ok(probe) => {
                    eprintln!("UI probe: writing {}", probe.path().display());
                    Self::On(Box::new(probe))
                }
                Err(e) => {
                    eprintln!("UI probe off: {e:#}");
                    Self::Off
                }
            };
        }
        if let Self::On(probe) = self
            && let Err(e) = probe.tick(mem, snap)
        {
            eprintln!("UI probe off: {e:#}");
            *self = Self::Off;
        }
    }
}

impl<P: Platform> MemoryReader<P> {
    pub const fn new(platform: P, cache: Option<PathBuf>) -> Self {
        Self {
            platform,
            cache,
            attached: None,
            waiting_logged: false,
            open_err: String::new(),
        }
    }

    /// Finds the unit table, retrying while the process lives (the structural search needs a
    /// game in progress); only the first attempt scans the code. `None` once the process is gone.
    /// The unit table's address and the panel flags' ([`ui_panels::address`]).
    fn find_table(&self, pid: u32, mem: &P::Mem) -> Option<(u64, Option<u64>)> {
        let mut last_err = String::new();
        let mut failures = 0u32;
        while self.platform.is_alive(pid) {
            let found = self.platform.find_image(pid, mem).and_then(|img| {
                let table = img.unit_table(mem, self.cache.as_deref(), failures == 0)?;
                let panels = ui_panels::address(&table.build, img.alias_base);
                Ok((table, panels))
            });
            match found {
                Ok((table, panels)) => {
                    eprintln!(
                        "unit table at {:#x} (rva {:#x}, via {:?})",
                        table.addr, table.rva, table.source
                    );
                    if panels.is_none() {
                        eprintln!(
                            "build {} is not {}: the stash window isn't detected",
                            table.build,
                            d2r_core::offsets::ui_panels::BUILD
                        );
                    }
                    return Some((table.addr, panels));
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

    /// Prints `e` (and the platform's hint) unless it repeats the last open error.
    fn log_open_error(&mut self, e: &anyhow::Error) {
        let msg = format!("{e:#}");
        if msg != self.open_err {
            eprintln!("{msg}");
            if let Some(hint) = self.platform.access_hint(e) {
                eprintln!("{hint}");
            }
            self.open_err = msg;
        }
    }
}

impl<P: Platform> GameReader for MemoryReader<P> {
    fn attach(&mut self) -> Attach {
        self.attached = None;
        let Ok(pid) = self.platform.find_game() else {
            if !self.waiting_logged {
                eprintln!("waiting for D2R to start");
                self.waiting_logged = true;
            }
            return Attach::Waiting;
        };
        let mem = match self.platform.open(pid) {
            Ok(mem) => mem,
            Err(e) => {
                self.log_open_error(&e);
                return Attach::Waiting;
            }
        };
        self.open_err.clear();
        eprintln!("attached to D2R (pid {pid})");
        self.waiting_logged = false;
        match self.find_table(pid, &mem) {
            Some((table, panels)) => {
                self.attached = Some(Attached::new(pid, mem, table, panels));
                Attach::Ready { pid }
            }
            None => Attach::Exited { pid },
        }
    }

    fn attach_now(&mut self, report: &mut dyn FnMut(&'static str, String)) -> Result<()> {
        self.attached = None;
        let pid = self.platform.find_game()?;
        report("pid", pid.to_string());
        let mem = self.platform.open(pid)?;
        let img = self
            .platform
            .find_image(pid, &mem)
            .context("locating D2R.exe image")?;
        report("live_base", format!("{:#x}", img.live_base));
        report("alias_base", format!("{:#x}", img.alias_base));
        report("size", format!("{:#x}", img.size));
        let table = img.unit_table(&mem, self.cache.as_deref(), true)?;
        report("build", table.build.clone());
        report(
            "unit_table",
            format!("{:#x} (rva {:#x}, via {:?})", table.addr, table.rva, table.source),
        );
        report(
            "cache",
            self.cache.as_ref().map_or_else(
                || "(none: neither $XDG_CACHE_HOME nor $HOME set)".to_owned(),
                |path| path.display().to_string(),
            ),
        );
        let panels = ui_panels::address(&table.build, img.alias_base);
        self.attached = Some(Attached::new(pid, mem, table.addr, panels));
        Ok(())
    }

    fn locate_items_now(&mut self) {
        if let Some(a) = &mut self.attached {
            a.items = ItemsTable::Done(locate_items(&self.platform, a.pid, &a.mem));
        }
    }

    fn snapshot(&mut self, player: Option<&str>, previous: Option<u32>) -> Result<Snapshot> {
        let a = self.attached.as_mut().context("not attached to the game")?;
        let snap = game::snapshot(&a.mem, a.table, a.items.base(), player, previous)?;
        let item_codes_needed = snap.ground_items > 0 || snap.inventory_items > 0;
        if item_codes_needed {
            a.items.search(&self.platform, a.pid);
        }
        a.probe.tick(&self.platform, a.pid, &a.mem, &snap);
        a.stash_open = d2r_core::game::is_town(snap.level.id)
            && a.panels
                .and_then(|addr| ui_panels::stash_open(&a.mem, addr))
                .unwrap_or(false);
        Ok(snap)
    }

    fn quest_flags(&mut self, new_game: bool) -> &[u16] {
        match &mut self.attached {
            Some(a) => a.quests.read(&self.platform, a.pid, &a.mem, new_game),
            None => &[],
        }
    }

    fn forget_quests(&mut self) {
        if let Some(a) = &mut self.attached {
            a.quests.forget();
        }
    }

    fn stash_open(&self) -> bool {
        self.attached.as_ref().is_some_and(|a| a.stash_open)
    }

    fn is_alive(&self) -> bool {
        self.attached
            .as_ref()
            .is_some_and(|a| self.platform.is_alive(a.pid))
    }

    fn save_dir(&self) -> Option<PathBuf> {
        self.attached.as_ref().and_then(|a| self.platform.save_dir(a.pid))
    }
}

/// The items table of process `pid` ([`items::locate`]); `None` (logged) if it can't be found.
fn locate_items<P: Platform>(platform: &P, pid: u32, mem: &P::Mem) -> Option<u64> {
    match platform.heap_regions(pid) {
        Ok(regions) => items::locate(mem, &regions),
        Err(e) => {
            eprintln!("items table: {e:#}; gems and runes are not shown");
            None
        }
    }
}

const QUEST_SEARCH_EVERY: Duration = Duration::from_secs(10);
/// Failed searches per game: each reads `.data` and ~45k headers on the tick thread.
const QUEST_SEARCH_TRIES: u32 = 3;

/// The quest-flags global ([`crate::image::Image::quest_flags_global`]), searched on a budget
/// until found, and the last good record read through it (kept so used quests don't flicker).
#[derive(Debug)]
struct QuestFlags {
    global: Option<u64>,
    last_search: Option<Instant>,
    /// Failed searches in this game.
    failures: u32,
    record: Vec<u16>,
    /// The last search error printed (a repeat isn't printed again).
    last_err: String,
}

impl QuestFlags {
    const fn new() -> Self {
        Self {
            global: None,
            last_search: None,
            failures: 0,
            record: Vec::new(),
            last_err: String::new(),
        }
    }

    /// The current game's quest record (the last good one if this tick's read failed); empty
    /// until one was read. `new_game` resets the record and the search budget.
    fn read<P: Platform>(&mut self, platform: &P, pid: u32, mem: &P::Mem, new_game: bool) -> &[u16] {
        if new_game {
            self.record.clear();
            self.failures = 0;
        }
        if self.search_due() {
            self.search(platform, pid, mem);
        }
        if let Some(words) = self.global.and_then(|g| quests::read(mem, g).ok()) {
            self.record = words;
        }
        &self.record
    }

    fn search_due(&self) -> bool {
        let waited = self.last_search.is_none_or(|t| t.elapsed() >= QUEST_SEARCH_EVERY);
        self.global.is_none() && self.failures < QUEST_SEARCH_TRIES && waited
    }

    fn search<P: Platform>(&mut self, platform: &P, pid: u32, mem: &P::Mem) {
        self.last_search = Some(Instant::now());
        let found = platform
            .find_image(pid, mem)
            .and_then(|img| img.quest_flags_global(mem));
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

    /// Drops the record (no game: menu, loading, exit); the global and budget stay.
    fn forget(&mut self) {
        self.record.clear();
    }
}

/// The item-code table ([`items::locate`]). The search reads gigabytes, so it runs once per
/// process on its own thread with its own memory handle.
#[derive(Debug)]
enum ItemsTable {
    NotSearched,
    Searching(Receiver<Option<u64>>),
    Done(Option<u64>),
}

impl ItemsTable {
    /// Starts the search, unless it already ran or is running.
    fn search<P: Platform>(&mut self, platform: &P, pid: u32) {
        if !matches!(self, Self::NotSearched) {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let platform = platform.clone();
        let spawned = thread::Builder::new().name("items-scan".into()).spawn(move || {
            let found = platform.open(pid).map_or_else(
                |e| {
                    eprintln!("items table: {e:#}");
                    None
                },
                |mem| locate_items(&platform, pid, &mem),
            );
            // The receiver is gone if the reader moved on to another process.
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
                Err(TryRecvError::Disconnected) => {
                    *self = Self::Done(None);
                    None
                }
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ops::Range;

    use anyhow::{anyhow, bail};

    use crate::image::Image;
    use d2r_core::testutil::FakeMem;

    /// A platform with no game (`pid: None`) or one whose memory can't be opened.
    #[derive(Debug, Clone, Default)]
    struct FakePlatform {
        pid: Option<u32>,
    }

    impl Platform for FakePlatform {
        type Mem = FakeMem;

        fn find_game(&self) -> Result<u32> {
            self.pid.context("no game")
        }
        fn open(&self, _pid: u32) -> Result<FakeMem> {
            bail!("no access")
        }
        fn is_alive(&self, pid: u32) -> bool {
            self.pid == Some(pid)
        }
        fn find_image(&self, _pid: u32, _mem: &FakeMem) -> Result<Image> {
            Err(anyhow!("no image"))
        }
        fn heap_regions(&self, _pid: u32) -> Result<Vec<Range<u64>>> {
            Ok(Vec::new())
        }
        fn save_dir(&self, _pid: u32) -> Option<PathBuf> {
            None
        }
        fn access_hint(&self, _err: &anyhow::Error) -> Option<String> {
            None
        }
    }

    #[test]
    fn a_detached_reader_waits_and_reads_nothing() {
        for pid in [None, Some(42)] {
            let mut r = MemoryReader::new(FakePlatform { pid }, None);
            assert!(matches!(r.attach(), Attach::Waiting), "{pid:?}");
            assert!(r.snapshot(None, None).is_err());
            assert_eq!(r.quest_flags(true), &[] as &[u16]);
            r.forget_quests();
            r.locate_items_now();
            assert!(!r.is_alive());
            assert_eq!(r.save_dir(), None);
            let mut lines = Vec::new();
            assert!(r.attach_now(&mut |k, v| lines.push((k, v))).is_err());
            assert_eq!(
                lines.len(),
                usize::from(pid.is_some()),
                "only the pid is reported"
            );
        }
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
        t.search(&FakePlatform::default(), 1);
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
}

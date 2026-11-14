//! Live mode reader thread: follows the game process and sends rendered frames to the UI loop.
//!
//! Waits for D2R, opens its memory, locates the image and unit table once per process, then
//! snapshots and renders ~5 times a second. Each snapshot is folded into a [`Session`]
//! (superunique kills); unchanged results are not re-rendered or sent. The overlay is
//! hidden in towns and outside a game. The item-code table is searched once per process, in a
//! background thread, when the first item lies on the ground.

use std::io;
use std::path::Path;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use smithay_client_toolkit::reexports::calloop::channel::Sender;

use crate::game::{self, Snapshot};
use crate::journal::Journal;
use crate::overlay::Msg;
use crate::process::ProcMem;
use crate::session::Session;
use crate::{image, items, offset_cache, process, render};

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

fn send(tx: &Sender<Msg>, msg: Msg) -> Result<(), UiGone> {
    tx.send(msg).map_err(|_| UiGone)
}

/// Runs until the UI is gone. `player` picks among several player units; `size` is the frame size;
/// `journal` records what is seen (see [`crate::journal`]).
pub fn run(tx: &Sender<Msg>, player: Option<&str>, size: (u32, u32), mut journal: Journal) {
    let cache = offset_cache::default_path();
    // The last open error printed: a repeat isn't printed again, a different one is.
    let mut open_err = String::new();
    let mut waiting_logged = false;
    loop {
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
            follow_game(tx, pid, &mem, table, player, size, &mut journal)
        });
        // The process is gone (or never got a table): hide until the next one is in a game.
        if follow.and_then(|()| send(tx, Msg::Hide)).is_err() {
            return;
        }
        eprintln!("D2R (pid {pid}) exited");
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

/// Snapshots at [`TICK`] until the process exits; `Ok(())` then, `Err` if the UI is gone.
fn follow_game(
    tx: &Sender<Msg>,
    pid: u32,
    mem: &ProcMem,
    table: u64,
    player: Option<&str>,
    (w, h): (u32, u32),
    journal: &mut Journal,
) -> Result<(), UiGone> {
    let mut session = Session::default();
    let mut dedup = Dedup::default();
    let mut items_table = ItemsTable::NotSearched;
    loop {
        let start = Instant::now();
        match game::snapshot(mem, table, items_table.base(), player) {
            Ok(mut snap) => {
                if snap.ground_items > 0 {
                    items_table.search(pid);
                }
                // Before `apply`: it drops the monster presets the journal wants to see.
                journal.observe(&snap);
                let update = session.apply(&mut snap);
                if let Some(arrival) = update.arrival {
                    journal.arrival(&snap, arrival);
                }
                if update.new_game {
                    eprintln!("new game (player id or act changed): superunique kills cleared");
                }
                if game::is_town(snap.level.id) {
                    if dedup.hide(Hidden::Town) {
                        send(tx, Msg::Hide)?;
                    }
                } else if let Some(snap) = dedup.changed(snap) {
                    match render::render(snap, w, h) {
                        Ok(frame) => send(tx, Msg::Frame(frame))?,
                        Err(e) => eprintln!("rendering failed: {e:#}"),
                    }
                }
            }
            Err(e) => {
                if !is_alive(pid) {
                    return Ok(());
                }
                if dedup.hide(Hidden::NoGame) {
                    eprintln!("no game state (menu or loading?): {e:#}");
                    send(tx, Msg::Hide)?;
                }
            }
        }
        thread::sleep(TICK.saturating_sub(start.elapsed()));
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

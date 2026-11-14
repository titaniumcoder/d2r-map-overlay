//! Live mode reader thread: follows the game process and sends rendered frames to the UI loop.
//!
//! Waits for D2R, opens its memory, locates the image and unit table once per process, then
//! snapshots and renders ~5 times a second. Unchanged snapshots are not re-rendered or sent.

use std::io;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use smithay_client_toolkit::reexports::calloop::channel::Sender;

use crate::game::{self, Snapshot};
use crate::overlay::Msg;
use crate::process::ProcMem;
use crate::{image, offset_cache, process, render};

/// Snapshot interval (~5 Hz).
const TICK: Duration = Duration::from_millis(200);
/// Wait between attempts to find the game, open its memory or find the unit table.
const RETRY: Duration = Duration::from_secs(2);

/// The UI loop dropped its end of the channel: stop.
#[derive(Debug)]
struct UiGone;

fn send(tx: &Sender<Msg>, msg: Msg) -> Result<(), UiGone> {
    tx.send(msg).map_err(|_| UiGone)
}

/// Runs until the UI is gone. `player` picks among several player units; `size` is the frame size.
pub fn run(tx: &Sender<Msg>, player: Option<&str>, size: (u32, u32)) {
    let cache = offset_cache::default_path();
    let mut ptrace_hint_shown = false;
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
                if !ptrace_hint_shown {
                    eprintln!("{e:#}");
                    if is_permission_denied(&e) {
                        eprintln!("{}", ptrace_hint());
                    }
                    ptrace_hint_shown = true;
                }
                thread::sleep(RETRY);
                continue;
            }
        };
        eprintln!("attached to D2R (pid {pid})");
        waiting_logged = false;
        let follow = find_table(pid, &mem, cache.as_deref())
            .map_or(Ok(()), |table| follow_game(tx, pid, &mem, table, player, size));
        // The process is gone (or never got a table): hide until the next one is in a game.
        if follow.and_then(|()| send(tx, Msg::Hide)).is_err() {
            return;
        }
        eprintln!("D2R (pid {pid}) exited");
    }
}

/// Finds the unit table, retrying while the process lives (the structural search needs a game
/// in progress). `None` once the process is gone.
fn find_table(pid: u32, mem: &ProcMem, cache: Option<&Path>) -> Option<u64> {
    let mut last_err = String::new();
    while is_alive(pid) {
        let found = process::read_maps(pid).and_then(|maps| {
            let img = image::find_image(&maps, mem)?;
            img.unit_table(mem, cache)
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
        thread::sleep(RETRY);
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
) -> Result<(), UiGone> {
    let mut dedup = Dedup::default();
    loop {
        let start = Instant::now();
        match game::snapshot(mem, table, player) {
            Ok(snap) => {
                if let Some(snap) = dedup.changed(snap) {
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
                if dedup.failed() {
                    eprintln!("no game state (menu or loading?): {e:#}");
                    send(tx, Msg::Hide)?;
                }
            }
        }
        thread::sleep(TICK.saturating_sub(start.elapsed()));
    }
}

/// Suppresses repeated work: the same snapshot twice in a row is not re-rendered, and a run of
/// failed snapshots sends one [`Msg::Hide`].
#[derive(Debug, Default)]
struct Dedup {
    /// The last snapshot sent as a frame; `None` after a failure (the UI is hidden then).
    last: Option<Snapshot>,
    /// The previous tick failed (a hide was already sent).
    failing: bool,
}

impl Dedup {
    /// The snapshot to render, or `None` if it equals the one on screen.
    fn changed(&mut self, snap: Snapshot) -> Option<&Snapshot> {
        self.failing = false;
        if self.last.as_ref() == Some(&snap) {
            return None;
        }
        Some(self.last.insert(snap))
    }

    /// Records a failed snapshot; true if this starts a failure run (send a hide).
    fn failed(&mut self) -> bool {
        // Forget the frame: after the hide, even an identical snapshot must be sent again.
        self.last = None;
        !std::mem::replace(&mut self.failing, true)
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
        assert!(d.failed(), "first failure hides");
        assert!(!d.failed(), "later failures don't repeat the hide");
        assert!(
            d.changed(fixture()?).is_some(),
            "same snapshot after a hide is sent again"
        );
        assert!(d.changed(fixture()?).is_none());
        assert!(d.failed(), "a new failure run hides again");
        Ok(())
    }

    #[test]
    fn permission_denied_is_detected_through_context() {
        let e = anyhow::Error::from(io::Error::from(io::ErrorKind::PermissionDenied)).context("opening mem");
        assert!(is_permission_denied(&e));
        let e = anyhow::Error::from(io::Error::from(io::ErrorKind::NotFound)).context("opening mem");
        assert!(!is_permission_denied(&e));
    }
}

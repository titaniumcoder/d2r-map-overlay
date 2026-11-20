//! Hyprland focus tracking: is the D2R window the active one?
//!
//! Follows the event socket (`.socket2.sock`, `activewindow>>CLASS,TITLE` lines) and asks the
//! command socket (`.socket.sock`, `j/activewindow`) once per connection for the starting state.
//! Only focus changes are sent on. Socket I/O stays thin; the line parsing is unit-tested.

use std::io::{BufRead, BufReader, Read, Write};
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use serde::Deserialize;
use smithay_client_toolkit::reexports::calloop::channel::Sender;

use crate::overlay::Msg;

/// Window class of the game under Steam/Proton.
const D2R_CLASS: &str = "steam_app_battlenet";
/// Window title of the game.
const D2R_TITLE: &str = "Diablo II: Resurrected";
/// Wait before reconnecting after the event socket closed or failed.
const RECONNECT: Duration = Duration::from_secs(1);
/// Wait while the same error repeats.
const RECONNECT_MAX: Duration = Duration::from_secs(5);
/// Longest wait for the command socket's answer.
const QUERY_TIMEOUT: Duration = Duration::from_secs(2);

/// Sends [`Msg::Focus`] on every focus change until the UI is gone. Without Hyprland (env
/// missing) it says so once, reports "focused" and returns.
pub fn watch(tx: &Sender<Msg>) {
    let Some(dir) = socket_dir() else {
        eprintln!("not running under Hyprland ($HYPRLAND_INSTANCE_SIGNATURE unset): overlay always shown");
        // If the UI is already gone there is nobody left to tell.
        let _ = tx.send(Msg::Focus(true));
        return;
    };
    let mut last = None;
    let mut reconnect = Reconnect::default();
    loop {
        let wait = match follow(&dir, tx, &mut last, &mut reconnect) {
            Ok(()) => return, // UI gone
            Err(e) => {
                let (log, wait) = reconnect.failed(format!("{e:#}"));
                if let Some(msg) = log {
                    eprintln!("hyprland socket: {msg}; reconnecting");
                }
                wait
            }
        };
        thread::sleep(wait);
    }
}

/// Reconnect pacing: an error is logged only when its text differs from the previous one since
/// the last good connection, and a repeat waits [`RECONNECT_MAX`] instead of [`RECONNECT`].
#[derive(Debug, Default)]
struct Reconnect {
    last: Option<String>,
}

impl Reconnect {
    /// The line to log (if any) and the wait before the next attempt.
    fn failed(&mut self, err: String) -> (Option<String>, Duration) {
        if self.last.as_ref() == Some(&err) {
            return (None, RECONNECT_MAX);
        }
        self.last = Some(err.clone());
        (Some(err), RECONNECT)
    }

    /// A connection worked: the next failure is logged and retried quickly again.
    fn connected(&mut self) {
        self.last = None;
    }
}

/// `$XDG_RUNTIME_DIR/hypr/$HYPRLAND_INSTANCE_SIGNATURE`, if both are set.
fn socket_dir() -> Option<PathBuf> {
    let runtime = std::env::var_os("XDG_RUNTIME_DIR")?;
    let sig = std::env::var_os("HYPRLAND_INSTANCE_SIGNATURE")?;
    Some(Path::new(&runtime).join("hypr").join(sig))
}

/// Follows one event-socket connection. `Ok(())` means the UI is gone; `Err` means reconnect.
fn follow(dir: &Path, tx: &Sender<Msg>, last: &mut Option<bool>, reconnect: &mut Reconnect) -> Result<()> {
    // Connect to the event socket before asking, so no change between the two is lost.
    let events = UnixStream::connect(dir.join(".socket2.sock")).context("connecting to .socket2.sock")?;
    let initial = query_active(&dir.join(".socket.sock"))?;
    reconnect.connected();
    if !report(tx, last, initial) {
        return Ok(());
    }
    for line in BufReader::new(events).lines() {
        let line = line.context("reading .socket2.sock")?;
        if let Some(focused) = parse_event(&line)
            && !report(tx, last, focused)
        {
            return Ok(());
        }
    }
    bail!("Hyprland closed .socket2.sock")
}

/// Sends `focused` if it differs from `last`. False if the UI is gone.
fn report(tx: &Sender<Msg>, last: &mut Option<bool>, focused: bool) -> bool {
    if *last == Some(focused) {
        return true;
    }
    *last = Some(focused);
    tx.send(Msg::Focus(focused)).is_ok()
}

/// Asks Hyprland for the active window (JSON) and returns whether it is D2R.
fn query_active(socket: &Path) -> Result<bool> {
    let mut s = UnixStream::connect(socket).with_context(|| format!("connecting to {}", socket.display()))?;
    s.set_read_timeout(Some(QUERY_TIMEOUT))?;
    s.write_all(b"j/activewindow").context("sending j/activewindow")?;
    let mut reply = String::new();
    s.read_to_string(&mut reply)
        .context("reading the activewindow reply")?;
    parse_active_json(&reply)
}

/// Parses a `.socket2.sock` line: `Some(focused)` for `activewindow>>CLASS,TITLE`, `None` for
/// any other event. Hyprland splits at the first comma, so a title may itself contain commas.
fn parse_event(line: &str) -> Option<bool> {
    let rest = line.strip_prefix("activewindow>>")?;
    let (class, title) = rest.split_once(',').unwrap_or((rest, ""));
    Some(is_d2r(class, title))
}

/// The two fields of `j/activewindow` we need; Hyprland answers `{}` when no window is active.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ActiveWindow {
    class: String,
    title: String,
}

/// Parses the `j/activewindow` reply.
fn parse_active_json(reply: &str) -> Result<bool> {
    let w: ActiveWindow = serde_json::from_str(reply).with_context(|| {
        format!(
            "unexpected activewindow reply {:?}",
            reply.get(..200).unwrap_or(reply)
        )
    })?;
    Ok(is_d2r(&w.class, &w.title))
}

fn is_d2r(class: &str, title: &str) -> bool {
    class == D2R_CLASS && title == D2R_TITLE
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events() {
        assert_eq!(
            parse_event("activewindow>>steam_app_battlenet,Diablo II: Resurrected"),
            Some(true)
        );
        assert_eq!(
            parse_event("activewindow>>steam_app_battlenet,Battle.net"),
            Some(false)
        );
        assert_eq!(
            parse_event("activewindow>>kitty,Diablo II: Resurrected"),
            Some(false)
        );
        // Split at the first comma: the title keeps its own commas.
        assert_eq!(parse_event("activewindow>>firefox,a, b, c"), Some(false));
        assert_eq!(
            parse_event("activewindow>>steam_app_battlenet,Diablo II: Resurrected, again"),
            Some(false)
        );
        // Nothing focused (empty workspace).
        assert_eq!(parse_event("activewindow>>,"), Some(false));
        assert_eq!(parse_event("activewindow>>"), Some(false));
        // Other events, including the v2 variant, are ignored.
        assert_eq!(parse_event("activewindowv2>>55d1e0a0b0c0"), None);
        assert_eq!(parse_event("workspace>>2"), None);
        assert_eq!(parse_event(""), None);
    }

    #[test]
    fn reconnect_logs_each_error_once_and_backs_off() {
        let mut r = Reconnect::default();
        assert_eq!(
            r.failed("no socket".into()),
            (Some("no socket".into()), RECONNECT)
        );
        assert_eq!(r.failed("no socket".into()), (None, RECONNECT_MAX));
        assert_eq!(r.failed("no socket".into()), (None, RECONNECT_MAX));
        assert_eq!(r.failed("refused".into()), (Some("refused".into()), RECONNECT));
        assert_eq!(r.failed("refused".into()), (None, RECONNECT_MAX));
        r.connected();
        assert_eq!(r.failed("refused".into()), (Some("refused".into()), RECONNECT));
    }

    #[test]
    fn initial_state() -> Result<()> {
        let d2r = r#"{"address": "0x55d1", "class": "steam_app_battlenet",
                      "title": "Diablo II: Resurrected", "pid": 1234, "floating": false}"#;
        assert!(parse_active_json(d2r)?);
        assert!(!parse_active_json(
            r#"{"class": "kitty", "title": "Diablo II: Resurrected"}"#
        )?);
        assert!(!parse_active_json("{}")?);
        assert!(parse_active_json("unknown request").is_err());
        Ok(())
    }
}

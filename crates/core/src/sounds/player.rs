//! Plays the sounds through the desktop's audio server: a WAV piped into `pw-play` (`PipeWire`),
//! else `paplay` (`PulseAudio`), else `aplay` (ALSA), one after another on a background thread so
//! the live loop never waits.

use std::io::{self, Write};
use std::process::{Command, Stdio};
use std::sync::OnceLock;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread;
use std::time::{Duration, Instant};

use super::{Sound, synth};

/// A sound that waited longer than this behind the ones playing is dropped, not piled up.
const BACKLOG: Duration = Duration::from_secs(2);

/// A player still running this long after a sound under 0.6 s is stuck (an audio server that
/// never starts the stream) and is killed, so the queue moves on.
const HANG: Duration = Duration::from_secs(5);

/// The players tried in order, each reading a WAV from stdin.
const PLAYERS: [(&str, &[&str]); 3] = [("pw-play", &["-"]), ("paplay", &[]), ("aplay", &["-q", "-"])];

/// The player thread's queue; `None` if the thread couldn't start.
static QUEUE: OnceLock<Option<Sender<(Sound, Instant)>>> = OnceLock::new();

/// Queues `sound` behind the ones still playing.
pub fn play(sound: Sound) {
    let queue = QUEUE.get_or_init(|| {
        let (tx, rx) = mpsc::channel();
        match thread::Builder::new()
            .name("sounds".into())
            .spawn(move || run(&rx))
        {
            Ok(_) => Some(tx),
            Err(e) => {
                eprintln!("sounds: could not start the player thread ({e}); sounds are off");
                None
            }
        }
    });
    if let Some(tx) = queue {
        // Only fails once the thread is gone, which already logged.
        let _ = tx.send((sound, Instant::now()));
    }
}

fn run(rx: &Receiver<(Sound, Instant)>) {
    // The first player found is kept; `None` once none is installed.
    let mut player = Some(0);
    // A broken player fails every sound: logged once until one plays again.
    let mut failing = false;
    for (sound, queued) in rx {
        let Some(from) = player else { continue };
        if stale(queued, Instant::now()) {
            continue;
        }
        let wav = synth::wav(&synth::pcm(sound));
        match play_wav(&wav, from) {
            Played::By(index) => {
                player = Some(index);
                failing = false;
            }
            Played::Failed(index, e) => {
                player = Some(index);
                if !failing {
                    eprintln!("sounds: playing {} failed: {e}", sound.name());
                }
                failing = true;
            }
            Played::Missing => {
                eprintln!("sounds: no pw-play, paplay or aplay found; sounds are off");
                player = None;
            }
        }
    }
}

/// The outcome of one play, with the index of the player in [`PLAYERS`] that ran (the last
/// one that failed, when none played).
enum Played {
    By(usize),
    Failed(usize, String),
    Missing,
}

/// Plays `wav` with the first player from `PLAYERS[from..]` that works: a missing one or one
/// that fails (`pw-play` installed without a running `PipeWire`) hands over to the next.
fn play_wav(wav: &[u8], from: usize) -> Played {
    let mut failed = None;
    for (index, (program, args)) in PLAYERS.iter().enumerate().skip(from) {
        match pipe(program, args, wav) {
            Err(e) if e.kind() == io::ErrorKind::NotFound => {}
            Err(e) => failed = Some(Played::Failed(index, format!("{program}: {e}"))),
            Ok(()) => return Played::By(index),
        }
    }
    failed.unwrap_or(Played::Missing)
}

/// Runs `program` with `wav` on its stdin until it exits, or kills it after [`HANG`].
fn pipe(program: &str, args: &[&str], wav: &[u8]) -> io::Result<()> {
    let mut child = Command::new(program)
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()?;
    let deadline = Instant::now() + HANG;
    // A scoped thread writes stdin, so a player that never reads can't block past the deadline
    // (killing it breaks the pipe and ends the write). Dropping stdin at the end of the write
    // closes the pipe: the player sees the end and finishes.
    let stdin = child.stdin.take();
    let (status, written) = thread::scope(|scope| {
        let writer = scope.spawn(move || stdin.map_or(Ok(()), |mut stdin| stdin.write_all(wav)));
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Ok(status),
                Ok(None) if Instant::now() > deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    break Err(io::Error::other(format!("still playing after {HANG:?}, killed")));
                }
                Ok(None) => thread::sleep(Duration::from_millis(20)),
                Err(e) => break Err(e),
            }
        };
        let written = writer
            .join()
            .unwrap_or_else(|_| Err(io::Error::other("stdin writer panicked")));
        (status, written)
    });
    let status = status?;
    written?;
    if status.success() {
        Ok(())
    } else {
        Err(io::Error::other(format!("exited with {status}")))
    }
}

fn stale(queued: Instant, now: Instant) -> bool {
    now.saturating_duration_since(queued) > BACKLOG
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_backlog_over_two_seconds_is_dropped() {
        let t = Instant::now();
        assert!(!stale(t, t));
        assert!(!stale(t, t + Duration::from_millis(1900)));
        assert!(stale(t, t + Duration::from_millis(2100)));
        assert!(!stale(t + Duration::from_secs(1), t), "a clock step back");
    }

    #[test]
    fn the_wav_reaches_stdin_and_a_failing_exit_is_an_error() {
        let wav = synth::wav(&synth::pcm(Sound::TradeDrop));
        let expect = format!("test \"$(wc -c)\" -eq {}", wav.len());
        assert!(
            pipe("sh", &["-c", &expect], &wav).is_ok(),
            "the whole WAV on stdin"
        );
        let failed = pipe("sh", &["-c", "cat >/dev/null; exit 3"], &wav);
        assert!(failed.is_err_and(|e| e.to_string().contains("exit status: 3")));
    }

    #[test]
    fn a_missing_program_is_not_found() {
        let missing = pipe("d2r-map-overlay-no-such-player", &[], b"");
        assert!(missing.is_err_and(|e| e.kind() == io::ErrorKind::NotFound));
    }
}

//! Read-only Diablo II: Resurrected minimap overlay for Hyprland: wires one reader plugin and
//! one playstyle plugin, picked by Cargo features, into the core.

#[cfg(not(feature = "reader-proton"))]
compile_error!("no reader plugin: enable the `reader-proton` feature");
#[cfg(not(feature = "playstyle-config"))]
compile_error!("no playstyle plugin: enable the `playstyle-config` feature");

use std::thread;

use anyhow::{Context, Result, anyhow, bail};
use d2r_core::overlay::Msg;
use d2r_core::playstyle::{Files, Playstyle};
use d2r_core::reader::GameReader;
use d2r_core::{config, game, guidance, hypr, journal, live, overlay, render, session};
use smithay_client_toolkit::reexports::calloop::channel;

type TheReader = d2r_reader_mem::ProtonReader;

type ThePlaystyle = d2r_playstyle_config::ConfigDriven;

/// px.
const DEFAULT_SIZE: (u32, u32) = (400, 400);

const USAGE: &str = "usage: d2r-map-overlay [--player NAME] [--size WxH] [--no-journal]   (live overlay)\n       \
                     d2r-map-overlay --probe | --dump [--player NAME] \
                     | --render SNAPSHOT.json OUT.png [WxH] | --show SNAPSHOT.json [WxH]";

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let setup = || {
        let config = config::load();
        let files = Files::new(&config.playstyle);
        (config, files)
    };
    match args.as_slice() {
        _ if args
            .first()
            .is_none_or(|a| ["--player", "--size", "--no-journal"].contains(&a.as_str())) =>
        {
            let (player, size, journal) = parse_live_args(&args)?;
            let (config, files) = setup();
            live(player, size, journal, config, files)
        }
        [flag] if flag == "--probe" => probe(),
        [flag] if flag == "--dump" => dump(None, &setup().1),
        [flag, opt, name] if flag == "--dump" && opt == "--player" => dump(Some(name), &setup().1),
        [flag, snap, out] if flag == "--render" => render_png(snap, out, DEFAULT_SIZE, &setup()),
        [flag, snap, out, size] if flag == "--render" => render_png(snap, out, parse_size(size)?, &setup()),
        [flag, snap] if flag == "--show" => show(snap, DEFAULT_SIZE, &setup()),
        [flag, snap, size] if flag == "--show" => show(snap, parse_size(size)?, &setup()),
        _ => bail!("{USAGE}"),
    }
}

/// `--player NAME`, `--size WxH` and `--no-journal`, in any order: (player, size, journal on).
fn parse_live_args(args: &[String]) -> Result<(Option<String>, (u32, u32), bool)> {
    let mut player = None;
    let mut size = DEFAULT_SIZE;
    let mut journal = true;
    let mut it = args.iter();
    while let Some(flag) = it.next() {
        if flag == "--no-journal" {
            journal = false;
            continue;
        }
        let value = it
            .next()
            .with_context(|| format!("{flag} needs a value\n{USAGE}"))?;
        match flag.as_str() {
            "--player" => player = Some(value.clone()),
            "--size" => size = parse_size(value)?,
            _ => bail!("unknown option {flag:?}\n{USAGE}"),
        }
    }
    Ok((player, size, journal))
}

/// A reader thread and a focus thread feed the UI loop on this thread until killed.
fn live(
    player: Option<String>,
    size: (u32, u32),
    journal: bool,
    config: config::Config,
    files: Files,
) -> Result<()> {
    let journal = match journal::default_dir() {
        Some(dir) if journal => {
            eprintln!("journal: {}", dir.display());
            journal::Journal::new(dir)
        }
        Some(_) => journal::Journal::disabled(),
        None => {
            eprintln!("journal off: neither $XDG_DATA_HOME nor $HOME is set");
            journal::Journal::disabled()
        }
    };
    let (tx, rx) = channel::channel();
    let reader_tx = tx.clone();
    thread::Builder::new()
        .name("reader".into())
        .spawn(move || {
            let _exit = ExitOnDrop;
            let mut reader = TheReader::default();
            let mut playstyle = ThePlaystyle::load(&files);
            live::run(
                &mut reader,
                &mut playstyle,
                &reader_tx,
                player.as_deref(),
                size,
                journal,
                &config,
                &files,
            );
        })
        .context("starting the reader thread")?;
    thread::Builder::new()
        .name("hypr".into())
        .spawn(move || hypr::watch(&tx))
        .context("starting the focus thread")?;
    overlay::run(rx, size)
}

/// Exits the process if the reader thread panics: a panic otherwise ends only that thread and
/// leaves a frozen frame on screen.
#[derive(Debug)]
struct ExitOnDrop;

impl Drop for ExitOnDrop {
    fn drop(&mut self) {
        if thread::panicking() {
            eprintln!("reader thread panicked; exiting");
            std::process::exit(1);
        }
    }
}

fn probe() -> Result<()> {
    TheReader::default().attach_now(&mut |label, value| println!("{label:<12}{value}"))
}

/// Prints one JSON snapshot; as in live mode, only the bases the playstyle marks are kept.
fn dump(player: Option<&str>, files: &Files) -> Result<()> {
    let mut reader = TheReader::default();
    reader.attach_now(&mut |_, _| {})?;
    reader.locate_items_now();
    eprintln!("quest flags: {:04x?}", reader.quest_flags(true));
    let mut snap = reader.snapshot(player, None)?;
    let playstyle = ThePlaystyle::load(files);
    let wants = playstyle.evaluate(&snap, None).wants;
    let marks = playstyle.mark_drops(&mut snap.drops, &wants);
    if !marks.is_empty() {
        eprintln!("marks: {marks:?}");
    }
    println!("{}", serde_json::to_string_pretty(&snap)?);
    Ok(())
}

/// `WxH`, both sides 1..=8192.
fn parse_size(s: &str) -> Result<(u32, u32)> {
    let (w, h) = s
        .split_once(['x', 'X'])
        .with_context(|| format!("size {s:?} is not WxH"))?;
    let w: u32 = w.parse().with_context(|| format!("bad width in {s:?}"))?;
    let h: u32 = h.parse().with_context(|| format!("bad height in {s:?}"))?;
    if !(1..=8192).contains(&w) || !(1..=8192).contains(&h) {
        bail!("size {s:?} out of range (1..=8192 per side)");
    }
    Ok((w, h))
}

fn load_snapshot(path: &str) -> Result<game::Snapshot> {
    let json = std::fs::read_to_string(path).with_context(|| format!("reading {path}"))?;
    serde_json::from_str(&json).with_context(|| format!("parsing snapshot {path}"))
}

/// Loads a `--dump` snapshot, filtered as in live mode, with the guidance the map draws.
fn load_for_drawing(
    path: &str,
    (config, files): &(config::Config, Files),
) -> Result<(game::Snapshot, guidance::Guidance)> {
    let mut snap = load_snapshot(path)?;
    let playstyle = ThePlaystyle::load(files);
    session::Session::default().apply(&mut snap);
    config.shrines.retain_shown(&mut snap.shrines, snap.difficulty);
    let evaluation = playstyle.evaluate(&snap, None);
    let marks = playstyle.mark_drops(&mut snap.drops, &evaluation.wants);
    let mut guidance = evaluation.guidance;
    guidance.marks = marks;
    Ok((snap, guidance))
}

fn render_png(snap: &str, out: &str, (w, h): (u32, u32), setup: &(config::Config, Files)) -> Result<()> {
    let (snap, guidance) = load_for_drawing(snap, setup)?;
    let pixmap = render::render(&snap, &guidance, w, h)?;
    pixmap.save_png(out).with_context(|| format!("writing {out}"))?;
    println!("wrote {out} ({w}x{h})");
    Ok(())
}

/// Shows a `--dump` snapshot in the overlay window until killed, focus ignored.
fn show(snap: &str, (w, h): (u32, u32), setup: &(config::Config, Files)) -> Result<()> {
    let (snap, guidance) = load_for_drawing(snap, setup)?;
    let frame = render::render(&snap, &guidance, w, h)?;
    let (tx, rx) = channel::channel();
    for msg in [Msg::Frame(frame), Msg::Focus(true)] {
        tx.send(msg).map_err(|_| anyhow!("overlay channel closed"))?;
    }
    // `tx` stays alive until `run` returns, so the loop never sees the channel close.
    let shown = overlay::run(rx, (w, h));
    drop(tx);
    shown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sizes() -> Result<()> {
        assert_eq!(parse_size("400x300")?, (400, 300));
        assert_eq!(parse_size("64X64")?, (64, 64));
        for bad in ["400", "x300", "400x", "0x10", "10x0", "-1x5", "9000x10", "axb"] {
            assert!(parse_size(bad).is_err(), "{bad} accepted");
        }
        Ok(())
    }

    fn strings(args: &[&str]) -> Vec<String> {
        args.iter().map(|&a| a.to_owned()).collect()
    }

    #[test]
    fn parses_live_args() -> Result<()> {
        assert_eq!(parse_live_args(&[])?, (None, DEFAULT_SIZE, true));
        assert_eq!(
            parse_live_args(&strings(&["--size", "300x200", "--player", "BigWolf"]))?,
            (Some("BigWolf".to_owned()), (300, 200), true)
        );
        assert_eq!(
            parse_live_args(&strings(&["--no-journal", "--size", "300x200"]))?,
            (None, (300, 200), false)
        );
        assert!(parse_live_args(&strings(&["--player"])).is_err());
        assert!(parse_live_args(&strings(&["--size", "big"])).is_err());
        assert!(parse_live_args(&strings(&["--player", "a", "--bogus", "b"])).is_err());
        Ok(())
    }
}

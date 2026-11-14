//! Read-only Diablo II: Resurrected minimap overlay for Hyprland.
//!
//! See `PLAN.md` for the architecture and milestones.

mod game;
mod image;
mod offset_cache;
mod offsets;
mod overlay;
mod process;
mod render;
#[cfg(test)]
mod testutil;

use anyhow::{Context, Result, bail};

/// Overlay / render size in px when none is given.
const DEFAULT_SIZE: (u32, u32) = (400, 400);

fn main() -> Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.as_slice() {
        [] => {
            println!("d2r-map-overlay: not implemented yet (see PLAN.md)");
            Ok(())
        }
        [flag] if flag == "--probe" => probe(),
        [flag] if flag == "--dump" => dump(None),
        [flag, opt, name] if flag == "--dump" && opt == "--player" => dump(Some(name)),
        [flag, snap, out] if flag == "--render" => render_png(snap, out, DEFAULT_SIZE),
        [flag, snap, out, size] if flag == "--render" => render_png(snap, out, parse_size(size)?),
        [flag, snap] if flag == "--show" => show(snap, DEFAULT_SIZE),
        [flag, snap, size] if flag == "--show" => show(snap, parse_size(size)?),
        _ => bail!(
            "usage: d2r-map-overlay [--probe | --dump [--player NAME] | --render SNAPSHOT.json OUT.png [WxH] \
             | --show SNAPSHOT.json [WxH]]"
        ),
    }
}

/// Locates the game, its image and the unit table, and prints what it found.
fn probe() -> Result<()> {
    let pid = process::find_d2r_pid()?;
    println!("pid         {pid}");
    let maps = process::read_maps(pid)?;
    let mem = process::ProcMem::open(pid)?;
    let img = image::find_image(&maps, &mem).context("locating D2R.exe image")?;
    println!("live_base   {:#x}", img.live_base);
    println!("alias_base  {:#x}", img.alias_base);
    println!("size        {:#x}", img.size);
    let cache = offset_cache::default_path();
    let table = img.unit_table(&mem, cache.as_deref())?;
    println!("build       {}", table.build);
    println!(
        "unit_table  {:#x} (rva {:#x}, via {:?})",
        table.addr, table.rva, table.source
    );
    match &cache {
        Some(path) => println!("cache       {}", path.display()),
        None => println!("cache       (none: neither $XDG_CACHE_HOME nor $HOME set)"),
    }
    Ok(())
}

/// Prints one JSON snapshot of the game state; `player` picks among several live player units.
fn dump(player: Option<&str>) -> Result<()> {
    let pid = process::find_d2r_pid()?;
    let maps = process::read_maps(pid)?;
    let mem = process::ProcMem::open(pid)?;
    let img = image::find_image(&maps, &mem).context("locating D2R.exe image")?;
    let table = img.unit_table(&mem, offset_cache::default_path().as_deref())?;
    let snap = game::snapshot(&mem, table.addr, player)?;
    println!("{}", serde_json::to_string_pretty(&snap)?);
    Ok(())
}

/// Parses `WxH` (e.g. `400x300`); both sides must be 1..=8192.
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

/// Renders a `--dump` JSON snapshot to a PNG file.
fn render_png(snap: &str, out: &str, (w, h): (u32, u32)) -> Result<()> {
    let pixmap = render::render(&load_snapshot(snap)?, w, h)?;
    pixmap.save_png(out).with_context(|| format!("writing {out}"))?;
    println!("wrote {out} ({w}x{h})");
    Ok(())
}

/// Shows a `--dump` JSON snapshot in the overlay window until killed.
fn show(snap: &str, (w, h): (u32, u32)) -> Result<()> {
    overlay::show(render::render(&load_snapshot(snap)?, w, h)?)
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
}

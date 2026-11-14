//! Read-only Diablo II: Resurrected minimap overlay for Hyprland.
//!
//! See `PLAN.md` for the architecture and milestones.

mod game;
mod image;
mod offset_cache;
mod offsets;
mod process;
#[cfg(test)]
mod testutil;

use anyhow::{Context, Result, bail};

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
        _ => bail!("usage: d2r-map-overlay [--probe | --dump [--player NAME]]"),
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

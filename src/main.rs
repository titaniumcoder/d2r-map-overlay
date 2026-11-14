//! Read-only Diablo II: Resurrected minimap overlay for Hyprland.
//!
//! See `PLAN.md` for the architecture and milestones.

mod image;
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
        _ => bail!("usage: d2r-map-overlay [--probe]"),
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
    let table = img.unit_table(&mem)?;
    println!("unit_table  {table:#x}");
    Ok(())
}

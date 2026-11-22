//! Research probe for the UI state the in-game marks still lack: whether the stash window is
//! open, which of its tabs / shared pages it shows, and which store tab a trade window shows.
//! With `D2R_UI_PROBE` set, every tick in a town diffs `.data` (bytes that go between small
//! values, as panel flags and tab indices do) and the player's `Inventory` head (every byte)
//! against the last tick and appends the changes to `ui-probe.jsonl` in the data directory,
//! with the inventory items' grid cells for checking the cell read. `research/ui_probe.py`
//! reads it back.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, ensure};

use d2r_core::game::{Place, Snapshot, is_town};
use d2r_core::offsets::unit;
use d2r_core::reader::ReadMem;

use crate::image::Image;

/// The environment variable that turns the probe on.
pub const ENV: &str = "D2R_UI_PROBE";
/// A `.data` byte is logged only while both its old and new value are at most this.
const SMALL: u8 = 16;
/// After this many logged changes a `.data` byte is taken for a counter, not UI state: no longer logged.
const NOISY: u8 = 40;
/// `.data` changes per line; the rest are only counted.
const MAX_CHANGES: usize = 400;
/// Bytes of the player's `Inventory` struct compared.
const INVENTORY_LEN: usize = 0x100;
/// A larger `.data` is not copied every tick.
const MAX_DATA: usize = 32 << 20;

/// One changed byte: (offset, old, new).
type Change = (usize, u8, u8);

#[derive(Debug)]
pub struct UiProbe {
    out: File,
    path: PathBuf,
    image: Image,
    data_rva: u64,
    data: Vec<u8>,
    /// Logged changes per `.data` byte (saturating).
    counts: Vec<u8>,
    inventory: Vec<u8>,
    /// The last line's state fields, so an unchanged tick writes nothing.
    last_state: String,
}

impl UiProbe {
    /// Opens `ui-probe.jsonl` in `dir` (appending) and takes the first `.data` copy.
    ///
    /// # Errors
    /// `.data` can't be read or is too large, or the file can't be opened.
    pub fn open(image: Image, mem: &impl ReadMem, dir: &Path) -> Result<Self> {
        let (data_rva, len) = image.data_section(mem)?;
        ensure!(
            len <= MAX_DATA,
            ".data is {len:#x} bytes, too large to copy every tick"
        );
        let data = image.read_alias(mem, data_rva, len)?;
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
        let path = dir.join("ui-probe.jsonl");
        let mut out = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .with_context(|| format!("opening {}", path.display()))?;
        writeln!(
            out,
            r#"{{"t":{},"start":true,"data_rva":{data_rva},"data_len":{len}}}"#,
            now()
        )?;
        Ok(Self {
            out,
            path,
            image,
            data_rva,
            counts: vec![0; data.len()],
            data,
            inventory: Vec::new(),
            last_state: String::new(),
        })
    }

    #[must_use]
    pub const fn path(&self) -> &PathBuf {
        &self.path
    }

    /// One tick: a line when anything changed. Outside towns nothing is read.
    ///
    /// # Errors
    /// Writing the line failed (read failures only skip that part).
    pub fn tick(&mut self, mem: &impl ReadMem, snap: &Snapshot) -> Result<()> {
        if !is_town(snap.level.id) {
            return Ok(());
        }
        let mut data_changes = Vec::new();
        if let Ok(data) = self.image.read_alias(mem, self.data_rva, self.data.len()) {
            data_changes = small_changes(&self.data, &data, &mut self.counts);
            self.data = data;
        }
        let inventory = mem
            .read_ptr(snap.player.addr.0 + unit::INVENTORY)
            .ok()
            .flatten()
            .and_then(|inv| mem.read_bytes(inv, INVENTORY_LEN).ok())
            .unwrap_or_default();
        let inventory_changes = if inventory.len() == self.inventory.len() {
            changes(&self.inventory, &inventory)
        } else {
            Vec::new()
        };
        self.inventory = inventory;
        let state = state_fields(snap);
        if data_changes.is_empty() && inventory_changes.is_empty() && state == self.last_state {
            return Ok(());
        }
        let more = data_changes.len().saturating_sub(MAX_CHANGES);
        data_changes.truncate(MAX_CHANGES);
        let rva = |off: usize| self.data_rva + off as u64;
        let data = data_changes
            .iter()
            .map(|&(off, old, new)| format!("[{},{old},{new}]", rva(off)))
            .collect::<Vec<_>>()
            .join(",");
        writeln!(
            self.out,
            r#"{{"t":{},"level":{},{state},"data":[{data}],"more":{more},"inv":[{}]}}"#,
            now(),
            snap.level.id,
            json_changes(&inventory_changes),
        )?;
        self.last_state = state;
        Ok(())
    }
}

/// The trade window's item count, the shared stash's tab count and the inventory items'
/// codes with their read cells, as JSON fields.
fn state_fields(snap: &Snapshot) -> String {
    let shop = snap.shop.as_ref().map_or(0, |s| s.items.len());
    let cells = snap
        .owned_items
        .iter()
        .filter(|(_, place)| *place == Place::Inventory)
        .map(|(item, _)| {
            let code = item.code.as_deref().unwrap_or("?");
            item.cell
                .map_or_else(|| format!("{code}@-"), |c| format!("{code}@{},{}", c.x, c.y))
        })
        .collect::<Vec<_>>()
        .join(" ");
    format!(r#""shop":{shop},"tabs":{},"cells":"{cells}""#, snap.stash.len())
}

fn json_changes(changes: &[Change]) -> String {
    changes
        .iter()
        .map(|&(off, old, new)| format!("[{off},{old},{new}]"))
        .collect::<Vec<_>>()
        .join(",")
}

/// Every byte that differs.
fn changes(prev: &[u8], cur: &[u8]) -> Vec<Change> {
    prev.iter()
        .zip(cur)
        .enumerate()
        .filter(|(_, (a, b))| a != b)
        .map(|(i, (&a, &b))| (i, a, b))
        .collect()
}

/// The bytes that went from one value up to [`SMALL`] to another, unless they changed
/// [`NOISY`] times already; `counts` (one per byte) is updated.
fn small_changes(prev: &[u8], cur: &[u8], counts: &mut [u8]) -> Vec<Change> {
    let mut out = Vec::new();
    for (i, ((&old, &new), count)) in prev.iter().zip(cur).zip(counts.iter_mut()).enumerate() {
        if old == new || old > SMALL || new > SMALL || *count >= NOISY {
            continue;
        }
        *count = count.saturating_add(1);
        out.push((i, old, new));
    }
    out
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_small_values_until_noisy() {
        let mut counts = vec![0u8; 4];
        let prev = [0, 1, 200, 5];
        let cur = [1, 1, 3, 17];
        assert_eq!(
            small_changes(&prev, &cur, &mut counts),
            [(0, 0, 1)],
            "unchanged, from a large value and to a large value are left out"
        );
        let mut flips = Vec::new();
        let mut count = [0u8];
        for n in 0..50u8 {
            let (a, b) = if n % 2 == 0 { ([0u8], [1u8]) } else { ([1], [0]) };
            flips.extend(small_changes(&a, &b, &mut count));
        }
        assert_eq!(flips.len(), usize::from(NOISY), "a counter stops being logged");
    }

    #[test]
    fn every_inventory_byte_change() {
        assert_eq!(changes(&[1, 2, 0xFF], &[1, 3, 0]), [(1, 2, 3), (2, 0xFF, 0)]);
    }
}

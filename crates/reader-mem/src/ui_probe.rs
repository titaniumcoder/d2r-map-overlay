//! Research probe for the UI state the in-game marks still lack: whether the stash window is
//! open, which of its tabs / shared pages it shows, and which store tab a trade window shows.
//! With `D2R_UI_PROBE` set, every tick in a town diffs against the last tick and appends the
//! changes to `ui-probe.jsonl` in the data directory: `.data` bytes that go between small
//! values (as panel flags and tab indices do), `.data` qwords that go from one heap pointer to
//! another (a "shown tab" held as a pointer), every byte of the candidate panel-flag array
//! ([`ui_panels`]) and the flags it reads, small-value bytes of the heap objects that `.data`
//! qwords around that array point to, and every byte of the player's `Inventory` head; with
//! the inventory items' grid cells. `research/ui_probe.py` reads it back.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, ensure};

use d2r_core::game::{Place, Snapshot, is_town};
use d2r_core::offsets::{ui_panels, unit};
use d2r_core::reader::{ReadMem, is_user_ptr};

use crate::image::Image;

/// The environment variable that turns the probe on.
pub const ENV: &str = "D2R_UI_PROBE";
/// A `.data` byte is logged only while both its old and new value are at most this.
const SMALL: u8 = 16;
/// After this many logged changes a `.data` byte is taken for a counter, not UI state: no longer logged.
const NOISY: u8 = 40;
/// `.data` bytes on each side of the panel array whose qwords' heap objects are compared.
const NEAR_PANELS: u64 = 0x800;
/// Bytes compared of each such heap object.
const POINTEE_LEN: usize = 0x200;
/// Bytes of the player's `Inventory` struct compared.
const INVENTORY_LEN: usize = 0x100;
/// A larger `.data` is not copied every tick.
const MAX_DATA: usize = 32 << 20;

/// One changed byte: (offset, old, new).
type Change = (usize, u8, u8);

/// A heap object a `.data` qword near the panel array points to.
#[derive(Debug)]
struct Pointee {
    /// rva of the qword.
    slot: u64,
    ptr: u64,
    bytes: Vec<u8>,
    counts: Vec<u8>,
}

#[derive(Debug)]
pub struct UiProbe {
    out: File,
    path: PathBuf,
    image: Image,
    data_rva: u64,
    data: Vec<u8>,
    /// Logged changes per `.data` byte (saturating).
    counts: Vec<u8>,
    /// Logged pointer changes per `.data` qword (saturating).
    ptr_counts: Vec<u8>,
    pointees: Vec<Pointee>,
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
            ptr_counts: vec![0; data.len() / 8],
            pointees: Vec::new(),
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
        let mut ptr_changes = Vec::new();
        let mut panel_changes = Vec::new();
        if let Ok(data) = self.image.read_alias(mem, self.data_rva, self.data.len()) {
            data_changes = small_changes(&self.data, &data, &mut self.counts);
            let image = self.image.live_base..self.image.live_base + self.image.size;
            ptr_changes = pointer_changes(&self.data, &data, &mut self.ptr_counts, &image);
            let panels = self.panels_range();
            if let (Some(old), Some(new)) = (self.data.get(panels.clone()), data.get(panels)) {
                panel_changes = changes(old, new);
            }
            self.data = data;
        }
        let heap_changes = self.pointee_changes(mem);
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
        let open = self
            .data
            .get(self.panels_range())
            .map_or_else(String::new, open_panels);
        let state = format!(r#"{},"open":"{open}""#, state_fields(snap));
        if data_changes.is_empty()
            && ptr_changes.is_empty()
            && panel_changes.is_empty()
            && heap_changes.is_empty()
            && inventory_changes.is_empty()
            && state == self.last_state
        {
            return Ok(());
        }
        let rva = |off: usize| self.data_rva + off as u64;
        let data = data_changes
            .iter()
            .map(|&(off, old, new)| format!("[{},{old},{new}]", rva(off)))
            .collect::<Vec<_>>()
            .join(",");
        let ptrs = ptr_changes
            .iter()
            .map(|&(off, old, new)| format!("[{},{old},{new}]", rva(off)))
            .collect::<Vec<_>>()
            .join(",");
        let heap = heap_changes
            .iter()
            .map(|&(slot, (off, old, new))| format!("[{slot},{off},{old},{new}]"))
            .collect::<Vec<_>>()
            .join(",");
        writeln!(
            self.out,
            r#"{{"t":{},"level":{},{state},"data":[{data}],"ptrs":[{ptrs}],"panels":[{}],"heap":[{heap}],"inv":[{}]}}"#,
            now(),
            snap.level.id,
            json_changes(&panel_changes),
            json_changes(&inventory_changes),
        )?;
        self.last_state = state;
        Ok(())
    }
}

impl UiProbe {
    /// The panel array's bytes in the `.data` copy.
    fn panels_range(&self) -> std::ops::Range<usize> {
        let start = usize::try_from(ui_panels::RVA.saturating_sub(self.data_rva)).unwrap_or(usize::MAX);
        start..start.saturating_add(ui_panels::LEN)
    }

    /// The small-value byte changes of the heap objects the qwords within [`NEAR_PANELS`] of
    /// the panel array point to, as (qword rva, change). A qword that points elsewhere than
    /// last tick starts over (its pointer change is in the `ptrs` field).
    fn pointee_changes(&mut self, mem: &impl ReadMem) -> Vec<(u64, Change)> {
        let image = self.image.live_base..self.image.live_base + self.image.size;
        let first = ui_panels::RVA.saturating_sub(NEAR_PANELS) & !7;
        let last = ui_panels::RVA + ui_panels::LEN as u64 + NEAR_PANELS;
        let mut out = Vec::new();
        let mut kept = Vec::new();
        for slot in (first..last).step_by(8) {
            let Some(ptr) = slot
                .checked_sub(self.data_rva)
                .and_then(|off| usize::try_from(off).ok())
                .and_then(|off| self.data.get(off..off + 8))
                .and_then(|b| b.try_into().ok())
                .map(u64::from_le_bytes)
                .filter(|&p| is_heap_ptr(p, &image))
            else {
                continue;
            };
            let Ok(bytes) = mem.read_bytes(ptr, POINTEE_LEN) else {
                continue;
            };
            let old = self
                .pointees
                .iter()
                .position(|p| p.slot == slot && p.ptr == ptr)
                .map(|i| self.pointees.swap_remove(i));
            let mut counts = vec![0; POINTEE_LEN];
            if let Some(old) = old {
                counts = old.counts;
                out.extend(
                    small_changes(&old.bytes, &bytes, &mut counts)
                        .into_iter()
                        .map(|c| (slot, c)),
                );
            }
            kept.push(Pointee {
                slot,
                ptr,
                bytes,
                counts,
            });
        }
        self.pointees = kept;
        out
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

/// A user-space pointer outside the game image: a heap object.
fn is_heap_ptr(p: u64, image: &std::ops::Range<u64>) -> bool {
    is_user_ptr(p) && !image.contains(&p)
}

/// The 8-byte-aligned qwords that went from one heap pointer to another, unless they did
/// [`NOISY`] times already; `counts` (one per qword) is updated. The change is logged as
/// (byte offset, old, new).
fn pointer_changes(
    prev: &[u8],
    cur: &[u8],
    counts: &mut [u8],
    image: &std::ops::Range<u64>,
) -> Vec<(usize, u64, u64)> {
    let mut out = Vec::new();
    let pairs = prev.as_chunks::<8>().0.iter().zip(cur.as_chunks::<8>().0);
    for (i, ((&old, &new), count)) in pairs.zip(counts.iter_mut()).enumerate() {
        let (old, new) = (u64::from_le_bytes(old), u64::from_le_bytes(new));
        if old == new || !is_heap_ptr(old, image) || !is_heap_ptr(new, image) || *count >= NOISY {
            continue;
        }
        *count = count.saturating_add(1);
        out.push((i * 8, old, new));
    }
    out
}

/// The panel flags set in `panels` (the [`ui_panels`] array), space-separated.
fn open_panels(panels: &[u8]) -> String {
    [
        (ui_panels::INVENTORY, "inventory"),
        (ui_panels::NPC_INTERACT, "npc"),
        (ui_panels::NPC_SHOP, "shop"),
        (ui_panels::STASH, "stash"),
        (ui_panels::CUBE, "cube"),
    ]
    .into_iter()
    .filter(|&(at, _)| panels.get(at).is_some_and(|&b| b != 0))
    .map(|(_, name)| name)
    .collect::<Vec<_>>()
    .join(" ")
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
    fn heap_pointer_moves_only() {
        let image = 0x7000_0000_0000..0x7000_1000_0000;
        let q = |vals: &[u64]| vals.iter().flat_map(|v| v.to_le_bytes()).collect::<Vec<u8>>();
        let heap = 0x5c3c_1a70;
        let prev = q(&[heap, heap, 0x7000_0000_1000, 3, heap]);
        let cur = q(&[heap + 0x40, heap, heap, 4, 0]);
        let mut counts = vec![0u8; 5];
        assert_eq!(
            pointer_changes(&prev, &cur, &mut counts, &image),
            [(0, heap, heap + 0x40)],
            "unchanged, from an image address, small numbers and to null are left out"
        );
    }

    /// The panel array at two moments of the 2026-11-22 recordings, from the bytes that
    /// changed: Hratli's trade window opening (the NPC flag first) and the stash open.
    #[test]
    fn open_panels_from_the_recording() {
        let set = |on: &[usize]| {
            (0..ui_panels::LEN)
                .map(|i| u8::from(on.contains(&i)))
                .collect::<Vec<_>>()
        };
        assert_eq!(open_panels(&set(&[])), "");
        assert_eq!(open_panels(&set(&[0x08])), "npc");
        assert_eq!(open_panels(&set(&[0x01, 0x18])), "inventory stash");
        assert_eq!(open_panels(&[0, 1]), "inventory", "a short read sees what it has");
    }

    #[test]
    fn every_inventory_byte_change() {
        assert_eq!(changes(&[1, 2, 0xFF], &[1, 3, 0]), [(1, 2, 3), (2, 0xFF, 0)]);
    }
}

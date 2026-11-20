//! The reader plugin: finds the game and reads its memory into a [`game::Snapshot`].
//!
//! Live mode and `--probe` / `--dump` use it only through [`GameReader`]; this build's reader is
//! [`ProtonReader`]. It has two layers:
//!
//! - **Platform layer** ([`Platform`]): find the game process, open its memory ([`ReadMem`]),
//!   locate the D2R image, list the heap regions, find the save directory. Today [`Proton`]:
//!   [`process`] (`/proc` pid discovery, `/proc/<pid>/maps`, `/proc/<pid>/mem`), Wine's memfd
//!   alias of the encrypted image ([`image::find_image`]), the `WINEPREFIX` save directory.
//! - **Game layer** (any platform): [`offsets`], [`offset_cache`], the structure walk in
//!   [`game`] (generic over [`ReadMem`]), the unit-table and quest-global searches on an
//!   [`image::Image`], the items-table scan in [`items`], the quest record in [`quests`], and
//!   [`MemoryReader`], which drives them over any [`Platform`] (retries, the background items
//!   search, the quest search budget).
//!
//! # A Windows platform
//! A Windows build would add a `Platform` implementation and reuse the game layer:
//! - `find_game`: Toolhelp32 (`CreateToolhelp32Snapshot` + `Process32First/NextW`, exe name
//!   `D2R.exe`); `is_alive`: `OpenProcess` + `GetExitCodeProcess` (`STILL_ACTIVE`).
//! - `open` / `Mem`: `OpenProcess(PROCESS_VM_READ | PROCESS_QUERY_INFORMATION)` and
//!   `ReadProcessMemory`. That is FFI, so it needs `unsafe` (forbidden in this crate) or a
//!   wrapper crate that keeps the `unsafe` on its side (e.g. `read-process-memory`).
//! - `find_image`: the module base from `EnumProcessModulesEx` / `Module32FirstW`, with
//!   `live_base == alias_base`: Windows has no memfd alias. The encrypted code pages stay
//!   unreadable, so the code-signature scan ([`image::Image::unit_table`]) needs another
//!   source: read the readable sections only and rely on the per-build offset cache and the
//!   structural `.data` search (never encrypted), or a signature found elsewhere.
//! - `heap_regions`: `VirtualQueryEx`, keeping committed private `PAGE_READWRITE` regions.
//! - `save_dir`: `%USERPROFILE%\Saved Games\Diablo II Resurrected`.

pub mod game;
pub mod image;
pub mod items;
mod memory_reader;
pub mod offset_cache;
pub mod offsets;
pub mod process;
mod proton;
pub mod quests;

use std::fmt::Debug;
use std::io;
use std::ops::Range;
use std::path::PathBuf;

use anyhow::Result;

pub use memory_reader::MemoryReader;
pub use proton::Proton;

/// The reader of this build: the game layer over the Proton/Linux platform.
pub type ProtonReader = MemoryReader<Proton>;

/// Outcome of one [`GameReader::attach`] attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attach {
    /// No game process, or its memory can't be opened (logged): try again later.
    Waiting,
    /// Attached and the unit table located: snapshots can be read.
    Ready { pid: u32 },
    /// The process exited before the unit table was found.
    Exited { pid: u32 },
}

/// What the rest of the program needs from a game reader. One reader follows one game process
/// at a time; [`attach`](GameReader::attach) again after it exits.
pub trait GameReader {
    /// Live mode: one attempt to find the game and open its memory (failures are logged once
    /// per distinct error, with a hint when access is denied). Once open, locates the unit
    /// table, retrying with a backoff while the process lives (blocks until found or exited).
    fn attach(&mut self) -> Attach;

    /// `--probe` / `--dump`: one attempt, no waiting: find the game, open its memory, locate
    /// the image and the unit table. What was found is passed to `report` as `(label, value)`
    /// as it is found, so a failure still shows how far it got.
    ///
    /// # Errors
    /// Fails if any step fails.
    fn attach_now(&mut self, report: &mut dyn FnMut(&'static str, String)) -> Result<()>;

    /// Searches the item-code table now, blocking (`--dump`); in live mode [`snapshot`]
    /// starts the search in the background once items are seen.
    ///
    /// [`snapshot`]: GameReader::snapshot
    fn locate_items_now(&mut self);

    /// Reads one snapshot. `player` picks among several player units; `previous` is the unit
    /// id chosen before (see [`game::ChosenBy`]).
    ///
    /// # Errors
    /// Not attached, or no game state (menu, loading): see [`game::snapshot`].
    fn snapshot(&mut self, player: Option<&str>, previous: Option<u32>) -> Result<game::Snapshot>;

    /// The current game's quest record ([`quests::read`]); empty until found. `new_game`
    /// drops the previous game's record.
    fn quest_flags(&mut self, new_game: bool) -> &[u16];

    /// Drops the quest record (no game: menu, loading).
    fn forget_quests(&mut self);

    /// True while the attached process exists.
    fn is_alive(&self) -> bool;

    /// The game's save directory (character save files), if found.
    fn save_dir(&self) -> Option<PathBuf>;
}

/// The platform layer under [`MemoryReader`]: what a port to another OS replaces.
/// `Clone + Send + 'static`: the items search runs on its own thread with its own copy.
pub trait Platform: Debug + Clone + Send + 'static {
    /// An open handle on the game's memory.
    type Mem: ReadMem + Debug;

    /// The pid of the running game.
    ///
    /// # Errors
    /// No game process.
    fn find_game(&self) -> Result<u32>;

    /// Opens the memory of process `pid`.
    ///
    /// # Errors
    /// Missing permission, or the process is gone.
    fn open(&self, pid: u32) -> Result<Self::Mem>;

    /// True while process `pid` exists.
    fn is_alive(&self, pid: u32) -> bool;

    /// The D2R image: its live base (where pointers point) and a fully readable view of it.
    ///
    /// # Errors
    /// The image can't be located.
    fn find_image(&self, pid: u32, mem: &Self::Mem) -> Result<image::Image>;

    /// The readable anonymous memory ranges heap tables can be in ([`items::find_table`]).
    ///
    /// # Errors
    /// The process's memory map can't be read.
    fn heap_regions(&self, pid: u32) -> Result<Vec<Range<u64>>>;

    /// The game's save directory for process `pid`, if found.
    fn save_dir(&self, pid: u32) -> Option<PathBuf>;

    /// A hint to print with `err` from [`open`](Platform::open) (e.g. the missing capability).
    fn access_hint(&self, err: &anyhow::Error) -> Option<String>;
}

/// Lowest address treated as a plausible user-space pointer (exclusive).
const MIN_USER_PTR: u64 = 0x10000;
/// Highest address treated as a plausible user-space pointer (exclusive).
const MAX_USER_PTR: u64 = 0x7fff_ffff_ffff;

/// Read access to another process's address space. Little-endian helpers are provided.
pub trait ReadMem {
    /// Fills `buf` from `addr`; a short or failed read is an error.
    ///
    /// # Errors
    /// Any I/O error, including unmapped or no-access addresses.
    fn read(&self, addr: u64, buf: &mut [u8]) -> io::Result<()>;

    /// # Errors
    /// See [`ReadMem::read`].
    fn read_bytes(&self, addr: u64, len: usize) -> io::Result<Vec<u8>> {
        let mut buf = vec![0; len];
        self.read(addr, &mut buf)?;
        Ok(buf)
    }

    /// # Errors
    /// See [`ReadMem::read`].
    fn read_array<const N: usize>(&self, addr: u64) -> io::Result<[u8; N]> {
        let mut buf = [0; N];
        self.read(addr, &mut buf)?;
        Ok(buf)
    }

    /// # Errors
    /// See [`ReadMem::read`].
    fn read_u8(&self, addr: u64) -> io::Result<u8> {
        self.read_array::<1>(addr).map(u8::from_le_bytes)
    }

    /// # Errors
    /// See [`ReadMem::read`].
    fn read_u16(&self, addr: u64) -> io::Result<u16> {
        self.read_array(addr).map(u16::from_le_bytes)
    }

    /// # Errors
    /// See [`ReadMem::read`].
    fn read_u32(&self, addr: u64) -> io::Result<u32> {
        self.read_array(addr).map(u32::from_le_bytes)
    }

    /// # Errors
    /// See [`ReadMem::read`].
    fn read_u64(&self, addr: u64) -> io::Result<u64> {
        self.read_array(addr).map(u64::from_le_bytes)
    }

    /// Reads a pointer; `Ok(None)` if the value isn't a plausible user-space address
    /// (null, tiny, or kernel-half).
    ///
    /// # Errors
    /// See [`ReadMem::read`].
    fn read_ptr(&self, addr: u64) -> io::Result<Option<u64>> {
        let p = self.read_u64(addr)?;
        Ok(is_user_ptr(p).then_some(p))
    }
}

/// True if `p` is a plausible user-space pointer (not null, not tiny, not kernel-half).
#[must_use]
pub const fn is_user_ptr(p: u64) -> bool {
    MIN_USER_PTR < p && p < MAX_USER_PTR
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::FakeMem;
    use anyhow::Result;

    #[test]
    fn readmem_helpers() -> Result<()> {
        let mut m = FakeMem::default();
        m.add(0x1000, vec![1, 2, 3, 4, 5, 6, 7, 8, 0, 0, 2, 0, 0, 0, 0, 0]);
        assert_eq!(m.read_u8(0x1000)?, 0x01);
        assert_eq!(m.read_u16(0x1000)?, 0x0201);
        assert_eq!(m.read_u32(0x1000)?, 0x0403_0201);
        assert_eq!(m.read_u64(0x1000)?, 0x0807_0605_0403_0201);
        assert_eq!(m.read_bytes(0x1002, 3)?, vec![3, 4, 5]);
        assert_eq!(m.read_ptr(0x1008)?, Some(0x2_0000));
        assert!(m.read_u64(0x100c).is_err(), "crosses region end");
        assert!(m.read_u8(0x0fff).is_err(), "before region");
        Ok(())
    }

    #[test]
    fn read_ptr_plausibility() -> Result<()> {
        let mut m = FakeMem::default();
        let vals: [u64; 4] = [0, 0x10000, 0x7fff_ffff_ffff, 0x7ff0_0000_0000];
        m.add(0x1000, vals.iter().flat_map(|v| v.to_le_bytes()).collect());
        assert_eq!(m.read_ptr(0x1000)?, None);
        assert_eq!(m.read_ptr(0x1008)?, None);
        assert_eq!(m.read_ptr(0x1010)?, None);
        assert_eq!(m.read_ptr(0x1018)?, Some(0x7ff0_0000_0000));
        Ok(())
    }
}

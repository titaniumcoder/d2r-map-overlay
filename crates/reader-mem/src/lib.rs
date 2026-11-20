//! The memory reader plugin: finds the game and reads its memory into the core's
//! [`d2r_core::game::Snapshot`], behind the core's [`d2r_core::reader::GameReader`].
//!
//! It has two layers:
//!
//! - **Platform layer** ([`Platform`]): find the game process, open its memory ([`ReadMem`]),
//!   locate the D2R image, list the heap regions, find the save directory. Today `Proton`
//!   (Cargo feature `proton`): [`process`] (`/proc` pid discovery, `/proc/<pid>/maps`,
//!   `/proc/<pid>/mem`), Wine's memfd alias of the encrypted image ([`image::find_image`]), the
//!   `WINEPREFIX` save directory.
//! - **Game layer** (any platform): the offsets ([`d2r_core::offsets`]), [`offset_cache`], the
//!   structure walk in [`game`] (generic over [`ReadMem`]), the unit-table and quest-global
//!   searches on an [`image::Image`], the items-table scan ([`d2r_core::items`]), the quest
//!   record ([`d2r_core::quests`]), and [`MemoryReader`], which drives them over any
//!   [`Platform`] (retries, the background items search, the quest search budget).
//!
//! # A Windows platform
//! A Windows build would add a `Platform` implementation (behind its own feature) and reuse the
//! game layer:
//! - `find_game`: Toolhelp32 (`CreateToolhelp32Snapshot` + `Process32First/NextW`, exe name
//!   `D2R.exe`); `is_alive`: `OpenProcess` + `GetExitCodeProcess` (`STILL_ACTIVE`).
//! - `open` / `Mem`: `OpenProcess(PROCESS_VM_READ | PROCESS_QUERY_INFORMATION)` and
//!   `ReadProcessMemory`. That is FFI, so it needs `unsafe` (forbidden in this workspace) or a
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
mod memory_reader;
pub mod offset_cache;
pub mod process;
#[cfg(feature = "proton")]
mod proton;

use std::fmt::Debug;
use std::ops::Range;
use std::path::PathBuf;

use anyhow::Result;
use d2r_core::reader::ReadMem;

pub use memory_reader::MemoryReader;
#[cfg(feature = "proton")]
pub use proton::Proton;

/// The reader over the Proton/Linux platform.
#[cfg(feature = "proton")]
pub type ProtonReader = MemoryReader<Proton>;

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

    /// The readable anonymous memory ranges heap tables can be in ([`d2r_core::items::find_table`]).
    ///
    /// # Errors
    /// The process's memory map can't be read.
    fn heap_regions(&self, pid: u32) -> Result<Vec<Range<u64>>>;

    /// The game's save directory for process `pid`, if found.
    fn save_dir(&self, pid: u32) -> Option<PathBuf>;

    /// A hint to print with `err` from [`open`](Platform::open) (e.g. the missing capability).
    fn access_hint(&self, err: &anyhow::Error) -> Option<String>;
}

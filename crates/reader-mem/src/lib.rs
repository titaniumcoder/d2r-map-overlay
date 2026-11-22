//! The memory reader plugin: a platform layer ([`Platform`]) finds the game and opens its memory;
//! the game layer ([`game`], [`image`], [`MemoryReader`]) walks it into a [`d2r_core::game::Snapshot`].

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
/// `Clone + Send`: the items search runs on its own thread with its own copy.
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

//! The reader plugin's interface: [`GameReader`], what the core needs from a game reader, and
//! [`ReadMem`], read access to the game's memory (the quest record decoder, [`crate::quests`],
//! reads through it too).
//!
//! The reader plugins are crates of their own, picked at build time by the binary's Cargo
//! features; today `d2r-reader-mem` (the memory reader over a platform layer, with the
//! Proton/Linux platform behind its `proton` feature). A reader fills the core's
//! [`crate::game::Snapshot`].

use std::io;
use std::path::PathBuf;

use anyhow::Result;

use crate::game;

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
    /// Not attached, or no game state (menu, loading).
    fn snapshot(&mut self, player: Option<&str>, previous: Option<u32>) -> Result<game::Snapshot>;

    /// The current game's quest record ([`crate::quests::read`]); empty until found. `new_game`
    /// drops the previous game's record.
    fn quest_flags(&mut self, new_game: bool) -> &[u16];

    /// Drops the quest record (no game: menu, loading).
    fn forget_quests(&mut self);

    /// True while the attached process exists.
    fn is_alive(&self) -> bool;

    /// The game's save directory (character save files), if found.
    fn save_dir(&self) -> Option<PathBuf>;
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

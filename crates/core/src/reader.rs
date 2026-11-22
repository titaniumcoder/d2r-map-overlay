//! The reader plugin's interface ([`GameReader`]) and read access to the game's memory
//! ([`ReadMem`]).

use std::io;
use std::path::PathBuf;

use anyhow::Result;

use crate::game;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Attach {
    /// No game process, or its memory can't be opened (logged): try again later.
    Waiting,
    /// Attached and the unit table located.
    Ready { pid: u32 },
    /// The process exited before the unit table was found.
    Exited { pid: u32 },
}

/// One reader follows one game process at a time; [`attach`](GameReader::attach) again after
/// it exits.
pub trait GameReader {
    /// One attempt to find the game and open its memory; once open, blocks until the unit
    /// table is found or the process exits.
    fn attach(&mut self) -> Attach;

    /// One attempt without waiting; each finding goes to `report` as it is found, so a failure
    /// still shows how far it got.
    ///
    /// # Errors
    /// Fails if any step fails.
    fn attach_now(&mut self, report: &mut dyn FnMut(&'static str, String)) -> Result<()>;

    /// Searches the item-code table now, blocking; live mode searches in the background.
    fn locate_items_now(&mut self);

    /// `player` picks among several player units; `previous` is the unit id chosen before.
    ///
    /// # Errors
    /// Not attached, or no game state (menu, loading).
    fn snapshot(&mut self, player: Option<&str>, previous: Option<u32>) -> Result<game::Snapshot>;

    /// The current game's quest record; empty until found.
    fn quest_flags(&mut self, new_game: bool) -> &[u16];

    fn forget_quests(&mut self);

    fn is_alive(&self) -> bool;

    /// The game's character save directory.
    fn save_dir(&self) -> Option<PathBuf>;
}

/// Plausible user-space pointers lie strictly between these.
const MIN_USER_PTR: u64 = 0x10000;
const MAX_USER_PTR: u64 = 0x7fff_ffff_ffff;

/// Read access to another process's address space (little-endian).
pub trait ReadMem {
    /// A short read is an error.
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

    /// `Ok(None)` if the value isn't a plausible user-space address.
    ///
    /// # Errors
    /// See [`ReadMem::read`].
    fn read_ptr(&self, addr: u64) -> io::Result<Option<u64>> {
        let p = self.read_u64(addr)?;
        Ok(is_user_ptr(p).then_some(p))
    }
}

/// Not null, not tiny, not kernel-half.
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

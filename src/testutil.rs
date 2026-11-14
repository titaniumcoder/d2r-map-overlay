//! Test doubles for the OS boundary.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::{fs, io};

use crate::process::ReadMem;

/// Sparse fake address space: regions keyed by base address. A read must fit inside one region.
#[derive(Debug, Default)]
pub struct FakeMem {
    regions: BTreeMap<u64, Vec<u8>>,
}

impl FakeMem {
    pub fn add(&mut self, base: u64, bytes: Vec<u8>) {
        self.regions.insert(base, bytes);
    }
}

impl ReadMem for FakeMem {
    fn read(&self, addr: u64, buf: &mut [u8]) -> io::Result<()> {
        let len = buf.len();
        let unmapped = || io::Error::other(format!("unmapped read at {addr:#x} (+{len:#x})"));
        // Last region starting at or below `addr`.
        let (base, bytes) = self.regions.range(..=addr).next_back().ok_or_else(unmapped)?;
        let start = usize::try_from(addr - base).map_err(|_| unmapped())?;
        let src = start
            .checked_add(len)
            .and_then(|end| bytes.get(start..end))
            .ok_or_else(unmapped)?;
        buf.copy_from_slice(src);
        Ok(())
    }
}

/// A fresh, empty directory path under the system temp dir, unique per process and `name`
/// (a leftover from an earlier run is removed). Not created: the code under test creates it.
pub fn temp_dir(name: &str) -> io::Result<PathBuf> {
    let dir = std::env::temp_dir().join(format!("d2r-map-overlay-test-{}-{name}", std::process::id()));
    match fs::remove_dir_all(&dir) {
        Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
        _ => Ok(dir),
    }
}

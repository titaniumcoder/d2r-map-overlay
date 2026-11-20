//! The Proton/Linux platform layer: the game is a Wine process found through `/proc`, its memory
//! is read with `/proc/<pid>/mem`, and the image's encrypted pages are read through Wine's
//! readable memfd alias ([`image::find_image`]).

use std::fs;
use std::io;
use std::ops::Range;
use std::path::{Path, PathBuf};

use anyhow::Result;

use super::Platform;
use super::image::{self, Image};
use super::process::{self, ProcMem};

/// D2R under Proton (Wine) on Linux.
#[derive(Debug, Clone, Copy, Default)]
pub struct Proton;

impl Platform for Proton {
    type Mem = ProcMem;

    fn find_game(&self) -> Result<u32> {
        process::find_d2r_pid()
    }

    fn open(&self, pid: u32) -> Result<ProcMem> {
        ProcMem::open(pid)
    }

    fn is_alive(&self, pid: u32) -> bool {
        process::is_alive(pid)
    }

    fn find_image(&self, pid: u32, mem: &ProcMem) -> Result<Image> {
        image::find_image(&process::read_maps(pid)?, mem)
    }

    fn heap_regions(&self, pid: u32) -> Result<Vec<Range<u64>>> {
        Ok(process::heap_regions(&process::read_maps(pid)?))
    }

    /// From the process's `WINEPREFIX` (its environment: same user, so no ptrace capability
    /// is needed).
    fn save_dir(&self, pid: u32) -> Option<PathBuf> {
        fs::read(format!("/proc/{pid}/environ"))
            .ok()
            .and_then(|env| wineprefix(&env))
            .and_then(|prefix| saves_dir(&prefix))
    }

    fn access_hint(&self, err: &anyhow::Error) -> Option<String> {
        is_permission_denied(err).then(ptrace_hint)
    }
}

/// `WINEPREFIX` from a process's `/proc/<pid>/environ` bytes (NUL-separated `KEY=value`).
fn wineprefix(environ: &[u8]) -> Option<PathBuf> {
    environ
        .split(|&b| b == 0)
        .find_map(|var| var.strip_prefix(b"WINEPREFIX="))
        .and_then(|v| std::str::from_utf8(v).ok())
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// D2R's save directory under `prefix`: `drive_c/users/<user>/Saved Games/Diablo II
/// Resurrected` for the first user that has one (Proton's is `steamuser`).
fn saves_dir(prefix: &Path) -> Option<PathBuf> {
    let users = fs::read_dir(prefix.join("drive_c/users")).ok()?;
    users
        .filter_map(Result::ok)
        .map(|u| u.path().join("Saved Games/Diablo II Resurrected"))
        .find(|d| d.is_dir())
}

/// True if any cause in the chain is EPERM/EACCES (both map to `PermissionDenied`).
fn is_permission_denied(e: &anyhow::Error) -> bool {
    e.chain()
        .filter_map(|c| c.downcast_ref::<io::Error>())
        .any(|io| io.kind() == io::ErrorKind::PermissionDenied)
}

fn ptrace_hint() -> String {
    let exe =
        std::env::current_exe().map_or_else(|_| "d2r-map-overlay".to_owned(), |p| p.display().to_string());
    format!(
        "reading the game's memory needs CAP_SYS_PTRACE; grant it once per build with:\n  \
         sudo setcap cap_sys_ptrace=ep {exe}"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::Context;

    use crate::testutil::temp_dir;

    #[test]
    fn wineprefix_is_read_from_environ() {
        let env = b"HOME=/home/u\0WINEPREFIX=/p/pfx/\0X=1\0";
        assert_eq!(wineprefix(env), Some(PathBuf::from("/p/pfx/")));
        assert_eq!(wineprefix(b"HOME=/home/u\0"), None);
    }

    #[test]
    fn the_save_dir_is_found_in_a_prefix() -> Result<()> {
        let prefix = temp_dir("proton-prefix")?;
        let saves = prefix.join("drive_c/users/steamuser/Saved Games/Diablo II Resurrected");
        fs::create_dir_all(&saves)?;
        assert_eq!(saves_dir(&prefix).context("saves dir")?, saves);
        fs::remove_dir_all(&prefix)?;
        assert_eq!(saves_dir(&prefix), None, "no prefix, no dir");
        Ok(())
    }

    #[test]
    fn permission_denied_is_detected_through_context() {
        let e = anyhow::Error::from(io::Error::from(io::ErrorKind::PermissionDenied)).context("opening mem");
        assert!(is_permission_denied(&e));
        assert!(Proton.access_hint(&e).is_some_and(|h| h.contains("setcap")));
        let e = anyhow::Error::from(io::Error::from(io::ErrorKind::NotFound)).context("opening mem");
        assert!(!is_permission_denied(&e));
        assert_eq!(Proton.access_hint(&e), None);
    }
}

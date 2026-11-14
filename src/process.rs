//! The OS boundary: finding the D2R process, parsing `/proc/<pid>/maps`, and reading its memory.

use std::fs::{self, File};
use std::io;
use std::os::unix::fs::FileExt;

use anyhow::{Context, Result};

/// Substring of the Wine process's cmdline that identifies the game (Windows path, backslash).
const D2R_CMDLINE: &str = "Diablo II Resurrected\\D2R.exe";
/// The game process's `/proc/<pid>/comm` (Wine names the main thread `Main`).
const D2R_COMM: &str = "Main";

/// Lowest address treated as a plausible user-space pointer (exclusive).
const MIN_USER_PTR: u64 = 0x10000;
/// Highest address treated as a plausible user-space pointer (exclusive).
const MAX_USER_PTR: u64 = 0x7fff_ffff_ffff;

/// Finds the pid of the running D2R process by scanning `/proc/*/cmdline`.
///
/// If several processes match (should not happen), the lowest pid wins, which is
/// normally the oldest one.
///
/// # Errors
/// Fails if `/proc` can't be listed or no process matches.
pub fn find_d2r_pid() -> Result<u32> {
    let mut found: Option<u32> = None;
    for entry in fs::read_dir("/proc").context("listing /proc")? {
        let Ok(entry) = entry else { continue };
        let Some(pid) = entry.file_name().to_str().and_then(|s| s.parse::<u32>().ok()) else {
            continue;
        };
        // Processes can exit or deny access between listing and reading; skip those.
        let Ok(cmdline) = fs::read(entry.path().join("cmdline")) else {
            continue;
        };
        // The game's comm is `Main`; this rules out launchers or `pgrep`s quoting the path.
        let comm = fs::read_to_string(entry.path().join("comm")).unwrap_or_default();
        if is_d2r_cmdline(&cmdline) && comm.trim_end() == D2R_COMM {
            found = Some(found.map_or(pid, |f| f.min(pid)));
        }
    }
    found.context("no running D2R process found (cmdline containing `Diablo II Resurrected\\D2R.exe`)")
}

/// True if a raw (NUL-separated) cmdline belongs to the game.
fn is_d2r_cmdline(cmdline: &[u8]) -> bool {
    String::from_utf8_lossy(cmdline).contains(D2R_CMDLINE)
}

/// Permission bits of a mapping (`rwxs` / `rwxp` column).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[allow(clippy::struct_excessive_bools)] // reason: mirrors the four independent perm flags of the maps line
pub struct Perms {
    pub read: bool,
    pub write: bool,
    pub exec: bool,
    /// `s` (shared) rather than `p` (private).
    pub shared: bool,
}

impl Perms {
    fn parse(s: &str) -> Option<Self> {
        let b = s.as_bytes();
        if b.len() != 4 {
            return None;
        }
        let flag = |i: usize, on: u8| match b.get(i) {
            Some(&c) if c == on => Some(true),
            Some(b'-') => Some(false),
            _ => None,
        };
        let shared = match b.get(3) {
            Some(b's') => true,
            Some(b'p') => false,
            _ => return None,
        };
        Some(Self {
            read: flag(0, b'r')?,
            write: flag(1, b'w')?,
            exec: flag(2, b'x')?,
            shared,
        })
    }
}

/// One line of `/proc/<pid>/maps`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MapEntry {
    pub start: u64,
    pub end: u64,
    pub perms: Perms,
    pub offset: u64,
    pub inode: u64,
    /// May be empty, contain spaces, or end in ` (deleted)`.
    pub pathname: String,
}

impl MapEntry {
    #[must_use]
    pub const fn len(&self) -> u64 {
        self.end.saturating_sub(self.start)
    }
}

/// Splits off the next space-separated field, returning `(field, rest)`.
fn next_field(s: &str) -> Option<(&str, &str)> {
    let s = s.trim_start_matches(' ');
    if s.is_empty() {
        return None;
    }
    Some(s.split_once(' ').unwrap_or((s, "")))
}

fn parse_maps_line(line: &str) -> Option<MapEntry> {
    let (range, rest) = next_field(line)?;
    let (perms, rest) = next_field(rest)?;
    let (offset, rest) = next_field(rest)?;
    let (_dev, rest) = next_field(rest)?;
    let (inode, rest) = next_field(rest)?;
    let (start, end) = range.split_once('-')?;
    Some(MapEntry {
        start: u64::from_str_radix(start, 16).ok()?,
        end: u64::from_str_radix(end, 16).ok()?,
        perms: Perms::parse(perms)?,
        offset: u64::from_str_radix(offset, 16).ok()?,
        inode: inode.parse().ok()?,
        // The kernel pads with spaces before the pathname; the pathname itself keeps its spaces.
        pathname: rest.trim_start_matches(' ').to_owned(),
    })
}

/// Parses the full text of a maps file.
///
/// # Errors
/// Fails on the first malformed line, naming its line number.
pub fn parse_maps(text: &str) -> Result<Vec<MapEntry>> {
    text.lines()
        .enumerate()
        .filter(|(_, l)| !l.is_empty())
        .map(|(i, l)| parse_maps_line(l).with_context(|| format!("maps line {}: malformed: {l:?}", i + 1)))
        .collect()
}

/// Reads and parses `/proc/<pid>/maps`.
///
/// # Errors
/// Fails if the file can't be read or is malformed.
pub fn read_maps(pid: u32) -> Result<Vec<MapEntry>> {
    let path = format!("/proc/{pid}/maps");
    let text = fs::read_to_string(&path).with_context(|| format!("reading {path}"))?;
    parse_maps(&text)
}

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

/// Reads a live process through `/proc/<pid>/mem` (positioned reads; needs `CAP_SYS_PTRACE`).
#[derive(Debug)]
pub struct ProcMem {
    file: File,
}

impl ProcMem {
    /// # Errors
    /// Fails if `/proc/<pid>/mem` can't be opened (typically missing `CAP_SYS_PTRACE`).
    pub fn open(pid: u32) -> Result<Self> {
        let path = format!("/proc/{pid}/mem");
        let file = File::open(&path)
            .with_context(|| format!("opening {path} (needs CAP_SYS_PTRACE, see CLAUDE.md)"))?;
        Ok(Self { file })
    }
}

impl ReadMem for ProcMem {
    fn read(&self, addr: u64, buf: &mut [u8]) -> io::Result<()> {
        // `read_exact_at` is a pread loop: no shared file cursor, so `&self` is enough.
        self.file.read_exact_at(buf, addr)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::FakeMem;

    #[test]
    fn cmdline_match() {
        assert!(is_d2r_cmdline(
            b"C:\\Program Files (x86)\\Diablo II Resurrected\\D2R.exe\0"
        ));
        assert!(!is_d2r_cmdline(b"/usr/bin/bash\0-c\0D2R.exe\0"));
    }

    #[test]
    fn maps_line_basic() -> Result<()> {
        let e = parse_maps_line("7f00a000-7f00b000 r-xp 00001000 fd:01 1234      /usr/lib/libc.so.6")
            .context("parse")?;
        assert_eq!(e.start, 0x7f00_a000);
        assert_eq!(e.end, 0x7f00_b000);
        assert_eq!(e.len(), 0x1000);
        assert_eq!(
            e.perms,
            Perms {
                read: true,
                write: false,
                exec: true,
                shared: false
            }
        );
        assert_eq!(e.offset, 0x1000);
        assert_eq!(e.inode, 1234);
        assert_eq!(e.pathname, "/usr/lib/libc.so.6");
        Ok(())
    }

    #[test]
    fn maps_line_spaces_and_deleted() -> Result<()> {
        let e = parse_maps_line(
            "6ffff9a60000-6ffff9a61000 r--s 00000000 00:01 4242                       /memfd:wine-mapping (deleted)",
        )
        .context("parse")?;
        assert!(e.perms.shared && e.perms.read && !e.perms.write);
        assert_eq!(e.inode, 4242);
        assert_eq!(e.pathname, "/memfd:wine-mapping (deleted)");

        let e =
            parse_maps_line("1000-2000 ---s 00000000 00:01 7 /home/u/My Games/a  b.bin").context("parse")?;
        assert_eq!(
            e.perms,
            Perms {
                shared: true,
                ..Perms::default()
            }
        );
        assert_eq!(e.pathname, "/home/u/My Games/a  b.bin");
        Ok(())
    }

    #[test]
    fn maps_line_anonymous() -> Result<()> {
        let e = parse_maps_line("1000-2000 rw-p 00000000 00:00 0 ").context("parse")?;
        assert_eq!(e.pathname, "");
        let e = parse_maps_line("1000-2000 rw-p 00000000 00:00 0").context("parse")?;
        assert_eq!(e.pathname, "");
        Ok(())
    }

    #[test]
    fn maps_malformed() -> Result<()> {
        assert!(parse_maps_line("zz-2000 rw-p 0 00:00 0").is_none());
        assert!(parse_maps_line("1000-2000 rwxq 0 00:00 0").is_none());
        assert!(parse_maps_line("1000-2000 rw-p 0").is_none());
        let err = parse_maps("1000-2000 rw-p 0 00:00 0\nbad\n")
            .err()
            .context("expected an error")?;
        assert!(err.to_string().contains("line 2"), "{err}");
        Ok(())
    }

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

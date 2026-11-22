//! The OS boundary: finding the D2R process, parsing `/proc/<pid>/maps`, and reading its memory.

use std::fs::{self, File};
use std::io;
use std::ops::Range;
use std::os::unix::fs::FileExt;
use std::path::Path;

use anyhow::{Context, Result};

use d2r_core::reader::ReadMem;

/// Substring of the Wine process's cmdline that identifies the game (Windows path, backslash).
const D2R_CMDLINE: &str = "Diablo II Resurrected\\D2R.exe";
/// The game process's `/proc/<pid>/comm` (Wine names the main thread `Main`).
const D2R_COMM: &str = "Main";

/// The pid of the running D2R process; the lowest (normally the oldest) if several match.
///
/// # Errors
/// Fails if `/proc` can't be listed or no process matches.
pub fn find_d2r_pid() -> Result<u32> {
    fs::read_dir("/proc")
        .context("listing /proc")?
        .filter_map(|entry| {
            let entry = entry.ok()?;
            let pid = entry.file_name().to_str()?.parse::<u32>().ok()?;
            is_d2r_process(&entry.path()).then_some(pid)
        })
        .min()
        .context("no running D2R process found (cmdline containing `Diablo II Resurrected\\D2R.exe`)")
}

/// True if the `/proc/<pid>` directory `dir` is the game's; a process that exits or denies
/// access mid-scan is not. The comm check rules out launchers and `pgrep`s quoting the path.
fn is_d2r_process(dir: &Path) -> bool {
    let Ok(cmdline) = fs::read(dir.join("cmdline")) else {
        return false;
    };
    let comm = fs::read_to_string(dir.join("comm")).unwrap_or_default();
    is_d2r_cmdline(&cmdline) && comm.trim_end() == D2R_COMM
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

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len() == 0
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

/// True for mappings a heap table can be in: readable, not backed by a file, not a kernel page.
fn is_heap_region(e: &MapEntry) -> bool {
    e.perms.read
        && e.inode == 0
        && !matches!(
            e.pathname.as_str(),
            "[vvar]" | "[vvar_vclock]" | "[vsyscall]" | "[vdso]"
        )
}

/// The address ranges of `maps` that heap tables can be in ([`is_heap_region`]), in order.
#[must_use]
pub fn heap_regions(maps: &[MapEntry]) -> Vec<Range<u64>> {
    maps.iter()
        .filter(|e| is_heap_region(e))
        .map(|e| e.start..e.end)
        .collect()
}

/// True while process `pid` exists.
#[must_use]
pub fn is_alive(pid: u32) -> bool {
    Path::new(&format!("/proc/{pid}")).exists()
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
    fn heap_regions_skip_files_unreadable_and_kernel_pages() -> Result<()> {
        let maps = parse_maps(
            "100000-101000 rw-p 00000000 00:00 0\n\
             200000-201000 rw-p 00000000 fd:01 77 /game/data.bin\n\
             300000-301000 ---p 00000000 00:00 0\n\
             400000-402000 rw-p 00000000 00:00 0 [heap]\n\
             500000-501000 r--p 00000000 00:00 0 [vvar]\n\
             600000-601000 r-xp 00000000 00:00 0 [vdso]\n",
        )?;
        assert_eq!(
            heap_regions(&maps),
            vec![0x10_0000..0x10_1000, 0x40_0000..0x40_2000]
        );
        Ok(())
    }
}

//! Persistent set of monster-preset txt ids known to be superuniques (the filesystem side of
//! superunique learning, see `session`).
//!
//! Monster presets include ordinary fixed spawns, so only presets whose txt id was once seen
//! next to a live superunique are drawn. The ids are learned in play and kept in a plain file
//! next to the offset cache, one decimal id per line; `#` lines are comments. Nothing is seeded.

use std::collections::BTreeSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::{fs, io};

use anyhow::{Context, Result};

const HEADER: &str =
    "# d2r-map-overlay: monster preset txt ids seen as superuniques, one per line. Safe to delete.\n";

/// `superuniques.txt` in the offset cache's directory; `None` if that has no path.
#[must_use]
pub fn default_path() -> Option<PathBuf> {
    Some(
        crate::offset_cache::default_path()?
            .parent()?
            .join("superuniques.txt"),
    )
}

/// Parses the file text.
///
/// # Errors
/// Fails on a line that is neither blank, a `#` comment, nor a decimal u32.
pub fn parse(text: &str) -> Result<BTreeSet<u32>> {
    text.lines()
        .enumerate()
        .map(|(i, raw)| (i, raw.trim()))
        .filter(|(_, line)| !line.is_empty() && !line.starts_with('#'))
        .map(|(i, line)| {
            line.parse()
                .with_context(|| format!("superuniques line {}: not an id: {line:?}", i + 1))
        })
        .collect()
}

/// Formats ids in the layout [`parse`] reads.
#[must_use]
pub fn format(ids: &BTreeSet<u32>) -> String {
    let mut out = String::from(HEADER);
    for id in ids {
        // Writing to a `String` can't fail.
        let _ = writeln!(out, "{id}");
    }
    out
}

/// Reads the file; a missing file is an empty set.
///
/// # Errors
/// Fails if the file exists but can't be read or parsed.
pub fn read(path: &Path) -> Result<BTreeSet<u32>> {
    match fs::read_to_string(path) {
        Ok(text) => parse(&text).with_context(|| format!("parsing {}", path.display())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(BTreeSet::new()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Writes `ids` (the full set), creating the directory if needed.
///
/// # Errors
/// Fails if the directory or file can't be written.
pub fn write(path: &Path, ids: &BTreeSet<u32>) -> Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    fs::write(path, format(ids)).with_context(|| format!("writing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::temp_dir;

    #[test]
    fn parse_format_round_trip() -> Result<()> {
        let ids = BTreeSet::from([771, 853, 7]);
        let text = format(&ids);
        assert!(text.ends_with("7\n771\n853\n"), "{text}");
        assert_eq!(parse(&text)?, ids);
        assert_eq!(parse("  12 \n\n# x\n3\n")?, BTreeSet::from([3, 12]));
        assert!(parse("12\nabc\n").is_err());
        Ok(())
    }

    #[test]
    fn write_read_round_trip() -> Result<()> {
        let dir = temp_dir("superuniques")?;
        let path = dir.join("sub").join("superuniques.txt");
        assert!(read(&path)?.is_empty(), "missing file = empty set");
        let ids = BTreeSet::from([853, 771]);
        write(&path, &ids)?;
        assert_eq!(read(&path)?, ids);
        fs::remove_dir_all(&dir)?;
        Ok(())
    }
}

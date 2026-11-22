//! Per-build cache of the unit-table offset: D2R re-encrypts code pages, so a signature that
//! matched once may stop matching. Hand-parsed TOML subset ([`HEADER`]); other lines are dropped.

use std::collections::BTreeMap;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::{fs, io};

use anyhow::{Context, Result, bail};
use d2r_core::game_file::cache_dir;

const HEADER: &str = "# d2r-map-overlay: offsets found at runtime, per D2R build. Safe to delete.\n\
                      # [build.\"<TimeDateStamp>-<SizeOfImage>\"] (hex); RVAs are from the image base.\n";
const UNIT_TABLE_RVA: &str = "unit_table_rva";

/// `offsets.toml` in the cache dir ([`cache_dir`]).
#[must_use]
pub fn default_path() -> Option<PathBuf> {
    cache_dir().map(|d| d.join("offsets.toml"))
}

/// Parses the cache text into build key → unit table RVA.
///
/// # Errors
/// Fails on a `key = value` line without `=` or a `unit_table_rva` that isn't `0x…` hex.
pub fn parse(text: &str) -> Result<BTreeMap<String, u64>> {
    let mut out = BTreeMap::new();
    let mut build: Option<&str> = None;
    for (i, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if line.starts_with('[') {
            build = line.strip_prefix("[build.\"").and_then(|r| r.strip_suffix("\"]"));
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            bail!("offsets cache line {}: malformed: {raw:?}", i + 1);
        };
        let (Some(build), UNIT_TABLE_RVA) = (build, key.trim()) else {
            continue;
        };
        let rva = value
            .trim()
            .strip_prefix("0x")
            .and_then(|hex| u64::from_str_radix(hex, 16).ok())
            .with_context(|| format!("offsets cache line {}: bad hex value: {raw:?}", i + 1))?;
        out.insert(build.to_owned(), rva);
    }
    Ok(out)
}

/// Formats entries in the layout [`parse`] reads.
#[must_use]
pub fn format(entries: &BTreeMap<String, u64>) -> String {
    let mut out = String::from(HEADER);
    for (build, rva) in entries {
        // Writing to a `String` can't fail.
        let _ = write!(out, "\n[build.\"{build}\"]\n{UNIT_TABLE_RVA} = {rva:#x}\n");
    }
    out
}

/// Reads the cache; a missing file is an empty cache.
///
/// # Errors
/// Fails if the file exists but can't be read or parsed.
pub fn read(path: &Path) -> Result<BTreeMap<String, u64>> {
    match fs::read_to_string(path) {
        Ok(text) => parse(&text).with_context(|| format!("parsing {}", path.display())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(BTreeMap::new()),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Records `rva` for `build`, keeping the other builds. A corrupt file is replaced.
/// Does nothing if the entry is already there.
///
/// # Errors
/// Fails if the directory or file can't be written.
pub fn store(path: &Path, build: &str, rva: u64) -> Result<()> {
    let mut entries = read(path).unwrap_or_default();
    if entries.insert(build.to_owned(), rva) == Some(rva) {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    fs::write(path, format(&entries)).with_context(|| format!("writing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use d2r_core::testutil::temp_dir;

    #[test]
    fn parse_format_round_trip() -> Result<()> {
        let entries = BTreeMap::from([
            ("6543abcd-279c000".to_owned(), 0x1eb_e5f0),
            ("00000001-2700000".to_owned(), 0x10),
        ]);
        let text = format(&entries);
        assert!(
            text.contains("[build.\"6543abcd-279c000\"]\nunit_table_rva = 0x1ebe5f0\n"),
            "{text}"
        );
        assert_eq!(parse(&text)?, entries);
        Ok(())
    }

    #[test]
    fn parse_ignores_unknown_and_rejects_garbage() -> Result<()> {
        let text = "[other]\nunit_table_rva = 0x5\n[build.\"k\"]\nfoo = 1\nunit_table_rva = 0x20\n";
        assert_eq!(parse(text)?, BTreeMap::from([("k".to_owned(), 0x20)]));
        assert!(parse("[build.\"k\"]\nunit_table_rva = 32\n").is_err(), "decimal");
        assert!(parse("[build.\"k\"]\nunit_table_rva\n").is_err(), "no =");
        Ok(())
    }

    #[test]
    fn store_read_round_trip() -> Result<()> {
        let dir = temp_dir("cache")?;
        let path = dir.join("sub").join("offsets.toml");
        assert!(read(&path)?.is_empty(), "missing file = empty cache");
        store(&path, "a-1", 0x100)?;
        store(&path, "b-2", 0x200)?;
        store(&path, "a-1", 0x180)?;
        assert_eq!(
            read(&path)?,
            BTreeMap::from([("a-1".to_owned(), 0x180), ("b-2".to_owned(), 0x200)])
        );
        fs::write(&path, "garbage line\n")?;
        assert!(read(&path).is_err());
        store(&path, "c-3", 0x300)?;
        assert_eq!(
            read(&path)?,
            BTreeMap::from([("c-3".to_owned(), 0x300)]),
            "corrupt file replaced"
        );
        fs::remove_dir_all(&dir)?;
        Ok(())
    }
}

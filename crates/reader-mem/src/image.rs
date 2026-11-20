//! Locating the D2R.exe image in the game's memory and scanning it for signatures.
//!
//! D2R's code pages are encrypted, so the live image view contains no-access (`---s`) pages.
//! Wine maps the image from a memfd, and the same memfd is also mapped elsewhere as one fully
//! readable `rw-s` alias. We scan the alias and compute addresses relative to the live base.
//!
//! D2R also re-encrypts code pages after a while, so the unit-table signature can vanish from the
//! alias too. [`Image::unit_table`] therefore falls back to a per-build cache and to a structural
//! search of `.data` (which is never encrypted).

use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result, bail, ensure};

use crate::offset_cache;
use crate::process::MapEntry;
use d2r_core::offsets::{unit, unit_table};
use d2r_core::reader::{ReadMem, is_user_ptr};

/// Accepted `SizeOfImage` range (D2R was 0x279c000 when verified).
const SIZE_OF_IMAGE_RANGE: std::ops::RangeInclusive<u32> = 0x260_0000..=0x2A0_0000;
/// Bytes read from a mapping's start to look for a PE header.
const HEADER_LEN: u64 = 0x400;
/// Chunk size for reading the whole image through the alias.
const CHUNK: usize = 1 << 20;
/// Unit hash table signature; the `rel32` follows the 7 signature bytes.
const UNIT_TABLE_SIG: &str = "48 03 C7 49 8B 8C C6";

/// The located D2R image: where it lives (pointers refer to this) and its readable alias.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Image {
    pub live_base: u64,
    pub alias_base: u64,
    pub size: u64,
}

fn le_u32(b: &[u8], off: usize) -> Option<u32> {
    let bytes = b.get(off..off.checked_add(4)?)?;
    Some(u32::from_le_bytes(bytes.try_into().ok()?))
}

fn le_u16(b: &[u8], off: usize) -> Option<u16> {
    let bytes = b.get(off..off.checked_add(2)?)?;
    Some(u16::from_le_bytes(bytes.try_into().ok()?))
}

/// Offset of the `PE\0\0` signature (`e_lfanew` at 0x3c), after checking `MZ` and the signature.
fn pe_offset(header: &[u8]) -> Option<usize> {
    if header.get(..2)? != b"MZ" {
        return None;
    }
    let pe = usize::try_from(le_u32(header, 0x3c)?).ok()?;
    (header.get(pe..pe.checked_add(4)?)? == b"PE\0\0").then_some(pe)
}

/// `SizeOfImage` from a PE header.
fn size_of_image(header: &[u8]) -> Option<u32> {
    // COFF file header is 20 bytes after the 4-byte signature; SizeOfImage is at +56 in the optional header.
    le_u32(header, pe_offset(header)?.checked_add(4 + 20 + 56)?)
}

/// COFF `TimeDateStamp` (signature +8): together with `SizeOfImage` it identifies a build.
fn time_date_stamp(header: &[u8]) -> Option<u32> {
    le_u32(header, pe_offset(header)?.checked_add(8)?)
}

/// Cache key of a build: `"<TimeDateStamp>-<SizeOfImage>"`, both hex.
fn build_key(header: &[u8]) -> Option<String> {
    Some(format!(
        "{:08x}-{:x}",
        time_date_stamp(header)?,
        size_of_image(header)?
    ))
}

/// `(VirtualAddress, VirtualSize)` of the section named `name` (e.g. `b".data"`).
fn section(header: &[u8], name: &[u8]) -> Option<(u32, u32)> {
    let pe = pe_offset(header)?;
    let count = le_u16(header, pe.checked_add(6)?)?;
    let opt_size = le_u16(header, pe.checked_add(20)?)?;
    // Section headers (40 bytes each) follow the optional header.
    let table = pe.checked_add(24)?.checked_add(usize::from(opt_size))?;
    (0..usize::from(count)).find_map(|i| {
        let at = table.checked_add(i.checked_mul(40)?)?;
        let raw = header.get(at..at.checked_add(8)?)?;
        // The name is NUL-padded to 8 bytes.
        let len = raw.iter().position(|&b| b == 0).unwrap_or(raw.len());
        (raw.get(..len)? == name).then(|| {
            Some((
                le_u32(header, at.checked_add(12)?)?,
                le_u32(header, at.checked_add(8)?)?,
            ))
        })?
    })
}

/// True if the adjacent mappings of the same inode starting at `maps[first]` cover `size` bytes
/// and include at least one no-access page (the encrypted live view).
fn is_live_view(maps: &[MapEntry], first: usize, size: u64) -> bool {
    let Some(head) = maps.get(first) else { return false };
    let Some(want_end) = head.start.checked_add(size) else {
        return false;
    };
    let mut end = head.start;
    let mut parts = 0;
    let mut has_noaccess = false;
    for m in maps.iter().skip(first) {
        if m.inode != head.inode || m.start != end || !m.perms.shared {
            break;
        }
        end = m.end;
        parts += 1;
        has_noaccess |= !m.perms.read;
        if end >= want_end {
            break;
        }
    }
    end >= want_end && parts > 1 && has_noaccess
}

/// Finds the D2R image among the process's mappings.
///
/// Candidates are shared, readable, file-backed mappings that start with a PE header whose
/// `SizeOfImage` is in range. The image is the inode that has both a fully readable `rw-s`
/// alias covering the image and a separate multi-part live view with `---s` pages.
///
/// # Errors
/// Fails if no such pair, or more than one, is found.
pub fn find_image(maps: &[MapEntry], mem: &impl ReadMem) -> Result<Image> {
    let mut aliases = Vec::new();
    let mut lives = Vec::new();
    for (i, m) in maps.iter().enumerate() {
        if !(m.perms.read && m.perms.shared && m.inode != 0) {
            continue;
        }
        // Unreadable headers (guard pages, vanished mappings) are simply not candidates.
        let Ok(hdr) = mem.read_bytes(m.start, usize::try_from(m.len().min(HEADER_LEN))?) else {
            continue;
        };
        let Some(size) = size_of_image(&hdr).filter(|s| SIZE_OF_IMAGE_RANGE.contains(s)) else {
            continue;
        };
        let size = u64::from(size);
        if m.perms.write && m.len() >= size {
            aliases.push((m.inode, m.start, size));
        } else if is_live_view(maps, i, size) {
            lives.push((m.inode, m.start, size));
        }
    }
    let images: Vec<Image> = lives
        .iter()
        .flat_map(|&(inode, live_base, size)| {
            aliases
                .iter()
                .filter(move |a| a.0 == inode && a.2 == size)
                .map(move |&(_, alias_base, _)| Image {
                    live_base,
                    alias_base,
                    size,
                })
        })
        .collect();
    match images.as_slice() {
        [img] => Ok(*img),
        [] => bail!(
            "D2R image not found: {} live view(s), {} readable alias(es) with a matching PE header",
            lives.len(),
            aliases.len()
        ),
        many => bail!(
            "ambiguous D2R image: {} live view/alias pairs: {many:x?}",
            many.len()
        ),
    }
}

impl Image {
    /// Reads `len` image bytes at `rva` (offset from the image base) through the alias.
    ///
    /// # Errors
    /// Fails if the range leaves the image or the read fails.
    pub fn read_alias(&self, mem: &impl ReadMem, rva: u64, len: usize) -> Result<Vec<u8>> {
        let end = rva.checked_add(u64::try_from(len)?);
        ensure!(
            end.is_some_and(|e| e <= self.size),
            "rva {rva:#x}+{len:#x} outside image ({:#x})",
            self.size
        );
        let addr = self
            .alias_base
            .checked_add(rva)
            .context("alias address overflow")?;
        mem.read_bytes(addr, len)
            .with_context(|| format!("reading image alias at rva {rva:#x}"))
    }

    /// Reads the whole image through the alias, in 1 MiB chunks.
    fn read_all(&self, mem: &impl ReadMem) -> Result<Vec<u8>> {
        let size = usize::try_from(self.size)?;
        let mut out = Vec::with_capacity(size);
        while out.len() < size {
            let len = CHUNK.min(size - out.len());
            out.extend(self.read_alias(mem, u64::try_from(out.len())?, len)?);
        }
        Ok(out)
    }

    /// Locates the unit hash table, trying in order:
    /// 1. the code signature (`live_base + rel32`, `rel32` after the signature), if it hits once;
    /// 2. the RVA cached for this build in `cache` (if given);
    /// 3. a structural search of `.data` (needs a game in progress).
    ///
    /// With `scan_code` false, step 1 is skipped and only the PE header and `.data` are read
    /// instead of the whole image (~40 MiB): for retries while waiting for a game, where the
    /// signature already missed once and only the structural search can newly succeed.
    ///
    /// A result from 1 or 3 is written to `cache`; a write failure is only a warning on stderr.
    ///
    /// # Errors
    /// Fails if the image can't be read, has no PE header, or all three methods fail.
    pub fn unit_table(&self, mem: &impl ReadMem, cache: Option<&Path>, scan_code: bool) -> Result<UnitTable> {
        let image = if scan_code {
            Some(self.read_all(mem)?)
        } else {
            None
        };
        let header_len = usize::try_from(HEADER_LEN.min(self.size))?;
        let header = match &image {
            Some(img) => img.get(..header_len).unwrap_or(img).to_vec(),
            None => self.read_alias(mem, 0, header_len)?,
        };
        let build = build_key(&header).context("no PE header in the image alias")?;
        let found = |rva: u64, source| {
            if source != UnitTableSource::Cache
                && let Some(path) = cache
                && let Err(e) = offset_cache::store(path, &build, rva)
            {
                eprintln!("warning: could not cache the unit table offset: {e:#}");
            }
            let addr = self
                .live_base
                .checked_add(rva)
                .context("unit table address overflow")?;
            Ok(UnitTable {
                addr,
                rva,
                source,
                build: build.clone(),
            })
        };

        let sig_err = match image.as_deref().map(signature_rva) {
            Some(Ok(rva)) if rva < self.size => return found(rva, UnitTableSource::Signature),
            Some(Ok(rva)) => anyhow::anyhow!("rva {rva:#x} outside the image"),
            Some(Err(e)) => e,
            None => anyhow::anyhow!("not scanned on retry"),
        };
        let cache_note = match cache.map(offset_cache::read) {
            None => "no cache path (neither $XDG_CACHE_HOME nor $HOME set)".to_owned(),
            Some(Err(e)) => format!("{e:#}"),
            Some(Ok(entries)) => match entries.get(&build) {
                Some(&rva) if rva < self.size => return found(rva, UnitTableSource::Cache),
                Some(rva) => format!("cached rva {rva:#x} for build {build} is outside the image"),
                None => format!("no cached offset for build {build}"),
            },
        };
        let structure_err = match self.structural_rva(mem, &header, image.as_deref()) {
            Ok(rva) => return found(rva, UnitTableSource::Structure),
            Err(e) => e,
        };
        bail!(
            "unit table not found — are you in a game? (signature: {sig_err:#}; cache: {cache_note}; \
             structural search: {structure_err:#})"
        )
    }

    /// Live address of the quest-flags global ([`d2r_core::offsets::quest_flags`]), by a
    /// structural search of `.data` ([`find_quest_flags`]; needs a game in progress). Reads the
    /// PE header, `.data` and one header per pointer-like `.data` word (~45k small reads,
    /// once per process).
    ///
    /// # Errors
    /// Fails if `.data` can't be read or the search doesn't find exactly one record pair.
    pub fn quest_flags_global(&self, mem: &impl ReadMem) -> Result<u64> {
        let header_len = usize::try_from(HEADER_LEN.min(self.size))?;
        let header = self.read_alias(mem, 0, header_len)?;
        let (rva, vsize) = section(&header, b".data").context("no .data section in the PE header")?;
        let len = usize::try_from(vsize)?.min(usize::try_from(self.size.saturating_sub(u64::from(rva)))?);
        let data = self.read_alias(mem, u64::from(rva), len)?;
        let found = find_quest_flags(mem, &data, u64::from(rva))?;
        self.live_base
            .checked_add(found)
            .context("quest flags address overflow")
    }

    /// Structural search for the unit table in `.data` (see [`find_unit_table`]); `.data` is
    /// taken from `image` if it was read, else read on its own.
    fn structural_rva(&self, mem: &impl ReadMem, header: &[u8], image: Option<&[u8]>) -> Result<u64> {
        let (rva, vsize) = section(header, b".data").context("no .data section in the PE header")?;
        let start = usize::try_from(rva)?;
        let size = usize::try_from(self.size)?;
        let end = start.saturating_add(usize::try_from(vsize)?).min(size);
        let read;
        let data = if let Some(img) = image {
            img.get(start..end).context(".data starts outside the image")?
        } else {
            let len = end.checked_sub(start).context(".data starts outside the image")?;
            read = self.read_alias(mem, u64::from(rva), len)?;
            &read
        };
        find_unit_table(mem, data, u64::from(rva))
    }
}

/// How [`Image::unit_table`] found the table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnitTableSource {
    Signature,
    Cache,
    Structure,
}

/// The located unit hash table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnitTable {
    /// Live address (pointers in the game refer to this).
    pub addr: u64,
    /// Offset from the image base.
    pub rva: u64,
    pub source: UnitTableSource,
    /// Cache key of the build (`"<TimeDateStamp>-<SizeOfImage>"`, hex).
    pub build: String,
}

/// RVA of the unit table from the code signature: the `rel32` after it, relative to the image base.
///
/// # Errors
/// Fails unless the signature matches exactly once (0 hits: the page is probably re-encrypted),
/// or if the `rel32` is negative.
fn signature_rva(image: &[u8]) -> Result<u64> {
    let sig = parse_pattern(UNIT_TABLE_SIG)?;
    let hits = find_pattern(image, &sig);
    let [hit] = hits.as_slice() else {
        bail!("expected 1 hit, found {} (code page encrypted?)", hits.len());
    };
    let at = hit.checked_add(sig.len()).context("rel32 offset overflow")?;
    let rel = image
        .get(at..at.saturating_add(4))
        .and_then(|b| b.try_into().ok())
        .map(i32::from_le_bytes)
        .context("rel32 past end of image")?;
    u64::try_from(rel).with_context(|| format!("negative rel32 {rel:#x}"))
}

/// Unit types with a block in the hash table (player, monster, object, missile, item, tile).
const TABLE_TYPES: std::ops::RangeInclusive<u32> = 0..=5;

/// Finds the unit hash table in `data` (the `.data` section, which starts at `data_rva`).
///
/// A candidate is an 8-aligned offset where, for every block `t` in [`TABLE_TYPES`]
/// (128 slots of 8 bytes at `candidate + t * 1024`), every non-null slot `s` points to a unit
/// whose type (`+0x00`) is `t` and whose id (`+0x08`) satisfies `id & 127 == s`; the player
/// block must be non-empty and at least 2 slots non-null overall. The id check is what rejects
/// the table's shifted neighbours, which pass the type check whenever the edge slots are null.
///
/// Raw slot values are filtered first (every non-null slot a plausible user pointer), using
/// prefix counts so each offset costs O(1); only survivors have their pointees read.
///
/// # Errors
/// Fails unless exactly one candidate is found; the error lists the candidate RVAs.
fn find_unit_table(mem: &impl ReadMem, data: &[u8], data_rva: u64) -> Result<u64> {
    let slots = usize::try_from(unit_table::SLOTS)?;
    let span = slots * (*TABLE_TYPES.end() as usize + 1);
    // A trailing partial word (none in practice: sections are page-aligned) is ignored.
    let words: Vec<u64> = data
        .as_chunks::<8>()
        .0
        .iter()
        .map(|&c| u64::from_le_bytes(c))
        .collect();
    // prefix[i] = number of words in words[..i] matching the predicate.
    let prefix = |pred: fn(u64) -> bool| {
        let mut acc = 0u32;
        std::iter::once(0)
            .chain(words.iter().map(|&w| {
                acc += u32::from(pred(w));
                acc
            }))
            .collect::<Vec<u32>>()
    };
    let bad = prefix(|w| w != 0 && !is_user_ptr(w));
    let nonnull = prefix(|w| w != 0);
    let count = |pfx: &[u32], lo: usize| -> u32 {
        let at = |i| pfx.get(i).copied().unwrap_or(0);
        at(lo + slots) - at(lo)
    };
    let block = |w: usize, t: u32| w + t as usize * slots;

    // Pointee (type, id), memoized: neighbouring candidates share most pointers.
    let mut units: HashMap<u64, Option<(u32, u32)>> = HashMap::new();
    let mut unit_at = |p: u64| {
        *units.entry(p).or_insert_with(|| {
            Some((
                mem.read_u32(p + unit::TYPE).ok()?,
                mem.read_u32(p + unit::ID).ok()?,
            ))
        })
    };

    let mut found = Vec::new();
    for w in 0..(words.len() + 1).saturating_sub(span) {
        let cheap = TABLE_TYPES.clone().all(|t| count(&bad, block(w, t)) == 0)
            && count(&nonnull, w) > 0
            && TABLE_TYPES
                .clone()
                .map(|t| count(&nonnull, block(w, t)))
                .sum::<u32>()
                >= 2;
        if !cheap {
            continue;
        }
        let deep = TABLE_TYPES.clone().all(|t| {
            let lo = block(w, t);
            words.get(lo..lo + slots).is_some_and(|blk| {
                blk.iter().zip(0u32..).all(|(&p, slot)| {
                    p == 0 || unit_at(p).is_some_and(|(ty, id)| ty == t && id & 127 == slot)
                })
            })
        });
        if deep {
            found.push(data_rva + u64::try_from(w * 8)?);
        }
    }
    match found.as_slice() {
        [rva] => Ok(*rva),
        [] => bail!("no candidate in .data ({:#x} bytes)", data.len()),
        many => bail!("{} candidates at rva {many:#x?}", many.len()),
    }
}

/// Finds the quest-flags global in `data` (the `.data` section, which starts at `data_rva`):
/// the first of exactly one pair of adjacent 8-byte slots that both point to a quest-record
/// header ([`d2r_core::quests::is_header`]). Live 2026-11-18 exactly two slots matched, adjacent;
/// the first holds the player's quest flags.
///
/// # Errors
/// Fails unless exactly one adjacent pair is found; the error lists the matching RVAs.
fn find_quest_flags(mem: &impl ReadMem, data: &[u8], data_rva: u64) -> Result<u64> {
    let len = d2r_core::offsets::quest_flags::HEADER_LEN;
    let hits: Vec<usize> = data
        .as_chunks::<8>()
        .0
        .iter()
        .enumerate()
        .filter(|&(_, &w)| is_user_ptr(u64::from_le_bytes(w)))
        .filter(|&(_, &w)| {
            mem.read_bytes(u64::from_le_bytes(w), len)
                .is_ok_and(|h| d2r_core::quests::is_header(&h))
        })
        .map(|(i, _)| i)
        .collect();
    let pairs: Vec<usize> = hits
        .windows(2)
        .filter_map(|w| match *w {
            [a, b] if b == a + 1 => Some(a),
            _ => None,
        })
        .collect();
    let rva = |i: usize| data_rva + (i as u64) * 8;
    match pairs.as_slice() {
        [first] => Ok(rva(*first)),
        _ => bail!(
            "expected one adjacent pair of quest-record slots in .data, found {:#x?}",
            hits.iter().map(|&i| rva(i)).collect::<Vec<_>>()
        ),
    }
}

/// Parses a pattern like `"48 03 C7 ?? 8B"`; `?` or `??` is a wildcard.
///
/// # Errors
/// Fails on a token that isn't two hex digits or a wildcard, or on an empty pattern.
pub fn parse_pattern(s: &str) -> Result<Vec<Option<u8>>> {
    let pat = s
        .split_whitespace()
        .map(|tok| match tok {
            "?" | "??" => Ok(None),
            _ if tok.len() == 2 => u8::from_str_radix(tok, 16)
                .map(Some)
                .with_context(|| format!("bad pattern byte {tok:?}")),
            _ => bail!("bad pattern byte {tok:?}"),
        })
        .collect::<Result<Vec<_>>>()?;
    ensure!(!pat.is_empty(), "empty pattern");
    Ok(pat)
}

/// All offsets in `haystack` where `pattern` matches (`None` matches any byte).
/// An empty pattern matches nowhere.
#[must_use]
pub fn find_pattern(haystack: &[u8], pattern: &[Option<u8>]) -> Vec<usize> {
    if pattern.is_empty() {
        return Vec::new(); // `windows(0)` would panic
    }
    haystack
        .windows(pattern.len())
        .enumerate()
        .filter(|(_, w)| w.iter().zip(pattern).all(|(b, p)| p.is_none_or(|p| p == *b)))
        .map(|(i, _)| i)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::parse_maps;
    use d2r_core::testutil::{FakeMem, temp_dir};

    const D2R_SIZE: u32 = 0x279_c000;
    /// `TimeDateStamp` of the live build at the time of writing.
    const TIMESTAMP: u32 = 0x6ad8_1202;
    /// `.data` of the synthetic images (inside the 0x18_0000-byte fixture).
    const DATA_RVA: u32 = 0x14_0000;
    const DATA_VSIZE: u32 = 0x3_0000;

    /// PE header with `TimeDateStamp`, `SizeOfImage` and a `.text` + `.data` section table.
    fn pe_header(size_of_image: u32) -> Vec<u8> {
        let mut h = vec![0u8; 0x400];
        let mut put = |at: usize, bytes: &[u8]| {
            h.splice(at..at + bytes.len(), bytes.iter().copied());
        };
        let pe = 0x80;
        put(0, b"MZ");
        put(0x3c, &0x80u32.to_le_bytes());
        put(pe, b"PE\0\0");
        put(pe + 6, &2u16.to_le_bytes()); // NumberOfSections
        put(pe + 8, &TIMESTAMP.to_le_bytes());
        put(pe + 20, &0xF0u16.to_le_bytes()); // SizeOfOptionalHeader (PE32+)
        put(pe + 24 + 56, &size_of_image.to_le_bytes());
        let sections = [(b".text", 0x1000, 0x1000), (b".data", DATA_RVA, DATA_VSIZE)];
        for (i, (name, rva, vsize)) in sections.into_iter().enumerate() {
            let at = pe + 24 + 0xF0 + i * 40;
            put(at, name);
            put(at + 8, &vsize.to_le_bytes());
            put(at + 12, &rva.to_le_bytes());
        }
        h
    }

    #[test]
    fn pe_size_of_image() {
        assert_eq!(size_of_image(&pe_header(D2R_SIZE)), Some(D2R_SIZE));
        assert_eq!(size_of_image(b"MZ"), None, "too short");
        let mut bad = pe_header(D2R_SIZE);
        bad.splice(0..2, *b"ZM");
        assert_eq!(size_of_image(&bad), None, "no MZ");
        let mut bad = pe_header(D2R_SIZE);
        bad.splice(0x80..0x84, *b"PE\0\x01");
        assert_eq!(size_of_image(&bad), None, "no PE sig");
        let mut bad = pe_header(D2R_SIZE);
        bad.splice(0x3c..0x40, 0xffff_fff0u32.to_le_bytes());
        assert_eq!(size_of_image(&bad), None, "e_lfanew out of range");
    }

    #[test]
    fn pe_build_and_sections() {
        let h = pe_header(D2R_SIZE);
        assert_eq!(time_date_stamp(&h), Some(TIMESTAMP));
        assert_eq!(build_key(&h).as_deref(), Some("6ad81202-279c000"));
        assert_eq!(section(&h, b".data"), Some((DATA_RVA, DATA_VSIZE)));
        assert_eq!(section(&h, b".text"), Some((0x1000, 0x1000)));
        assert_eq!(section(&h, b".dat"), None, "prefix of a name");
        assert_eq!(section(&h, b".rdata"), None);
        let mut few = h.clone();
        few.splice(0x86..0x88, 1u16.to_le_bytes());
        assert_eq!(section(&few, b".data"), None, "past NumberOfSections");
        assert_eq!(
            section(h.get(..0x1bc).unwrap_or_default(), b".data"),
            None,
            "truncated table"
        );
        assert_eq!(time_date_stamp(b"MZ"), None);
    }

    #[test]
    fn pattern_parse() -> Result<()> {
        assert_eq!(
            parse_pattern("48 03 C7 ?? 8b ?")?,
            vec![Some(0x48), Some(0x03), Some(0xc7), None, Some(0x8b), None]
        );
        assert!(parse_pattern("").is_err());
        assert!(parse_pattern("4").is_err());
        assert!(parse_pattern("480").is_err());
        assert!(parse_pattern("GG").is_err());
        Ok(())
    }

    #[test]
    fn pattern_match() -> Result<()> {
        let hay = [0x00, 0x48, 0x03, 0xAA, 0x8B, 0x48, 0x03, 0xBB, 0x8B, 0x48, 0x03];
        assert_eq!(find_pattern(&hay, &parse_pattern("48 03 ?? 8B")?), vec![1, 5]);
        assert_eq!(find_pattern(&hay, &parse_pattern("48 03 AA")?), vec![1]);
        assert_eq!(
            find_pattern(&hay, &parse_pattern("48 03 ?? 8B 48 03 ?? 8B 48 03 ?? 8B")?),
            Vec::<usize>::new()
        );
        assert_eq!(find_pattern(&hay, &[]), Vec::<usize>::new());
        assert_eq!(find_pattern(&[], &[None]), Vec::<usize>::new());
        Ok(())
    }

    /// Synthetic layout modelled on the live game: the alias, a decoy private PE, a decoy
    /// shared PE without a live view, and the multi-part live view with encrypted pages.
    const MAPS: &str = "\
033b0000-05b4c000 rw-s 00000000 00:01 77                         /memfd:wine-mapping (deleted)
10000000-12790000 r--p 00000000 fd:01 88                         /home/u/prefix/drive_c/other.exe
20000000-2279c000 rw-s 00000000 00:01 99                         /memfd:wine-mapping (deleted)
6ffff9a60000-6ffff9a61000 r--s 00000000 00:01 77                 /memfd:wine-mapping (deleted)
6ffff9a61000-6ffffb000000 ---s 00001000 00:01 77                 /memfd:wine-mapping (deleted)
6ffffb000000-6ffffc1fc000 rw-s 015a0000 00:01 77                 /memfd:wine-mapping (deleted)
7f0000000000-7f0000001000 rw-p 00000000 00:00 0
";

    fn fake_for_maps() -> FakeMem {
        let mut m = FakeMem::default();
        m.add(0x033b_0000, pe_header(D2R_SIZE));
        m.add(0x1000_0000, pe_header(0x279_0000));
        m.add(0x2000_0000, pe_header(D2R_SIZE));
        m.add(0x6fff_f9a6_0000, pe_header(D2R_SIZE));
        m
    }

    #[test]
    fn image_group_detection() -> Result<()> {
        let maps = parse_maps(MAPS)?;
        let img = find_image(&maps, &fake_for_maps())?;
        assert_eq!(
            img,
            Image {
                live_base: 0x6fff_f9a6_0000,
                alias_base: 0x033b_0000,
                size: 0x279_c000
            }
        );
        Ok(())
    }

    #[test]
    fn image_not_found_without_live_view() -> Result<()> {
        // Drop the live view's encrypted part: no pair left, and the decoys don't qualify.
        let text = MAPS
            .lines()
            .filter(|l| !l.contains("---s"))
            .collect::<Vec<_>>()
            .join("\n");
        let err = find_image(&parse_maps(&text)?, &fake_for_maps())
            .err()
            .context("expected error")?;
        assert!(err.to_string().contains("not found"), "{err}");
        Ok(())
    }

    /// Fake heap holding the units the synthetic tables point to.
    const HEAP: u64 = 0x7f00_0000_0000;
    /// Units on the fake heap as `(type, id)`; unit `n` lives at `unit_addr(n)`.
    const UNITS: [(u32, u32); 6] = [
        (0, 0x105), // 0: player, slot 5
        (1, 0x922), // 1: monster, slot 0x22
        (1, 0x980), // 2: monster, slot 0
        (4, 0x07),  // 3: item, slot 7
        (2, 0x13),  // 4: object, slot 0x13
        (4, 0x122), // 5: item with a monster's slot (wrong type when put in block 1, slot 0x22)
    ];
    /// The real table: `(block, slot, unit index)`.
    const REAL: [(u32, usize, usize); 5] = [(0, 5, 0), (1, 0x22, 1), (1, 0, 2), (4, 7, 3), (2, 0x13, 4)];

    fn unit_addr(n: usize) -> u64 {
        HEAP + n as u64 * 0x100
    }

    fn heap_mem() -> FakeMem {
        let mut heap = vec![0u8; UNITS.len() * 0x100];
        for (n, (ty, id)) in UNITS.into_iter().enumerate() {
            heap.splice(n * 0x100..n * 0x100 + 4, ty.to_le_bytes());
            heap.splice(n * 0x100 + 8..n * 0x100 + 12, id.to_le_bytes());
        }
        let mut mem = FakeMem::default();
        mem.add(HEAP, heap);
        mem
    }

    /// Writes a table at `off` in `data`; entries are `(block, slot, pointer)`.
    fn put_table(data: &mut [u8], off: usize, slots: impl IntoIterator<Item = (u32, usize, u64)>) {
        for (t, slot, ptr) in slots {
            let at = off + (t as usize * 128 + slot) * 8;
            data.get_mut(at..at + 8)
                .into_iter()
                .for_each(|d| d.copy_from_slice(&ptr.to_le_bytes()));
        }
    }

    fn real_slots() -> impl Iterator<Item = (u32, usize, u64)> {
        REAL.into_iter().map(|(t, s, n)| (t, s, unit_addr(n)))
    }

    /// Table entries: `(block, slot, pointer)`.
    type Slots = Vec<(u32, usize, u64)>;

    fn data_with(tables: &[(usize, Slots)]) -> Vec<u8> {
        let mut data = vec![0u8; DATA_VSIZE as usize];
        for (off, slots) in tables {
            put_table(&mut data, *off, slots.iter().copied());
        }
        data
    }

    const DRVA: u64 = DATA_RVA as u64;

    #[test]
    fn structure_finds_real_not_empty_player_decoy() -> Result<()> {
        // Decoy: monsters and items, but no player.
        let decoy = REAL
            .into_iter()
            .filter(|e| e.0 != 0)
            .map(|(t, s, n)| (t, s, unit_addr(n)))
            .collect();
        let data = data_with(&[(0x1000, decoy), (0x2_0000, real_slots().collect())]);
        assert_eq!(find_unit_table(&heap_mem(), &data, DRVA)?, DRVA + 0x2_0000);
        Ok(())
    }

    #[test]
    fn structure_rejects_wrong_type_pointee() -> Result<()> {
        // Unit 5 is an item whose id fits slot 0x22, put in the monster block.
        let mut wrong: Vec<_> = real_slots().filter(|e| e.0 != 1).collect();
        wrong.push((1, 0x22, unit_addr(5)));
        let data = data_with(&[(0x1000, wrong.clone()), (0x2_0000, real_slots().collect())]);
        assert_eq!(find_unit_table(&heap_mem(), &data, DRVA)?, DRVA + 0x2_0000);
        let err = find_unit_table(&heap_mem(), &data_with(&[(0x1000, wrong)]), DRVA)
            .err()
            .context("expected error")?;
        assert!(err.to_string().contains("no candidate"), "{err}");
        Ok(())
    }

    #[test]
    fn structure_rejects_shifted_decoy() -> Result<()> {
        // The player one slot off, the other blocks intact: every type matches at this offset,
        // only the id hash (0x105 & 127 = 5, not 6) rejects it. At offset +8 the player block
        // lines up but the monster in block 1 slot 0 lands in block 0, so that fails on type.
        // (Shifting every block alike would just be the real table 8 bytes later.)
        let shifted: Vec<_> = real_slots()
            .map(|(t, s, p)| (t, if t == 0 { s + 1 } else { s }, p))
            .collect();
        let err = find_unit_table(&heap_mem(), &data_with(&[(0x1000, shifted.clone())]), DRVA)
            .err()
            .context("expected error")?;
        assert!(err.to_string().contains("no candidate"), "{err}");
        // With the real table present, neither the decoy nor the real table's ±8 neighbours
        // (which pass the type check: their edge slots are null) are reported.
        let data = data_with(&[(0x1000, shifted), (0x2_0000, real_slots().collect())]);
        assert_eq!(find_unit_table(&heap_mem(), &data, DRVA)?, DRVA + 0x2_0000);
        Ok(())
    }

    #[test]
    fn structure_rejects_unreadable_pointee_and_reports_ambiguity() -> Result<()> {
        let mut dangling: Vec<_> = real_slots().collect();
        dangling.push((1, 0x40, 0x6000_0000_0000)); // plausible but unmapped
        let err = find_unit_table(&heap_mem(), &data_with(&[(0x1000, dangling)]), DRVA)
            .err()
            .context("expected error")?;
        assert!(err.to_string().contains("no candidate"), "{err}");

        let two = data_with(&[
            (0x1000, real_slots().collect()),
            (0x2_0000, real_slots().collect()),
        ]);
        let err = find_unit_table(&heap_mem(), &two, DRVA)
            .err()
            .context("expected error")?;
        assert!(err.to_string().contains("2 candidates"), "{err}");
        assert!(err.to_string().contains("0x141000"), "{err}");
        Ok(())
    }

    /// Table RVA in the full-image fixture.
    const TABLE_RVA: u64 = DRVA + 0x2_0000;
    /// Image size of the full-image fixture.
    const FIXTURE_SIZE: u32 = 0x18_0000;

    /// Image fixture: PE header, signatures at `sig_rvas` with `rel`, and optionally the real
    /// table in `.data`. Also maps the fake heap.
    fn image_fixture(sig_rvas: &[usize], rel: i32, with_table: bool) -> (Image, FakeMem) {
        // > 1 MiB so the chunked read is exercised.
        let size = FIXTURE_SIZE as usize;
        let mut bytes = vec![0xCCu8; size];
        bytes.splice(0..0x400, pe_header(FIXTURE_SIZE));
        bytes.splice(
            DATA_RVA as usize..(DATA_RVA + DATA_VSIZE) as usize,
            data_with(&[]),
        );
        if with_table {
            put_table(&mut bytes, (DATA_RVA + 0x2_0000) as usize, real_slots());
        }
        for &rva in sig_rvas {
            let code = [0x48, 0x03, 0xC7, 0x49, 0x8B, 0x8C, 0xC6];
            bytes.splice(rva..rva + 7, code);
            bytes.splice(rva + 7..rva + 11, rel.to_le_bytes());
        }
        let img = Image {
            live_base: 0x6fff_f9a6_0000,
            alias_base: 0x033b_0000,
            size: u64::from(FIXTURE_SIZE),
        };
        let mut mem = heap_mem();
        mem.add(img.alias_base, bytes);
        (img, mem)
    }

    const BUILD: &str = "6ad81202-180000";

    #[test]
    fn unit_table_by_signature_is_cached() -> Result<()> {
        let cache = temp_dir("sig")?.join("offsets.toml");
        // Straddles the first 1 MiB chunk boundary.
        let (img, mem) = image_fixture(&[0x10_0000 - 3], 0x12_3450, false);
        let t = img.unit_table(&mem, Some(&cache), true)?;
        assert_eq!(t.source, UnitTableSource::Signature);
        assert_eq!((t.addr, t.rva), (img.live_base + 0x12_3450, 0x12_3450));
        assert_eq!(t.build, BUILD);
        assert_eq!(offset_cache::read(&cache)?.get(BUILD), Some(&0x12_3450));
        Ok(())
    }

    #[test]
    fn unit_table_signature_miss_uses_cache() -> Result<()> {
        let cache = temp_dir("cachehit")?.join("offsets.toml");
        offset_cache::store(&cache, BUILD, 0x1234)?;
        // No signature, and the table is present: the cache still wins over the search.
        let (img, mem) = image_fixture(&[], 0, true);
        let t = img.unit_table(&mem, Some(&cache), true)?;
        assert_eq!(t.source, UnitTableSource::Cache);
        assert_eq!(t.addr, img.live_base + 0x1234);
        // Two hits are a miss too.
        let (img, mem) = image_fixture(&[0x100, 0x200], 0x10, false);
        assert_eq!(
            img.unit_table(&mem, Some(&cache), true)?.source,
            UnitTableSource::Cache
        );
        Ok(())
    }

    #[test]
    fn unit_table_cache_miss_uses_structure_then_cache() -> Result<()> {
        let cache = temp_dir("structure")?.join("offsets.toml");
        offset_cache::store(&cache, "00000000-1", 0x99)?; // another build
        let (img, mem) = image_fixture(&[], 0, true);
        let t = img.unit_table(&mem, Some(&cache), true)?;
        assert_eq!(t.source, UnitTableSource::Structure);
        assert_eq!(t.addr, img.live_base + TABLE_RVA);
        assert_eq!(offset_cache::read(&cache)?.get(BUILD), Some(&TABLE_RVA));
        let t = img.unit_table(&mem, Some(&cache), true)?;
        assert_eq!((t.source, t.rva), (UnitTableSource::Cache, TABLE_RVA));
        Ok(())
    }

    #[test]
    fn unit_table_retry_reads_only_header_and_data() -> Result<()> {
        let cache = temp_dir("retry")?.join("offsets.toml");
        let (img, full) = image_fixture(&[0x10_0000], 0x1234, true);
        let bytes = full.read_bytes(img.alias_base, FIXTURE_SIZE as usize)?;
        // Map only the header and `.data` of the alias: a full read fails.
        let mut mem = heap_mem();
        mem.add(img.alias_base, bytes.get(..0x400).context("header")?.to_vec());
        let data = DATA_RVA as usize..(DATA_RVA + DATA_VSIZE) as usize;
        mem.add(
            img.alias_base + u64::from(DATA_RVA),
            bytes.get(data).context("data")?.to_vec(),
        );
        assert!(
            img.unit_table(&mem, Some(&cache), true).is_err(),
            "full read needs the whole image"
        );
        let t = img.unit_table(&mem, Some(&cache), false)?;
        assert_eq!(
            (t.source, t.rva),
            (UnitTableSource::Structure, TABLE_RVA),
            "signature skipped"
        );
        Ok(())
    }

    #[test]
    fn unit_table_all_methods_fail() -> Result<()> {
        let cache = temp_dir("fail")?.join("offsets.toml");
        let (img, mem) = image_fixture(&[0x100, 0x200], 0, false);
        let err = img
            .unit_table(&mem, Some(&cache), true)
            .err()
            .context("expected error")?;
        let msg = err.to_string();
        assert!(msg.contains("are you in a game"), "{msg}");
        assert!(msg.contains("found 2"), "{msg}");
        assert!(
            msg.contains(&format!("no cached offset for build {BUILD}")),
            "{msg}"
        );
        assert!(msg.contains("no candidate"), "{msg}");
        assert!(!cache.exists(), "nothing cached on failure");
        let err = img.unit_table(&mem, None, true).err().context("expected error")?;
        assert!(err.to_string().contains("no cache path"), "{err}");
        // A negative rel32 is rejected rather than used.
        let (img, mem) = image_fixture(&[0x7_1fd3], -0x1000, false);
        let err = img.unit_table(&mem, None, true).err().context("expected error")?;
        assert!(err.to_string().contains("negative rel32"), "{err}");
        Ok(())
    }

    #[test]
    fn read_alias_bounds() -> Result<()> {
        let (img, mem) = image_fixture(&[0x10_0000], 0x1234, false);
        assert_eq!(img.read_alias(&mem, 0x10_0000, 3)?, vec![0x48, 0x03, 0xC7]);
        assert!(img.read_alias(&mem, img.size - 1, 2).is_err());
        Ok(())
    }

    /// A quest-record header at `at` in `heap` (`heap` starts at `HEAP`): the buffer right
    /// after it, 0x300 bits, 0x60 bytes.
    fn put_quest_header(heap: &mut [u8], at: usize) {
        let buf = (HEAP + at as u64 + 0x30).to_le_bytes();
        for (off, bytes) in [
            (0, &buf[..]),
            (8, &0x300u32.to_le_bytes()[..]),
            (0x28, &0x60u64.to_le_bytes()[..]),
        ] {
            heap.get_mut(at + off..at + off + bytes.len())
                .into_iter()
                .for_each(|d| d.copy_from_slice(bytes));
        }
    }

    #[test]
    fn quest_flags_take_the_first_of_the_adjacent_pair() -> Result<()> {
        let mut heap = vec![0u8; 0x400];
        put_quest_header(&mut heap, 0x100);
        put_quest_header(&mut heap, 0x200);
        put_quest_header(&mut heap, 0x300);
        let mut mem = FakeMem::default();
        mem.add(HEAP, heap);
        let mut data = vec![0u8; 0x1000];
        let put = |data: &mut [u8], off: usize, p: u64| {
            data.get_mut(off..off + 8)
                .into_iter()
                .for_each(|d| d.copy_from_slice(&p.to_le_bytes()));
        };
        // A lone header pointer (decoy), a pointer to something else, then the pair.
        put(&mut data, 0x40, HEAP + 0x300);
        put(&mut data, 0x80, HEAP + 0x10);
        put(&mut data, 0x800, HEAP + 0x100);
        put(&mut data, 0x808, HEAP + 0x200);
        assert_eq!(find_quest_flags(&mem, &data, DRVA)?, DRVA + 0x800);
        // Without the pair: an error naming the lone hit.
        put(&mut data, 0x808, 0);
        let err = find_quest_flags(&mem, &data, DRVA)
            .err()
            .context("expected error")?;
        assert!(err.to_string().contains("adjacent pair"), "{err}");
        Ok(())
    }
}

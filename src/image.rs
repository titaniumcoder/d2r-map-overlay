//! Locating the D2R.exe image in the game's memory and scanning it for signatures.
//!
//! D2R's code pages are encrypted, so the live image view contains no-access (`---s`) pages.
//! Wine maps the image from a memfd, and the same memfd is also mapped elsewhere as one fully
//! readable `rw-s` alias. We scan the alias and compute addresses relative to the live base.

use anyhow::{Context, Result, bail, ensure};

use crate::process::{MapEntry, ReadMem};

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

/// `SizeOfImage` from a PE header (`MZ`, `e_lfanew` at 0x3c, `PE\0\0`, optional header +56).
fn size_of_image(header: &[u8]) -> Option<u32> {
    if header.get(..2)? != b"MZ" {
        return None;
    }
    let pe = usize::try_from(le_u32(header, 0x3c)?).ok()?;
    if header.get(pe..pe.checked_add(4)?)? != b"PE\0\0" {
        return None;
    }
    // COFF file header is 20 bytes after the 4-byte signature; SizeOfImage is at +56 in the optional header.
    le_u32(header, pe.checked_add(4 + 20 + 56)?)
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

    /// Address of the unit hash table: `live_base + rel32`, where `rel32` follows the signature.
    ///
    /// # Errors
    /// Fails if the image can't be read or the signature doesn't match exactly once.
    pub fn unit_table(&self, mem: &impl ReadMem) -> Result<u64> {
        let image = self.read_all(mem)?;
        let sig = parse_pattern(UNIT_TABLE_SIG)?;
        let hits = find_pattern(&image, &sig);
        let [hit] = hits.as_slice() else {
            bail!("unit table signature: expected 1 hit, found {}", hits.len());
        };
        let at = hit.checked_add(sig.len()).context("rel32 offset overflow")?;
        let rel = image
            .get(at..at.saturating_add(4))
            .and_then(|b| b.try_into().ok())
            .map(i32::from_le_bytes)
            .context("unit table rel32 past end of image")?;
        self.live_base
            .checked_add_signed(i64::from(rel))
            .context("unit table address overflow")
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
    use crate::testutil::FakeMem;

    const D2R_SIZE: u32 = 0x279_c000;

    fn pe_header(size_of_image: u32) -> Vec<u8> {
        let mut h = vec![0u8; 0x400];
        h.splice(0..2, *b"MZ");
        h.splice(0x3c..0x40, 0x80u32.to_le_bytes());
        h.splice(0x80..0x84, *b"PE\0\0");
        let at = 0x80 + 24 + 56;
        h.splice(at..at + 4, size_of_image.to_le_bytes());
        h.truncate(0x400);
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

    fn unit_table_fixture(sig_rvas: &[usize], rel: i32) -> (Image, FakeMem) {
        // > 1 MiB so the chunked read is exercised.
        let size = 0x18_0000;
        let mut bytes = vec![0xCCu8; size];
        for &rva in sig_rvas {
            let code = [0x48, 0x03, 0xC7, 0x49, 0x8B, 0x8C, 0xC6];
            bytes.splice(rva..rva + 7, code);
            bytes.splice(rva + 7..rva + 11, rel.to_le_bytes());
        }
        let img = Image {
            live_base: 0x6fff_f9a6_0000,
            alias_base: 0x033b_0000,
            size: size as u64,
        };
        let mut mem = FakeMem::default();
        mem.add(img.alias_base, bytes);
        (img, mem)
    }

    #[test]
    fn unit_table_address() -> Result<()> {
        // Straddles the first 1 MiB chunk boundary.
        let (img, mem) = unit_table_fixture(&[0x10_0000 - 3], 0x1eb_e5f0);
        assert_eq!(img.unit_table(&mem)?, 0x6fff_f9a6_0000 + 0x1eb_e5f0);
        let (img, mem) = unit_table_fixture(&[0x7_1fd3], -0x1000);
        assert_eq!(img.unit_table(&mem)?, 0x6fff_f9a5_f000);
        Ok(())
    }

    #[test]
    fn unit_table_hit_count() -> Result<()> {
        let (img, mem) = unit_table_fixture(&[], 0);
        let err = img.unit_table(&mem).err().context("expected error")?;
        assert!(err.to_string().contains("found 0"), "{err}");
        let (img, mem) = unit_table_fixture(&[0x100, 0x200], 0);
        let err = img.unit_table(&mem).err().context("expected error")?;
        assert!(err.to_string().contains("found 2"), "{err}");
        Ok(())
    }

    #[test]
    fn read_alias_bounds() -> Result<()> {
        let (img, mem) = unit_table_fixture(&[0x10], 0x1234);
        assert_eq!(img.read_alias(&mem, 0x10, 3)?, vec![0x48, 0x03, 0xC7]);
        assert!(img.read_alias(&mem, img.size - 1, 2).is_err());
        Ok(())
    }
}

//! Ground drops worth showing (unique, set, gems, runes, charms) and the game's item-code table.
//!
//! An item unit only carries its items.txt row (`unit::TXT`); the 4-character item code
//! (`gcg `, `r01 `) lives in the game's items table, an array on the heap whose address differs
//! per run. [`find_table`] locates it by scanning anonymous memory once per process.

use std::time::Instant;

use memchr::memmem;
use serde::{Deserialize, Serialize};

use crate::offsets::{item_data, items_txt};
use crate::process::{self, MapEntry, ReadMem};

/// Gem colour (skulls count as a gem colour here: they drop and are drawn like gems).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum GemColour {
    Amethyst,
    Topaz,
    Sapphire,
    Emerald,
    Ruby,
    Diamond,
    Skull,
}

/// Charm size: small (`cm1`), large (`cm2`), grand (`cm3`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CharmSize {
    Small,
    Large,
    Grand,
}

/// Why a ground item is shown. Variant order is draw order (unique on top).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DropKind {
    Gem(GemColour),
    /// Rune number 1 (El) ..= 33 (Zod).
    Rune(u8),
    /// A charm of any quality; `unique` (Annihilus, Torch, Gheed's) keeps the unique colour.
    /// JSON: `{"charm":{"size":"grand"}}`, plus `"unique":true` for a unique one.
    Charm {
        size: CharmSize,
        #[serde(default, skip_serializing_if = "std::ops::Not::not")]
        unique: bool,
    },
    Set,
    Unique,
}

/// Every gem and skull code of classic misc.txt (chipped, flawed, normal, flawless, perfect).
/// Note the flawless amethyst is `gzv`, not `glv`.
const GEMS: [([u8; 3], GemColour); 35] = {
    use GemColour::{Amethyst, Diamond, Emerald, Ruby, Sapphire, Skull, Topaz};
    [
        (*b"gcv", Amethyst),
        (*b"gfv", Amethyst),
        (*b"gsv", Amethyst),
        (*b"gzv", Amethyst),
        (*b"gpv", Amethyst),
        (*b"gcy", Topaz),
        (*b"gfy", Topaz),
        (*b"gsy", Topaz),
        (*b"gly", Topaz),
        (*b"gpy", Topaz),
        (*b"gcb", Sapphire),
        (*b"gfb", Sapphire),
        (*b"gsb", Sapphire),
        (*b"glb", Sapphire),
        (*b"gpb", Sapphire),
        (*b"gcg", Emerald),
        (*b"gfg", Emerald),
        (*b"gsg", Emerald),
        (*b"glg", Emerald),
        (*b"gpg", Emerald),
        (*b"gcr", Ruby),
        (*b"gfr", Ruby),
        (*b"gsr", Ruby),
        (*b"glr", Ruby),
        (*b"gpr", Ruby),
        (*b"gcw", Diamond),
        (*b"gfw", Diamond),
        (*b"gsw", Diamond),
        (*b"glw", Diamond),
        (*b"gpw", Diamond),
        (*b"skc", Skull),
        (*b"skf", Skull),
        (*b"sku", Skull),
        (*b"skl", Skull),
        (*b"skz", Skull),
    ]
};

/// Highest rune number (`r33`, Zod).
const MAX_RUNE: u8 = 33;

/// Charm codes of classic misc.txt.
const CHARMS: [([u8; 3], CharmSize); 3] = [
    (*b"cm1", CharmSize::Small),
    (*b"cm2", CharmSize::Large),
    (*b"cm3", CharmSize::Grand),
];

/// The kind of a gem, skull, rune or charm by its 4-byte code (3 letters + space); `None`
/// otherwise. A charm comes back as not unique: only [`classify`] knows the quality.
pub fn code_kind(code: [u8; 4]) -> Option<DropKind> {
    let [a, b, c, b' '] = code else { return None };
    if let Some(&(_, size)) = CHARMS.iter().find(|(ch, _)| *ch == [a, b, c]) {
        return Some(DropKind::Charm { size, unique: false });
    }
    if let Some(&(_, colour)) = GEMS.iter().find(|(g, _)| *g == [a, b, c]) {
        return Some(DropKind::Gem(colour));
    }
    if a == b'r' && b.is_ascii_digit() && c.is_ascii_digit() {
        let n = (b - b'0') * 10 + (c - b'0');
        if (1..=MAX_RUNE).contains(&n) {
            return Some(DropKind::Rune(n));
        }
    }
    None
}

/// Whether a ground item is shown: a charm of any quality first (so a unique charm stays a
/// charm), then unique or set by quality (any base), else gem/rune by code.
/// `code` is `None` if the items table wasn't found; then only unique and set items are shown.
pub fn classify(quality: u32, code: Option<[u8; 4]>) -> Option<DropKind> {
    let kind = code.and_then(code_kind);
    if let Some(DropKind::Charm { size, .. }) = kind {
        return Some(DropKind::Charm {
            size,
            unique: quality == item_data::QUALITY_UNIQUE,
        });
    }
    match quality {
        item_data::QUALITY_UNIQUE => Some(DropKind::Unique),
        item_data::QUALITY_SET => Some(DropKind::Set),
        _ => kind,
    }
}

/// The code of items.txt row `txt`, from the items table at `table`; `None` if unreadable.
pub fn read_code(mem: &impl ReadMem, table: u64, txt: u32) -> Option<[u8; 4]> {
    let addr = u64::from(txt)
        .checked_mul(items_txt::STRIDE)
        .and_then(|o| o.checked_add(items_txt::CODE))
        .and_then(|o| table.checked_add(o))?;
    mem.read_array(addr).ok()
}

/// The code as text, trailing padding removed (`"gcg"`).
pub fn code_text(code: [u8; 4]) -> String {
    String::from_utf8_lossy(&code)
        .trim_end_matches([' ', '\0'])
        .to_owned()
}

/// Codes of the first items.txt rows (classic weapons.txt order: hand axe, axe, double axe,
/// military pick, war axe). Matching all five, one record apart, identifies the table.
const SIGNATURE: [[u8; 4]; 5] = [*b"hax ", *b"axe ", *b"2ax ", *b"mpi ", *b"wax "];
/// Bytes read per `/proc/<pid>/mem` call while scanning.
const CHUNK: u64 = 8 << 20;
/// Consecutive chunks overlap by a code minus one byte, so a code across a border is still seen.
const OVERLAP: u64 = 3;

/// Result of a table scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Scan {
    /// Address of row 0's record, if found.
    pub base: Option<u64>,
    /// Bytes read until the table was found (or in total).
    pub scanned: u64,
}

/// Searches readable anonymous memory for the items table.
pub fn find_table(mem: &impl ReadMem, maps: &[MapEntry]) -> Scan {
    scan(mem, maps, CHUNK)
}

/// True for mappings the table can be in: readable, not backed by a file, not a kernel page.
fn is_scan_region(e: &MapEntry) -> bool {
    e.perms.read
        && e.inode == 0
        && !matches!(
            e.pathname.as_str(),
            "[vvar]" | "[vvar_vclock]" | "[vsyscall]" | "[vdso]"
        )
}

/// [`find_table`] with a configurable read size (tests use a tiny one to cross chunk borders).
fn scan(mem: &impl ReadMem, maps: &[MapEntry], chunk: u64) -> Scan {
    let finder = memmem::Finder::new(&SIGNATURE[0]);
    let mut buf = vec![0u8; usize::try_from(chunk).unwrap_or(0)];
    let mut scanned = 0u64;
    for region in maps.iter().filter(|e| is_scan_region(e)) {
        let mut start = region.start;
        while start < region.end {
            let len = (region.end - start).min(chunk);
            let Some(window) = usize::try_from(len).ok().and_then(|l| buf.get_mut(..l)) else {
                break;
            };
            // Pages can vanish or be guard pages: skip what can't be read.
            if mem.read(start, window).is_ok() {
                scanned = scanned.saturating_add(len);
                for pos in finder.find_iter(window) {
                    let Ok(pos) = u64::try_from(pos) else { continue };
                    let candidate = start + pos;
                    if is_table(mem, candidate) {
                        return Scan {
                            base: Some(candidate),
                            scanned,
                        };
                    }
                }
            }
            if start + len >= region.end || len <= OVERLAP {
                break;
            }
            start += len - OVERLAP;
        }
    }
    Scan { base: None, scanned }
}

/// True if every [`SIGNATURE`] code sits one record apart from `base`.
fn is_table(mem: &impl ReadMem, base: u64) -> bool {
    (0u64..).zip(SIGNATURE).all(|(i, code)| {
        i.checked_mul(items_txt::STRIDE)
            .and_then(|o| base.checked_add(o + items_txt::CODE))
            .and_then(|a| mem.read_array::<4>(a).ok())
            == Some(code)
    })
}

/// Finds the items table of process `pid` and logs the result and the time it took.
/// `None` (logged) if it can't be found: then only unique and set drops are shown.
pub fn locate(pid: u32, mem: &impl ReadMem) -> Option<u64> {
    let started = Instant::now();
    let maps = match process::read_maps(pid) {
        Ok(maps) => maps,
        Err(e) => {
            eprintln!("items table: {e:#}; gems and runes are not shown");
            return None;
        }
    };
    let Scan { base, scanned } = find_table(mem, &maps);
    let (mib, ms) = (scanned >> 20, started.elapsed().as_millis());
    match base {
        Some(b) => eprintln!("items table at {b:#x} (scanned {mib} MiB in {ms} ms)"),
        None => {
            eprintln!("items table not found (scanned {mib} MiB in {ms} ms); gems and runes are not shown");
        }
    }
    base
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::Perms;
    use crate::testutil::FakeMem;

    #[test]
    fn every_gem_and_skull() {
        let colours = [
            ("v", GemColour::Amethyst),
            ("y", GemColour::Topaz),
            ("b", GemColour::Sapphire),
            ("g", GemColour::Emerald),
            ("r", GemColour::Ruby),
            ("w", GemColour::Diamond),
        ];
        for (letter, colour) in colours {
            // Flawless amethyst is the odd one out: `gzv`.
            let flawless = if letter == "v" { "gz" } else { "gl" };
            for grade in ["gc", "gf", "gs", flawless, "gp"] {
                let code = format!("{grade}{letter} ");
                let bytes: [u8; 4] = code.as_bytes().try_into().unwrap_or_default();
                assert_eq!(code_kind(bytes), Some(DropKind::Gem(colour)), "{code}");
            }
        }
        for code in [b"skc ", b"skf ", b"sku ", b"skl ", b"skz "] {
            assert_eq!(code_kind(*code), Some(DropKind::Gem(GemColour::Skull)));
        }
        // Near misses: not in the list.
        for code in [b"glv ", b"gzy ", b"gcx ", b"sk  ", b"gcg\0", b"gcgx", b"hax "] {
            assert_eq!(code_kind(*code), None, "{:?}", code_text(*code));
        }
    }

    #[test]
    fn every_rune() {
        for n in 1..=33u8 {
            let code = format!("r{n:02} ");
            let bytes: [u8; 4] = code.as_bytes().try_into().unwrap_or_default();
            assert_eq!(code_kind(bytes), Some(DropKind::Rune(n)), "{code}");
        }
        for code in [b"r00 ", b"r34 ", b"r99 ", b"r1  ", b"rab ", b"r01x"] {
            assert_eq!(code_kind(*code), None, "{:?}", code_text(*code));
        }
    }

    #[test]
    fn every_charm() {
        let charm = |size| Some(DropKind::Charm { size, unique: false });
        assert_eq!(code_kind(*b"cm1 "), charm(CharmSize::Small));
        assert_eq!(code_kind(*b"cm2 "), charm(CharmSize::Large));
        assert_eq!(code_kind(*b"cm3 "), charm(CharmSize::Grand));
        for code in [b"cm0 ", b"cm4 ", b"cm  ", b"cm1\0", b"cm1x", b"cn1 ", b"CM1 "] {
            assert_eq!(code_kind(*code), None, "{:?}", code_text(*code));
        }
    }

    #[test]
    fn charms_before_quality() {
        assert_eq!(
            classify(7, Some(*b"cm3 ")),
            Some(DropKind::Charm {
                size: CharmSize::Grand,
                unique: true
            }),
            "Gheed's: a charm, still marked unique"
        );
        assert_eq!(
            classify(4, Some(*b"cm1 ")),
            Some(DropKind::Charm {
                size: CharmSize::Small,
                unique: false
            }),
            "magic small charm"
        );
        for q in [1, 2, 3, 5, 6] {
            assert_eq!(
                classify(q, Some(*b"cm2 ")),
                Some(DropKind::Charm {
                    size: CharmSize::Large,
                    unique: false
                }),
                "quality {q}"
            );
        }
        assert_eq!(classify(4, None), None, "charm without a table");
    }

    #[test]
    fn charm_json() -> serde_json::Result<()> {
        let plain = DropKind::Charm {
            size: CharmSize::Grand,
            unique: false,
        };
        let unique = DropKind::Charm {
            size: CharmSize::Small,
            unique: true,
        };
        assert_eq!(serde_json::to_string(&plain)?, r#"{"charm":{"size":"grand"}}"#);
        assert_eq!(
            serde_json::to_string(&unique)?,
            r#"{"charm":{"size":"small","unique":true}}"#
        );
        assert_eq!(
            serde_json::from_str::<DropKind>(r#"{"charm":{"size":"grand"}}"#)?,
            plain
        );
        assert_eq!(
            serde_json::from_str::<DropKind>(r#"{"charm":{"size":"small","unique":true}}"#)?,
            unique
        );
        Ok(())
    }

    #[test]
    fn quality_classification() {
        assert_eq!(classify(7, Some(*b"hax ")), Some(DropKind::Unique));
        assert_eq!(classify(5, None), Some(DropKind::Set), "no code needed");
        assert_eq!(
            classify(2, Some(*b"gcg ")),
            Some(DropKind::Gem(GemColour::Emerald))
        );
        assert_eq!(classify(2, Some(*b"r33 ")), Some(DropKind::Rune(33)));
        for q in [1, 2, 3, 4, 6, 8] {
            assert_eq!(classify(q, Some(*b"hax ")), None, "quality {q}");
        }
        assert_eq!(classify(2, None), None, "gem without a table");
    }

    #[test]
    fn code_text_trims() {
        assert_eq!(code_text(*b"gcg "), "gcg");
        assert_eq!(code_text(*b"r01\0"), "r01");
    }

    const STRIDE: usize = 0x1C0;

    /// A table of `rows` records with `codes` at their row indices.
    fn table_bytes(rows: usize, codes: &[(usize, &[u8; 4])]) -> Vec<u8> {
        let mut b = vec![0xAAu8; rows * STRIDE];
        for &(i, code) in codes {
            if let Some(dst) = b.get_mut(i * STRIDE..i * STRIDE + 4) {
                dst.copy_from_slice(code);
            }
        }
        b
    }

    fn signature_rows() -> Vec<(usize, &'static [u8; 4])> {
        vec![
            (0, b"hax "),
            (1, b"axe "),
            (2, b"2ax "),
            (3, b"mpi "),
            (4, b"wax "),
        ]
    }

    fn map(start: u64, len: usize, read: bool, inode: u64, pathname: &str) -> MapEntry {
        MapEntry {
            start,
            end: start + u64::try_from(len).unwrap_or(0),
            perms: Perms {
                read,
                write: true,
                ..Perms::default()
            },
            offset: 0,
            inode,
            pathname: pathname.into(),
        }
    }

    #[test]
    fn finds_table_and_reads_codes() {
        let mut mem = FakeMem::default();
        let mut maps = Vec::new();
        // 1. A decoy: `hax ` followed by `axe ` but not the rest.
        let decoy = table_bytes(6, &[(0, b"hax "), (1, b"axe "), (2, b"3ax ")]);
        maps.push(map(0x10_0000, decoy.len(), true, 0, ""));
        mem.add(0x10_0000, decoy);
        // 2. A real-looking table in a file-backed mapping: ignored.
        let file = table_bytes(6, &signature_rows());
        maps.push(map(0x20_0000, file.len(), true, 77, "/game/data.bin"));
        mem.add(0x20_0000, file);
        // 3. Listed but unreadable: skipped.
        maps.push(map(0x30_0000, 0x1000, true, 0, ""));
        // 4. Unreadable per perms (contains a table): skipped.
        let noread = table_bytes(6, &signature_rows());
        maps.push(map(0x40_0000, noread.len(), false, 0, ""));
        mem.add(0x40_0000, noread);
        // 5. The table, at an odd address inside an anonymous region, after some bytes.
        let mut rows = signature_rows();
        rows.extend([(587, b"gcg "), (625, b"r01 ")]);
        let mut heap = vec![0u8; 0x1235];
        heap.extend(table_bytes(700, &rows));
        maps.push(map(0x7f00_0000_0000, heap.len(), true, 0, "[heap]"));
        mem.add(0x7f00_0000_0000, heap);
        let want = 0x7f00_0000_1235;

        // A chunk size that doesn't divide anything, so needles fall across chunk borders.
        for chunk in [0x101, 0x3FE, CHUNK] {
            let s = scan(&mem, &maps, chunk);
            assert_eq!(s.base, Some(want), "chunk {chunk:#x}");
            assert!(s.scanned > 0);
        }
        assert_eq!(read_code(&mem, want, 587), Some(*b"gcg "));
        assert_eq!(read_code(&mem, want, 625), Some(*b"r01 "));
        assert_eq!(read_code(&mem, want, 1), Some(*b"axe "));
        assert_eq!(read_code(&mem, want, 100_000), None, "past the table");
        assert_eq!(read_code(&mem, u64::MAX - 4, 2), None, "overflow");
    }

    #[test]
    fn no_table_found() {
        let mut mem = FakeMem::default();
        let decoy = table_bytes(6, &[(0, b"hax "), (1, b"axe "), (2, b"2ax "), (3, b"mpi ")]);
        let maps = vec![map(0x10_0000, decoy.len(), true, 0, "")];
        mem.add(0x10_0000, decoy);
        let s = scan(&mem, &maps, 0x200);
        assert_eq!(s.base, None, "four of five signature codes is not enough");
        assert_eq!(
            s.scanned,
            u64::try_from(6 * STRIDE).unwrap_or(0) + 3 * 5,
            "overlaps reread"
        );
    }
}

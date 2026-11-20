//! User configuration: `$XDG_CONFIG_HOME/d2r-map-overlay/config.toml` (else
//! `~/.config/d2r-map-overlay/config.toml`). Optional; see `config.example.toml`.
//!
//! Loaded once at start. A missing file means the defaults; an unreadable or invalid one is
//! reported once on stderr and the defaults are used, so a typo never stops the overlay.
//!
//! The core's settings only: the shrines drawn and which playstyle runs (`playstyle =
//! "<name>"`). The playstyle's own files (builds, farm list, valuables tiers, damages) live in
//! its folder ([`crate::playstyle::Files`]). Older config files still carry the playstyle's
//! `[valuables]`, `[builds]` and `[bis]` tables: they are accepted here unread and read by the
//! playstyle as a fallback. The playstyle's types stay reachable under their old paths
//! (`crate::config::Build`, `crate::config::Bis`, ...).

use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use serde::de::IgnoredAny;

use crate::game::Shrine;

pub use crate::playstyle::config::{
    Bis, Build, FarmGroup, FarmItem, FarmKind, FarmSlot, Recipe, Stage, Valuables, owns_nothing,
};

/// Shrine names by shrines.txt code (row): `SHRINE_NAMES[code]`.
pub const SHRINE_NAMES: [&str; 23] = [
    "none",
    "refilling",
    "health",
    "mana",
    "health exchange",
    "mana exchange",
    "armor",
    "combat",
    "resist fire",
    "resist cold",
    "resist lightning",
    "resist poison",
    "skill",
    "mana recharge",
    "stamina",
    "experience",
    "enirhs",
    "portal",
    "gem",
    "fire",
    "monster",
    "exploding",
    "poison",
];

/// The map label of each shrine type, by shrines.txt code like [`SHRINE_NAMES`]: three capital
/// letters, one per type. Listed in `config.example.toml`'s `[shrines]` comment.
pub const SHRINE_TAGS: [&str; 23] = [
    "NON", // none
    "REF", // refilling
    "HLT", // health
    "MNA", // mana
    "HEX", // health exchange
    "MEX", // mana exchange
    "ARM", // armor
    "CMB", // combat
    "RFI", // resist fire
    "RCO", // resist cold
    "RLI", // resist lightning
    "RPO", // resist poison
    "SKL", // skill
    "MRC", // mana recharge
    "STA", // stamina
    "EXP", // experience
    "ENI", // enirhs
    "POR", // portal
    "GEM", // gem
    "FIR", // fire
    "MON", // monster
    "XPL", // exploding
    "POI", // poison
];

/// The `BiS` slot names, in display order (amulet, rings, body, helm, hands, feet, belt,
/// weapons; user's order, 2026-11-17).
pub const SLOT_NAMES: [&str; 11] = [
    "amulet", "ring1", "ring2", "body", "helm", "gloves", "boots", "belt", "weapon", "offhand", "swap",
];

/// Class names by class id (charstats row), for `[builds.class]`.
pub const CLASS_NAMES: [&str; 8] = [
    "amazon",
    "sorceress",
    "necromancer",
    "paladin",
    "barbarian",
    "druid",
    "assassin",
    // D2R patch class (user's "Demonmaster" read id 7 live, 2026-11-17).
    "warlock",
];

/// Shrines shown when the config doesn't say.
const DEFAULT_SHRINES: [&str; 5] = ["refilling", "skill", "stamina", "gem", "monster"];
/// Shrine types shown in Normal only by default: stamina is useless later (user, 2026-11-15).
const DEFAULT_NORMAL_ONLY: [&str; 1] = ["stamina"];

/// The playstyle run when the config doesn't say: the one shipped in the repo
/// (`playstyles/summoner-fury/`).
pub const DEFAULT_PLAYSTYLE: &str = "summoner-fury";

/// The whole (core) configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub shrines: Shrines,
    /// The playstyle's folder name (`playstyles/<name>/`, see [`crate::playstyle::Files`]).
    pub playstyle: String,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            shrines: Shrines::default(),
            playstyle: DEFAULT_PLAYSTYLE.to_owned(),
        }
    }
}

/// `[shrines]`: the shrine types to show, as shrines.txt codes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shrines {
    pub show: Vec<u8>,
    /// Of `show`, the types shown in Normal only (e.g. stamina; user request 2026-11-15).
    pub normal_only: Vec<u8>,
}

impl Default for Shrines {
    fn default() -> Self {
        let (show, _) = shrine_codes(&DEFAULT_SHRINES.map(str::to_owned));
        let (normal_only, _) = shrine_codes(&DEFAULT_NORMAL_ONLY.map(str::to_owned));
        Self { show, normal_only }
    }
}

impl Shrines {
    /// Keeps the shrines to draw: unused ones whose type is in `show`, minus the `normal_only`
    /// types outside Normal (`difficulty` 0; an unknown difficulty shows them). Wells are always
    /// drawn.
    pub fn retain_shown(&self, shrines: &mut Vec<Shrine>, difficulty: Option<u8>) {
        let normal = difficulty.is_none_or(|d| d == 0);
        shrines.retain(|s| {
            !s.used && self.show.contains(&s.code) && (normal || !self.normal_only.contains(&s.code))
        });
    }
}

/// The file as written; names are turned into codes by [`parse`].
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawConfig {
    shrines: RawShrines,
    playstyle: Option<String>,
    // The playstyle's tables of older config files: accepted, not read here (the playstyle
    // reads them as its fallback, [`crate::playstyle::Files`]).
    #[serde(rename = "valuables")]
    _valuables: Option<IgnoredAny>,
    #[serde(rename = "builds")]
    _builds: Option<IgnoredAny>,
    #[serde(rename = "bis")]
    _bis: Option<IgnoredAny>,
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawShrines {
    show: Vec<String>,
    normal_only: Vec<String>,
}

impl Default for RawShrines {
    fn default() -> Self {
        Self {
            show: DEFAULT_SHRINES.map(str::to_owned).to_vec(),
            normal_only: DEFAULT_NORMAL_ONLY.map(str::to_owned).to_vec(),
        }
    }
}

/// Codes of the known shrine `names` (in order, without repeats), and the unknown names.
fn shrine_codes(names: &[String]) -> (Vec<u8>, Vec<String>) {
    let mut codes = Vec::new();
    let mut unknown = Vec::new();
    for name in names {
        let wanted = name.trim().to_lowercase();
        match (0u8..).zip(SHRINE_NAMES).find(|&(_, n)| n == wanted) {
            Some((code, _)) if !codes.contains(&code) => codes.push(code),
            Some(_) => {}
            None => unknown.push(name.clone()),
        }
    }
    (codes, unknown)
}

/// The class id of `key`: a number 0..=7 or a class name (any case).
pub fn class_id(key: &str) -> Option<u32> {
    let key = key.trim().to_lowercase();
    let by_name = (0u32..)
        .zip(CLASS_NAMES)
        .find(|&(_, n)| n == key)
        .map(|(id, _)| id);
    by_name.or_else(|| key.parse().ok().filter(|&id: &u32| id < 8))
}

/// Parses a config file's text; also returns warnings about ignored entries (unknown shrine
/// names).
///
/// # Errors
/// Fails on invalid TOML, unknown keys or wrongly typed values.
pub fn parse(text: &str) -> Result<(Config, Vec<String>), toml::de::Error> {
    let raw: RawConfig = toml::from_str(text)?;
    let (show, mut unknown) = shrine_codes(&raw.shrines.show);
    let (normal_only, unknown_normal) = shrine_codes(&raw.shrines.normal_only);
    unknown.extend(unknown_normal);
    let warnings: Vec<String> = unknown
        .into_iter()
        .map(|n| {
            format!(
                "unknown shrine {n:?} ignored (known: {})",
                SHRINE_NAMES.join(", ")
            )
        })
        .collect();
    Ok((
        Config {
            shrines: Shrines { show, normal_only },
            playstyle: raw
                .playstyle
                .map(|name| name.trim().to_owned())
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| DEFAULT_PLAYSTYLE.to_owned()),
        },
        warnings,
    ))
}

/// `$XDG_CONFIG_HOME/d2r-map-overlay/config.toml` (else `~/.config/...`); `None` if neither is set.
#[must_use]
pub fn default_path() -> Option<PathBuf> {
    let absolute = |var| {
        std::env::var_os(var)
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())
    };
    let base = absolute("XDG_CONFIG_HOME").or_else(|| absolute("HOME").map(|h| h.join(".config")))?;
    Some(base.join("d2r-map-overlay").join("config.toml"))
}

/// Loads the config at [`default_path`]; see [`load_from`].
pub fn load() -> Config {
    default_path().map_or_else(Config::default, |p| load_from(&p))
}

/// Loads the config at `path`. Missing: the defaults. Unreadable or invalid: one message on
/// stderr, then the defaults. Ignored entries are reported too.
pub fn load_from(path: &Path) -> Config {
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Config::default(),
        Err(e) => {
            eprintln!("config {}: {e}; using the defaults", path.display());
            return Config::default();
        }
    };
    match parse(&text) {
        Ok((config, warnings)) => {
            for w in warnings {
                eprintln!("config {}: {w}", path.display());
            }
            eprintln!("config: {}", path.display());
            config
        }
        Err(e) => {
            eprintln!("config {}: invalid, using the defaults:\n{e}", path.display());
            Config::default()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::temp_dir;
    use anyhow::Result;

    #[test]
    fn defaults() -> Result<()> {
        let c = Config::default();
        assert_eq!(c.shrines.show, vec![1, 12, 14, 18, 20]);
        assert_eq!(c.playstyle, "summoner-fury");
        assert_eq!(parse("")?, (c.clone(), vec![]), "empty file = defaults");
        assert_eq!(parse("[shrines]\n")?.0, c, "empty section = defaults");
        assert_eq!(parse("playstyle = \" \"\n")?.0, c, "blank name = the default");
        Ok(())
    }

    #[test]
    fn parses_the_example() -> Result<()> {
        let (c, warnings) = parse(include_str!("../config.example.toml"))?;
        assert_eq!(c, Config::default(), "the example spells out the defaults");
        assert_eq!(warnings, Vec::<String>::new());
        Ok(())
    }

    #[test]
    fn parses_values() -> Result<()> {
        let (c, warnings) = parse(
            "playstyle = \"mine\"\n\
             [shrines]\nshow = [\"Experience\", \"health exchange\", \"bogus\", \"skill\", \"skill\"]\n",
        )?;
        assert_eq!(c.playstyle, "mine");
        assert_eq!(c.shrines.show, vec![15, 4, 12], "case-insensitive, deduped");
        assert_eq!(warnings.len(), 1);
        assert!(warnings.iter().all(|w| w.contains("\"bogus\"")), "{warnings:?}");
        let (c, _) = parse("[shrines]\nshow = []\n")?;
        assert!(c.shrines.show.is_empty(), "empty list = no shrines");
        Ok(())
    }

    /// Older config files carry the playstyle's tables: the core accepts them unread (the
    /// playstyle reads them, `playstyle::files`), whatever they hold.
    #[test]
    fn tolerates_the_old_playstyle_tables() -> Result<()> {
        let (c, warnings) = parse(
            "[valuables]\ntrade = [\"S\"]\n\
             [builds]\nVex = [\"fire\"]\n[builds.class]\ndruid = [\"physical\"]\n\
             [bis.Vex]\nbuild = \"b\"\n[bis.Vex.slots]\nhelm = [\"Lore\"]\n",
        )?;
        assert_eq!((c, warnings), (Config::default(), vec![]));
        Ok(())
    }

    #[test]
    fn class_ids() {
        assert_eq!(class_id("Assassin"), Some(6));
        assert_eq!(class_id("0"), Some(0));
        // The D2R patch class.
        assert_eq!(class_id("Warlock"), Some(7));
        assert_eq!(class_id("8"), None);
        assert_eq!(class_id("monk"), None);
    }

    #[test]
    fn shrine_tags_are_three_capitals_one_per_type() {
        assert_eq!(SHRINE_TAGS.len(), SHRINE_NAMES.len());
        for tag in SHRINE_TAGS {
            assert!(
                tag.len() == 3 && tag.bytes().all(|b| b.is_ascii_uppercase()),
                "{tag:?}"
            );
        }
        let mut sorted = SHRINE_TAGS.to_vec();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), SHRINE_TAGS.len(), "tags repeat");
    }

    #[test]
    fn shrine_table_is_in_code_order() {
        assert_eq!(SHRINE_NAMES.len(), 23, "0 none + 22 shrine types");
        let at = |code: usize| SHRINE_NAMES.get(code).copied();
        assert_eq!(
            (at(1), at(12), at(18), at(22)),
            (Some("refilling"), Some("skill"), Some("gem"), Some("poison"))
        );
    }

    #[test]
    fn shrine_filter() {
        let shrine = |code, used| Shrine {
            txt: 236,
            code,
            used,
            x: u32::from(code),
            y: 0,
        };
        let mut shrines = vec![
            shrine(2, false),
            shrine(12, false),
            shrine(12, true),
            shrine(20, false),
        ];
        Shrines::default().retain_shown(&mut shrines, Some(0));
        assert_eq!(
            shrines,
            vec![shrine(12, false), shrine(20, false)],
            "health not shown by default, used skill shrine dropped"
        );
        let mut shrines = vec![shrine(2, false)];
        Shrines {
            show: vec![2],
            normal_only: vec![],
        }
        .retain_shown(&mut shrines, Some(0));
        assert_eq!(shrines.len(), 1, "health shown once configured");
        let stamina = Shrines {
            show: vec![14, 12],
            normal_only: vec![14],
        };
        let mut shrines = vec![shrine(14, false), shrine(12, false)];
        stamina.retain_shown(&mut shrines, Some(1));
        assert_eq!(shrines, vec![shrine(12, false)], "stamina hidden in Nightmare");
        let mut shrines = vec![shrine(14, false)];
        stamina.retain_shown(&mut shrines, Some(0));
        assert_eq!(shrines.len(), 1, "stamina shown in Normal");
        let mut shrines = vec![shrine(14, false)];
        stamina.retain_shown(&mut shrines, None);
        assert_eq!(shrines.len(), 1, "unknown difficulty shows it");
    }

    #[test]
    fn rejects_invalid() {
        for bad in [
            "[shrines\n",
            "[shrines]\nshow = \"skill\"\n",
            "[shrines]\nshwo = [\"skill\"]\n",
            "playstyle = 3\n",
            "[bogus]\n",
        ] {
            assert!(parse(bad).is_err(), "{bad:?} accepted");
        }
    }

    #[test]
    fn load_falls_back_to_defaults() -> Result<()> {
        let dir = temp_dir("config")?;
        std::fs::create_dir_all(&dir)?;
        assert_eq!(load_from(&dir.join("missing.toml")), Config::default());
        let invalid = dir.join("invalid.toml");
        std::fs::write(&invalid, "[shrines]\nshow = \"skill\"\n")?;
        assert_eq!(load_from(&invalid), Config::default());
        let valid = dir.join("valid.toml");
        std::fs::write(&valid, "[shrines]\nshow = [\"skill\"]\n")?;
        assert_eq!(load_from(&valid).shrines.show, vec![12]);
        std::fs::remove_dir_all(&dir)?;
        Ok(())
    }
}

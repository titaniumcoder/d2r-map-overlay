//! User configuration: `$XDG_CONFIG_HOME/d2r-map-overlay/config.toml` (else
//! `~/.config/d2r-map-overlay/config.toml`). Optional; see `config.example.toml`.
//!
//! Loaded once at start. A missing file means the defaults; an unreadable or invalid one is
//! reported once on stderr and the defaults are used, so a typo never stops the overlay.
//!
//! Build definitions live in `builds/<slug>.toml` next to `config.toml`, one build per file,
//! authored by the agent (not edited in the webapp); the config only *selects* one per
//! character via `[bis.<name>] build = "<slug>"` (user decision, 2026-11-17).

use std::collections::BTreeMap;
use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;

use crate::build::Damage;
use crate::game::Shrine;
use crate::valuables::{Category, Tier};

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

/// An agent-authored build definition from `builds/<slug>.toml` (see [`load_builds`]): display
/// name, damage types, optional free-text notes and the per-slot piece lists in the same
/// `"Piece@<required level>"` format as the old inline `[bis]` slots.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Build {
    /// The display name (e.g. "Blood Boil"); the map key is its lowercase form.
    pub name: String,
    /// The build's damage types, primary first.
    pub damages: Vec<Damage>,
    /// Free text (e.g. the guide the build was authored from); `None` when empty.
    pub notes: Option<String>,
    /// Slot name → pieces, best (highest required level) first.
    pub slots: BTreeMap<String, Vec<String>>,
    /// The mercenary's slot name → pieces.
    pub merc_slots: BTreeMap<String, Vec<String>>,
    /// Per-piece details by piece name (`[pieces."<name>"]`): what the dashboard shows under
    /// each piece (kind, base, stats, why). A piece without an entry shows only its name.
    pub pieces: BTreeMap<String, PieceInfo>,
    /// Free-text goals of the build (breakpoints, attributes, mercenary), shown above the grid.
    pub targets: Vec<String>,
    /// Items outside the equipment slots (`[extras]`: charms, jewels, runes …), group name →
    /// pieces in the same `"Name@<required level>"` format.
    pub extras: BTreeMap<String, Vec<String>>,
}

/// `[pieces."<name>"]` of a build file: the details of one piece (all optional).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PieceInfo {
    /// unique, set, runeword, crafted, rare, magic or base (drives the colour).
    pub kind: String,
    /// The guide setup the piece belongs to: starter, standard or alternative.
    pub setup: String,
    /// Base item (a runeword's allowed bases and sockets).
    pub base: String,
    /// The key mods with their ranges, one short line each.
    pub stats: Vec<String>,
    /// Why the build uses it (from the guide).
    pub why: String,
    /// Caveats (unverified data, example rolls).
    pub notes: String,
}

/// The whole configuration.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Config {
    pub valuables: Valuables,
    pub shrines: Shrines,
    pub builds: Builds,
    /// Per-character `BiS` selection and optional legacy inline gear list (`[bis.<name]`).
    pub bis: BTreeMap<String, Bis>,
    /// The agent-authored build definitions from `builds/*.toml`, keyed by lowercased build
    /// name — the `slug` a `[bis.<name>]` `build` selection refers to (see [`load_builds`]).
    pub build_defs: BTreeMap<String, Build>,
}

/// A character's `BiS` selection and (legacy) inline gear list: `[bis.<name>]` (user request,
/// 2026-11-17). Since the build files exist, the webapp writes only `build` here; the inline
/// `slots`/`merc_slots` stay for hand-written configs and win over the selected build's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bis {
    /// The selected build's slug (lowercased build name from `builds/*.toml`, e.g.
    /// "blood boil"); empty when the entry is inline-only or nothing is selected.
    pub build: String,
    /// Slot name → pieces in chase order (`"Name@<required level>"`; the guide shows the
    /// highest-level one the character can currently use, so a slot can graduate from Spirit
    /// to Enigma as the character levels).
    pub slots: BTreeMap<String, Vec<String>>,
    /// The mercenary's slot name → pieces.
    pub merc_slots: BTreeMap<String, Vec<String>>,
}

/// `[valuables]`: the tiers highlighted per category (see [`crate::valuables`]).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Valuables {
    pub trade: Vec<Tier>,
    pub twink: Vec<Tier>,
}

impl Default for Valuables {
    fn default() -> Self {
        Self {
            trade: vec![Tier::S, Tier::A],
            twink: vec![Tier::S, Tier::A],
        }
    }
}

impl Valuables {
    /// Every tier of both categories (what the reader keeps as candidates).
    pub fn everything() -> Self {
        let all = vec![Tier::S, Tier::A, Tier::B];
        Self {
            trade: all.clone(),
            twink: all,
        }
    }

    pub fn enabled(&self, category: Category, tier: Tier) -> bool {
        match category {
            Category::Trade => &self.trade,
            Category::Twink => &self.twink,
        }
        .contains(&tier)
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

/// `[builds]`: each character's damage types, primary first (see [`crate::build`]); by name, else
/// by class (`[builds.class]`). Empty by default: no build, no colouring.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Builds {
    /// Lowercased character name → damage types.
    pub names: BTreeMap<String, Vec<Damage>>,
    /// Class id (0..=6, see [`CLASS_NAMES`]) → damage types.
    pub classes: BTreeMap<u32, Vec<Damage>>,
}

impl Config {
    /// The build definition the character's `[bis]` entry selects, if it names one that exists.
    #[must_use]
    pub fn selected_build(&self, name: &str) -> Option<&Build> {
        let slug = self.bis.get(&name.to_lowercase())?.build.trim().to_lowercase();
        if slug.is_empty() {
            None
        } else {
            self.build_defs.get(&slug)
        }
    }

    /// The character's `BiS` resolved over the build files: its inline slots when it has any,
    /// else the selected build's slots (else none — a build file may omit slots).
    #[must_use]
    pub fn resolved_bis(&self, name: &str) -> Option<Bis> {
        let bis = self.bis.get(&name.to_lowercase())?;
        if !bis.slots.is_empty() || !bis.merc_slots.is_empty() {
            return Some(bis.clone());
        }
        let def = self.selected_build(name)?;
        Some(Bis {
            build: def.name.clone(),
            slots: def.slots.clone(),
            merc_slots: def.merc_slots.clone(),
        })
    }

    /// The character's damage types, primary first: its own `[builds]` entry, else the selected
    /// build's damages, else its class's (`class` as charstats row id; `None` where the caller
    /// doesn't know a class, e.g. the config page).
    #[must_use]
    pub fn damages_for_player(&self, name: &str, class: Option<u32>) -> &[Damage] {
        if let Some(damages) = self.builds.names.get(&name.to_lowercase()) {
            return damages;
        }
        if let Some(def) = self.selected_build(name) {
            return &def.damages;
        }
        class
            .and_then(|c| self.builds.classes.get(&c))
            .map_or(&[], Vec::as_slice)
    }
}

/// The file as written; names are turned into codes by [`parse`].
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawConfig {
    valuables: Valuables,
    shrines: RawShrines,
    builds: RawBuilds,
    bis: BTreeMap<String, RawBis>,
}

/// `[builds]` as written: character name = damage names, plus the `class` table (class id or name
/// = damage names).
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawBuilds {
    class: BTreeMap<String, Vec<String>>,
    /// Every other key: a character name.
    #[serde(flatten)]
    names: BTreeMap<String, Vec<String>>,
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

/// The known damage types among `names` (in order, without repeats); warns about the others.
fn damages(names: &[String], whose: &str, warnings: &mut Vec<String>) -> Vec<Damage> {
    let mut out = Vec::new();
    for name in names {
        match Damage::from_name(name) {
            Some(d) if !out.contains(&d) => out.push(d),
            Some(_) => {}
            None => {
                let known: Vec<&str> = Damage::ALL.iter().map(|d| d.name()).collect();
                warnings.push(format!(
                    "build {whose}: unknown damage {name:?} ignored (known: {})",
                    known.join(", ")
                ));
            }
        }
    }
    out
}

/// The class id of `key`: a number 0..=7 or a class name (any case).
fn class_id(key: &str) -> Option<u32> {
    let key = key.trim().to_lowercase();
    let by_name = (0u32..)
        .zip(CLASS_NAMES)
        .find(|&(_, n)| n == key)
        .map(|(id, _)| id);
    by_name.or_else(|| key.parse().ok().filter(|&id: &u32| id < 8))
}

/// Trims a piece list and drops the blanks (`" "` entries are never meaningful).
fn clean_pieces(pieces: &[String]) -> Vec<String> {
    pieces
        .iter()
        .map(|p| p.trim().to_owned())
        .filter(|p| !p.is_empty())
        .collect()
}

/// Parses a `slots`/`merc_slots` piece table (`"Piece"` or `"Piece@<required level>"` per
/// slot): blanks dropped, empty lists dropped, unknown slot names warned about and ignored.
/// `label` names the entry in the warnings (e.g. `bis Demonmaster`).
fn parse_slot_tables(
    raw: BTreeMap<String, Vec<String>>,
    label: &str,
    warnings: &mut Vec<String>,
) -> BTreeMap<String, Vec<String>> {
    let mut out = BTreeMap::new();
    for (slot, pieces) in raw {
        let cleaned = clean_pieces(&pieces);
        if cleaned.is_empty() {
            continue;
        }
        if SLOT_NAMES.contains(&slot.as_str()) {
            out.insert(slot, cleaned);
        } else {
            warnings.push(format!(
                "{label}: unknown slot {slot:?} ignored (known: {})",
                SLOT_NAMES.join(", ")
            ));
        }
    }
    out
}

/// `[bis.<name>]` as written: the selected build's `slug` and the per-slot pieces
/// (`slots` for the character, `merc_slots` for the mercenary — legacy inline lists the webapp
/// no longer edits; they win over the selected build's slots).
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawBis {
    build: String,
    slots: BTreeMap<String, Vec<String>>,
    merc_slots: BTreeMap<String, Vec<String>>,
}

/// `builds/<slug>.toml` as written (agent-authored; see the module docs).
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawBuild {
    name: String,
    damages: Vec<String>,
    notes: String,
    slots: BTreeMap<String, Vec<String>>,
    merc_slots: BTreeMap<String, Vec<String>>,
    pieces: BTreeMap<String, PieceInfo>,
    targets: Vec<String>,
    extras: BTreeMap<String, Vec<String>>,
}

/// Parses one build file's text: the lowercased build name (the slug a `[bis.<name>]` `build`
/// selection refers to), the [`Build`], and warnings about ignored entries.
/// # Errors
/// Fails on invalid TOML, unknown keys or wrongly typed values.
fn parse_build(text: &str) -> Result<(String, Build, Vec<String>), toml::de::Error> {
    let raw: RawBuild = toml::from_str(text)?;
    let mut warnings = Vec::new();
    let name = raw.name.trim().to_owned();
    let damages = damages(&raw.damages, &name, &mut warnings);
    let label = format!("build {name}");
    let slots = parse_slot_tables(raw.slots, &label, &mut warnings);
    let merc_slots = parse_slot_tables(raw.merc_slots, &label, &mut warnings);
    let notes = raw.notes.trim().to_owned();
    Ok((
        name.to_lowercase(),
        Build {
            name,
            damages,
            notes: (!notes.is_empty()).then_some(notes),
            slots,
            merc_slots,
            pieces: raw.pieces,
            targets: raw
                .targets
                .iter()
                .map(|t| t.trim().to_owned())
                .filter(|t| !t.is_empty())
                .collect(),
            extras: raw
                .extras
                .into_iter()
                .map(|(group, pieces)| (group, clean_pieces(&pieces)))
                .filter(|(_, pieces)| !pieces.is_empty())
                .collect(),
        },
        warnings,
    ))
}

/// Loads every `*.toml` build definition from `dir` (one build per file, agent-authored), keyed
/// by lowercased build name. A missing directory means no builds and no error; an unreadable,
/// invalid or nameless file is reported on stderr and skipped.
pub fn load_builds(dir: &Path) -> BTreeMap<String, Build> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return BTreeMap::new();
    };
    let mut builds = BTreeMap::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_some_and(|e| e != "toml") {
            continue;
        }
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) => {
                eprintln!("build {}: {e}; skipped", path.display());
                continue;
            }
        };
        match parse_build(&text) {
            Ok((slug, build, warnings)) => {
                for w in warnings {
                    eprintln!("build {}: {w}", path.display());
                }
                if slug.is_empty() {
                    eprintln!("build {}: no name; skipped", path.display());
                } else {
                    builds.insert(slug, build);
                }
            }
            Err(e) => eprintln!("build {}: invalid, skipped:\n{e}", path.display()),
        }
    }
    builds
}

/// The `builds/` directory next to the config file at `config_path` (the agent-authored build
/// definitions; see [`load_builds`]).
#[must_use]
pub fn builds_dir_for(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .map_or_else(|| PathBuf::from("builds"), |p| p.join("builds"))
}

fn builds(raw: RawBuilds, warnings: &mut Vec<String>) -> Builds {
    let mut builds = Builds::default();
    for (name, list) in raw.names {
        let list = damages(&list, &name, warnings);
        builds.names.insert(name.to_lowercase(), list);
    }
    for (key, list) in raw.class {
        let list = damages(&list, &format!("class {key}"), warnings);
        match class_id(&key) {
            Some(id) => {
                builds.classes.insert(id, list);
            }
            None => warnings.push(format!(
                "build: unknown class {key:?} ignored (known: 0..7 or {})",
                CLASS_NAMES.join(", ")
            )),
        }
    }
    builds
}

/// Parses a config file's text; also returns warnings about ignored entries (unknown shrine names,
/// damage types and classes).
///
/// # Errors
/// Fails on invalid TOML, unknown keys or wrongly typed values (e.g. a tier other than S, A, B).
pub fn parse(text: &str) -> Result<(Config, Vec<String>), toml::de::Error> {
    let raw: RawConfig = toml::from_str(text)?;
    let (show, mut unknown) = shrine_codes(&raw.shrines.show);
    let (normal_only, unknown_normal) = shrine_codes(&raw.shrines.normal_only);
    unknown.extend(unknown_normal);
    let mut warnings: Vec<String> = unknown
        .into_iter()
        .map(|n| {
            format!(
                "unknown shrine {n:?} ignored (known: {})",
                SHRINE_NAMES.join(", ")
            )
        })
        .collect();
    let builds = builds(raw.builds, &mut warnings);
    let mut bis = BTreeMap::new();
    for (name, raw) in raw.bis {
        let label = format!("bis {name}");
        let slots = parse_slot_tables(raw.slots, &label, &mut warnings);
        let merc_slots = parse_slot_tables(raw.merc_slots, &label, &mut warnings);
        // Selection-only entries (`build` without any slots) are the webapp's normal output;
        // an entry with neither a build nor slots does nothing and is ignored with a warning.
        let build = raw.build.trim().to_owned();
        if build.is_empty() && slots.is_empty() && merc_slots.is_empty() {
            warnings.push(format!(
                "bis {name}: needs a build selection or at least one slot piece; ignored"
            ));
            continue;
        }
        bis.insert(
            name.to_lowercase(),
            Bis {
                build,
                slots,
                merc_slots,
            },
        );
    }
    Ok((
        Config {
            valuables: raw.valuables,
            shrines: Shrines { show, normal_only },
            builds,
            bis,
            build_defs: BTreeMap::new(),
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
        // The build files stand on their own (see the invalid branch below): a missing or
        // unreadable config.toml must not lose the builds dir either.
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            return Config {
                build_defs: load_builds(&builds_dir_for(path)),
                ..Config::default()
            };
        }
        Err(e) => {
            eprintln!("config {}: {e}; using the defaults", path.display());
            return Config {
                build_defs: load_builds(&builds_dir_for(path)),
                ..Config::default()
            };
        }
    };
    match parse(&text) {
        Ok((mut config, warnings)) => {
            for w in warnings {
                eprintln!("config {}: {w}", path.display());
            }
            eprintln!("config: {}", path.display());
            config.build_defs = load_builds(&builds_dir_for(path));
            config
        }
        Err(e) => {
            eprintln!("config {}: invalid, using the defaults:\n{e}", path.display());
            // The build files stand on their own: a broken config.toml must not lose them.
            Config {
                build_defs: load_builds(&builds_dir_for(path)),
                ..Config::default()
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::temp_dir;
    use anyhow::{Result, anyhow};

    #[test]
    fn defaults() -> Result<()> {
        let c = Config::default();
        assert_eq!(c.valuables.trade, vec![Tier::S, Tier::A]);
        assert_eq!(c.valuables.twink, vec![Tier::S, Tier::A]);
        assert_eq!(c.shrines.show, vec![1, 12, 14, 18, 20]);
        assert_eq!(parse("")?, (c.clone(), vec![]), "empty file = defaults");
        assert_eq!(parse("[valuables]\n")?.0, c, "empty section = defaults");
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
            "[valuables]\ntrade = [\"S\"]\ntwink = []\n\n\
             [shrines]\nshow = [\"Experience\", \"health exchange\", \"bogus\", \"skill\", \"skill\"]\n",
        )?;
        assert_eq!(c.valuables.trade, vec![Tier::S]);
        assert_eq!(c.valuables.twink, vec![]);
        assert!(!c.valuables.enabled(Category::Trade, Tier::A));
        assert!(c.valuables.enabled(Category::Trade, Tier::S));
        assert_eq!(c.shrines.show, vec![15, 4, 12], "case-insensitive, deduped");
        assert_eq!(warnings.len(), 1);
        assert!(warnings.iter().all(|w| w.contains("\"bogus\"")), "{warnings:?}");
        let (c, _) = parse("[valuables]\ntwink = [\"B\"]\n[shrines]\nshow = []\n")?;
        assert_eq!(c.valuables.trade, vec![Tier::S, Tier::A], "missing key = default");
        assert_eq!(c.valuables.twink, vec![Tier::B]);
        assert!(c.shrines.show.is_empty(), "empty list = no shrines");
        Ok(())
    }

    #[test]
    fn parses_builds() -> Result<()> {
        use Damage::{Cold, Fire, Lightning, Physical};
        let (c, warnings) = parse(
            "[builds]\nVex = [\"lightning\", \"Fire\", \"physical\", \"fire\"]\nOther = [\"holy\", \"cold\"]\n\n\
             [builds.class]\ndruid = [\"physical\"]\n4 = [\"physical\"]\nbard = [\"fire\"]\n",
        )?;
        assert_eq!(
            c.damages_for_player("Vex", Some(6)),
            [Lightning, Fire, Physical],
            "deduped, in order"
        );
        assert_eq!(
            c.damages_for_player("vex", Some(1)),
            [Lightning, Fire, Physical],
            "name in any case"
        );
        assert_eq!(
            c.damages_for_player("Other", Some(5)),
            [Cold],
            "own entry beats the class"
        );
        assert_eq!(c.damages_for_player("Bear", Some(5)), [Physical], "class by name");
        assert_eq!(c.damages_for_player("Barb", Some(4)), [Physical], "class by id");
        assert_eq!(
            c.damages_for_player("Sorc", Some(1)),
            [] as [Damage; 0],
            "nothing configured"
        );
        assert_eq!(
            c.damages_for_player("Sorc", None),
            [] as [Damage; 0],
            "no class known: still no damages"
        );
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(warnings.iter().any(|w| w.contains("\"holy\"")), "{warnings:?}");
        assert!(warnings.iter().any(|w| w.contains("\"bard\"")), "{warnings:?}");
        assert!(
            parse("[builds]\nVex = \"lightning\"\n").is_err(),
            "a list is required"
        );
        Ok(())
    }

    #[test]
    fn bis_entries() -> Result<()> {
        let (config, warnings) = parse(
            "[bis.Demonmaster]\nbuild = \"blood boil\"\n\
             [bis.Demonmaster.slots]\nweapon = [\"Heart of the Oak@55\"]\nbody = [\"Stealth@17\", \"Enigma@65\"]\nring1 = [\" \"]\n\
             unknown = [\"x\"]\n\
             [bis.Demonmaster.merc_slots]\nweapon = [\"Insight@27\"]",
        )?;
        // Blank pieces are dropped silently; unknown slots (in both tables) warn.
        assert_eq!(
            warnings,
            vec![
                "bis Demonmaster: unknown slot \"unknown\" ignored (known: amulet, ring1, ring2, body, helm, gloves, boots, belt, weapon, offhand, swap)".to_owned(),
            ],
            "{warnings:?}"
        );
        let Some(bis) = config.bis.get("demonmaster") else {
            return Err(anyhow!("no bis entry"));
        };
        assert_eq!(bis.build, "blood boil");
        assert_eq!(
            bis.slots.get("weapon").map(Vec::as_slice),
            Some(&["Heart of the Oak@55".to_owned()][..])
        );
        assert!(!bis.slots.contains_key("ring1"), "blank pieces are dropped");
        assert_eq!(
            bis.merc_slots.get("weapon").map(Vec::as_slice),
            Some(&["Insight@27".to_owned()][..])
        );
        assert!(!config.bis.contains_key("other"));
        // A selection without any slots is the webapp's normal output: valid.
        let (config, warnings) = parse("[bis.X]\nbuild = \"b\"\n")?;
        assert_eq!(config.bis.get("x").map(|b| b.build.as_str()), Some("b"));
        assert_eq!(warnings, Vec::<String>::new());
        // Neither a build nor slots: ignored with a warning.
        let (_, warnings) = parse("[bis.X]\nbuild = \"\"\n[bis.X.slots]\nhelm = [\" \"]")?;
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        Ok(())
    }

    #[test]
    fn build_piece_details_targets_and_extras() -> Result<()> {
        let text = "name = \"B\"\ntargets = [\"75% FCR\", \" \"]\n\
             [slots]\nhelm = [\"Harlequin Crest@62\"]\n\
             [extras]\ncharms = [\"Annihilus@70\", \"\"]\njewels = []\n\
             [pieces.\"Harlequin Crest\"]\nkind = \"unique\"\nsetup = \"alternative\"\nbase = \"Shako\"\n\
             stats = [\"+2 to All Skills\"]\nwhy = \"skills\"\n";
        let (_, def, warnings) = parse_build(text)?;
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(def.targets, ["75% FCR"], "blank targets dropped");
        assert_eq!(
            def.extras.keys().collect::<Vec<_>>(),
            ["charms"],
            "empty groups dropped"
        );
        assert_eq!(def.extras.get("charms").map(Vec::len), Some(1));
        let shako = def
            .pieces
            .get("Harlequin Crest")
            .ok_or_else(|| anyhow!("no piece"))?;
        assert_eq!((shako.kind.as_str(), shako.base.as_str()), ("unique", "Shako"));
        assert_eq!(shako.stats, ["+2 to All Skills"]);
        // Unknown keys in a piece are an error, like everywhere else in the build file.
        assert!(parse_build("name = \"B\"\n[pieces.X]\ncolour = \"red\"\n").is_err());
        Ok(())
    }

    #[test]
    fn build_files() -> Result<()> {
        let dir = temp_dir("builds")?;
        std::fs::create_dir_all(&dir)?;
        std::fs::write(
            dir.join("blood-boil.toml"),
            "# comment preserved on the way in\nname = \"Blood Boil\"\ndamages = [\"physical\", \"fire\"]\nnotes = \"guide\"\n\
             [slots]\nweapon = [\"Heart of the Oak@55\", \" \"]\n\
             [merc_slots]\nweapon = [\"Insight@27\"]",
        )?;
        std::fs::write(dir.join("broken.toml"), "name = [\n")?;
        std::fs::write(dir.join("nameless.toml"), "damages = [\"fire\"]\n")?;
        std::fs::write(dir.join("ignored.txt"), "name = \"Nope\"\n")?;
        let builds = load_builds(&dir);
        assert_eq!(builds.keys().collect::<Vec<_>>(), ["blood boil"], "{builds:?}");
        let Some(def) = builds.get("blood boil") else {
            return Err(anyhow!("no blood boil"));
        };
        assert_eq!(def.name, "Blood Boil");
        assert_eq!(def.damages, [Damage::Physical, Damage::Fire]);
        assert_eq!(def.notes.as_deref(), Some("guide"));
        assert_eq!(
            def.slots.get("weapon").map(Vec::as_slice),
            Some(&["Heart of the Oak@55".to_owned()][..]),
            "blank pieces dropped"
        );
        assert_eq!(
            def.merc_slots.get("weapon").map(Vec::as_slice),
            Some(&["Insight@27".to_owned()][..])
        );
        // A missing directory is no builds and no error.
        assert!(load_builds(&dir.join("missing")).is_empty());
        std::fs::remove_dir_all(&dir)?;
        Ok(())
    }

    #[test]
    fn resolution_inline_beats_build_beats_none() -> Result<()> {
        let mut config = Config::default();
        config.build_defs.insert(
            "blood boil".to_owned(),
            Build {
                name: "Blood Boil".to_owned(),
                damages: vec![Damage::Physical, Damage::Fire],
                notes: None,
                slots: [("weapon".to_owned(), vec!["Heart of the Oak@55".to_owned()])].into(),
                ..Build::default()
            },
        );
        config.bis.insert(
            "demonmaster".to_owned(),
            Bis {
                build: "blood boil".to_owned(),
                slots: BTreeMap::new(),
                merc_slots: BTreeMap::new(),
            },
        );
        // Selection-only: the build's slots and damages come through.
        let resolved = config
            .resolved_bis("DEMONmaster")
            .ok_or_else(|| anyhow!("unresolved"))?;
        assert_eq!(resolved.build, "Blood Boil", "the display name, not the slug");
        assert_eq!(
            resolved.slots.get("weapon").map(Vec::as_slice),
            Some(&["Heart of the Oak@55".to_owned()][..])
        );
        assert_eq!(
            config.damages_for_player("DEMONmaster", None),
            [Damage::Physical, Damage::Fire],
            "the build's damages"
        );
        // Inline slots win over the build's, and a character-level `[builds]` entry beats the
        // build's damages.
        let inline = [("body".to_owned(), vec!["Stealth@17".to_owned()])].into();
        if let Some(b) = config.bis.get_mut("demonmaster") {
            b.slots = inline;
        }
        config
            .builds
            .names
            .insert("demonmaster".to_owned(), vec![Damage::Fire]);
        let resolved = config
            .resolved_bis("demonmaster")
            .ok_or_else(|| anyhow!("unresolved"))?;
        assert!(
            !resolved.slots.contains_key("weapon"),
            "inline slots replace the build's"
        );
        assert_eq!(
            resolved.slots.get("body").map(Vec::as_slice),
            Some(&["Stealth@17".to_owned()][..])
        );
        assert_eq!(config.damages_for_player("demonmaster", Some(7)), [Damage::Fire]);
        // No bis entry at all: no resolved BiS, only the class fallback for damages.
        assert!(config.resolved_bis("other").is_none());
        config.builds.classes.insert(7, vec![Damage::Lightning]);
        assert_eq!(config.damages_for_player("other", Some(7)), [Damage::Lightning]);
        // A selected build that doesn't exist resolves to nothing.
        config.bis.insert(
            "ghost".to_owned(),
            Bis {
                build: "nope".to_owned(),
                slots: BTreeMap::new(),
                merc_slots: BTreeMap::new(),
            },
        );
        assert!(config.resolved_bis("ghost").is_none());
        assert_eq!(config.damages_for_player("ghost", None), [] as [Damage; 0]);
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
            "[valuables\n",
            "[valuables]\ntrade = [\"X\"]\n",
            "[valuables]\ntrade = \"S\"\n",
            "[valuables]\ntrdae = [\"S\"]\n",
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
        std::fs::write(&invalid, "[valuables]\ntrade = [\"X\"]\n")?;
        assert_eq!(load_from(&invalid), Config::default());
        let valid = dir.join("valid.toml");
        std::fs::write(&valid, "[valuables]\ntrade = [\"S\"]\n")?;
        assert_eq!(load_from(&valid).valuables.trade, vec![Tier::S]);
        std::fs::remove_dir_all(&dir)?;
        Ok(())
    }
}

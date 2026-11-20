//! The playstyle's settings ([`Settings`]): the agent-authored build files and farm list, the
//! valuables tiers, the damage defaults and each character's build selection, and the stage
//! logic over them. Where each comes from (the playstyle folder, the user's overrides, the old
//! places in the config dir) is [`crate::files`]; the core re-exports the types here under
//! their old paths (`crate::config::Build`, ...).
//!
//! Build definitions live in `builds/<slug>.toml`, one build per file, authored by the agent
//! (not edited in the webapp); a character only *selects* one via `[bis.<name>] build =
//! "<slug>"` (user decision, 2026-11-17), written by the `/config` page into `bis.toml`.
//!
//! The cross-character farm list lives in `farm.toml`, agent-authored too ([`parse_farm`]);
//! it is hot-reloaded with the build files.

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::valuables::{Category, Tier};
use d2r_core::build::Damage;
use d2r_core::config::{CLASS_NAMES, SLOT_NAMES, class_id};
// The skill plan format is the core skills panel's; the build files carry it.
pub use d2r_core::web::SkillTarget;
// The `BiS` selection is a core type: the guidance carries it to the map and the dashboard.
pub use d2r_core::guidance::Bis;

/// The playstyle's settings, as loaded from its files ([`crate::files::load`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    /// `[valuables]`: the drop mark tiers.
    pub valuables: Valuables,
    /// `[damages]` (`playstyle.toml`; `[builds]` in an old `config.toml`): damage types per
    /// character and class.
    pub builds: Builds,
    /// Per-character `BiS` selection and optional legacy inline gear list (`[bis.<name]`).
    pub bis: BTreeMap<String, Bis>,
    /// The agent-authored build definitions from `builds/*.toml`, keyed by lowercased build
    /// name — the `slug` a `[bis.<name>]` `build` selection refers to (see [`parse_build`]).
    pub build_defs: BTreeMap<String, Build>,
    /// The cross-character farm list from `farm.toml` (see [`parse_farm`]); empty when the
    /// file is missing or invalid.
    pub farm: Vec<FarmGroup>,
    /// The leveling and farming route from `route.toml` ([`crate::route`]); empty
    /// when the file is missing or invalid.
    pub route: crate::route::Route,
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
    #[must_use]
    pub fn everything() -> Self {
        let all = vec![Tier::S, Tier::A, Tier::B];
        Self {
            trade: all.clone(),
            twink: all,
        }
    }

    #[must_use]
    pub fn enabled(&self, category: Category, tier: Tier) -> bool {
        match category {
            Category::Trade => &self.trade,
            Category::Twink => &self.twink,
        }
        .contains(&tier)
    }
}

/// `[damages]`: each character's damage types, primary first (see [`d2r_core::build`]); by name,
/// else by class (`[damages.class]`). Empty by default: the builds' damages count.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Builds {
    /// Lowercased character name → damage types.
    pub names: BTreeMap<String, Vec<Damage>>,
    /// Class id (0..=7, see [`CLASS_NAMES`]) → damage types.
    pub classes: BTreeMap<u32, Vec<Damage>>,
}

/// The playstyle tables one TOML file sets; `None` where the file has no such table.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tables {
    pub valuables: Option<Valuables>,
    /// `[damages]` (`playstyle.toml`) or `[builds]` (an old `config.toml`).
    pub damages: Option<Builds>,
    pub bis: Option<BTreeMap<String, Bis>>,
}

#[cfg(test)]
impl Tables {
    /// The settings these tables give on their own (defaults for the absent ones; no builds,
    /// no farm list).
    #[must_use]
    pub fn into_settings(self) -> Settings {
        Settings {
            valuables: self.valuables.unwrap_or_default(),
            builds: self.damages.unwrap_or_default(),
            bis: self.bis.unwrap_or_default(),
            ..Settings::default()
        }
    }
}

/// `config.toml` (old places) or `bis.toml` as written: only the playstyle's tables are read;
/// the core's (`[shrines]`, `playstyle`) are [`d2r_core::config`]'s.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawTables {
    valuables: Option<Valuables>,
    builds: Option<RawBuilds>,
    bis: Option<BTreeMap<String, RawBis>>,
}

/// `playstyle.toml` as written.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawPlaystyle {
    name: String,
    /// For people reading the folder; shown nowhere yet.
    #[serde(default, rename = "description")]
    _description: String,
    valuables: Option<Valuables>,
    damages: Option<RawBuilds>,
}

/// `[damages]` / `[builds]` as written: character name = damage names, plus the `class` table
/// (class id or name = damage names).
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawBuilds {
    class: BTreeMap<String, Vec<String>>,
    /// Every other key: a character name.
    #[serde(flatten)]
    names: BTreeMap<String, Vec<String>>,
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

/// The known damage types among `names` (in order, without repeats); warns about the others.
pub fn damages(names: &[String], whose: &str, warnings: &mut Vec<String>) -> Vec<Damage> {
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

/// Trims a piece list and drops the blanks (`" "` entries are never meaningful).
#[must_use]
pub fn clean_pieces(pieces: &[String]) -> Vec<String> {
    pieces
        .iter()
        .map(|p| p.trim().to_owned())
        .filter(|p| !p.is_empty())
        .collect()
}

/// Parses a `slots`/`merc_slots` piece table (`"Piece"` or `"Piece@<required level>"` per
/// slot): blanks dropped, empty lists dropped, unknown slot names warned about and ignored.
/// `label` names the entry in the warnings (e.g. `bis Demonmaster`).
pub fn parse_slot_tables(
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

fn bis_entries(raw: BTreeMap<String, RawBis>, warnings: &mut Vec<String>) -> BTreeMap<String, Bis> {
    let mut bis = BTreeMap::new();
    for (name, raw) in raw {
        let label = format!("bis {name}");
        let slots = parse_slot_tables(raw.slots, &label, warnings);
        let merc_slots = parse_slot_tables(raw.merc_slots, &label, warnings);
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
    bis
}

/// Parses the playstyle tables of a `config.toml` (old places: `[valuables]`, `[builds]`,
/// `[bis]`; other tables are the core's and skipped) or of `bis.toml`; also returns warnings
/// about ignored entries (unknown damage types, classes and slots, empty `[bis]` entries).
///
/// # Errors
/// Fails on invalid TOML, unknown keys in these tables or wrongly typed values (e.g. a tier
/// other than S, A, B).
pub fn parse_tables(text: &str) -> Result<(Tables, Vec<String>), toml::de::Error> {
    let raw: RawTables = toml::from_str(text)?;
    let mut warnings = Vec::new();
    let tables = Tables {
        valuables: raw.valuables,
        damages: raw.builds.map(|raw| builds(raw, &mut warnings)),
        bis: raw.bis.map(|raw| bis_entries(raw, &mut warnings)),
    };
    Ok((tables, warnings))
}

/// Parses `playstyle.toml`: its display name, `[valuables]` and `[damages]` (its
/// `description` is for people reading the folder), with warnings as [`parse_tables`].
///
/// # Errors
/// Fails on invalid TOML, a missing `name`, unknown keys or wrongly typed values.
pub fn parse_playstyle(text: &str) -> Result<((String, Tables), Vec<String>), toml::de::Error> {
    let raw: RawPlaystyle = toml::from_str(text)?;
    let mut warnings = Vec::new();
    let tables = Tables {
        valuables: raw.valuables,
        damages: raw.damages.map(|raw| builds(raw, &mut warnings)),
        bis: None,
    };
    Ok(((raw.name, tables), warnings))
}

/// An agent-authored build definition from `builds/<slug>.toml` (see [`crate::files`]): display
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
    /// The class the build is for (`class = "druid"`, class id as in [`CLASS_NAMES`]).
    pub class: Option<u32>,
    /// A character of [`Build::class`] without a `[bis.<name>]` selection gets this build
    /// (user request, 2026-11-17: a new character works before it is named in the config).
    pub default_for_class: bool,
    /// The slug of the guide's next stage (`next = "druid tornado"`), see [`Settings::stage`].
    pub next: Option<String>,
    /// The character level at which the next stage starts.
    pub switch_level: Option<u8>,
    /// Whether the next stage starts on its own at [`Build::switch_level`]; when false the
    /// dashboard only says when to switch on `/config`.
    pub auto_switch: bool,
    /// The guide's advice for the switch (respec, prerequisites).
    pub switch_note: Option<String>,
    /// What the automatic switch waits for besides the level: every group met (see
    /// [`SwitchGroup`]; `switch_requires = ["Harmony", { label = "Cannot Be Frozen", any =
    /// ["Raven Frost", "Nosferatu's Coil"] }]`). Empty: the level alone decides.
    pub switch_requires: Vec<SwitchGroup>,
    /// Cumulative hard-point targets, in the guide's order (`[[skill_plan]]`).
    pub skill_plan: Vec<SkillTarget>,
    /// The recipes the guide uses (`[[recipes]]`: runewords, crafts, cube recipes …).
    pub recipes: Vec<Recipe>,
    /// Leveling tips by character-level range (`[[tips]]`).
    pub tips: Vec<Tip>,
    /// How to spend stat points, one line per level range.
    pub stats_plan: Vec<String>,
}

/// One condition of a stage switch ([`Build::switch_requires`]): met when the account holds
/// `count` copies of the items in `any` together (unique/set display names or runeword names;
/// copies as counted by [`d2r_core::stash::Holdings::count`]: shared stash, the character, its
/// mercenary's gear). A plain string in the TOML is a group of one item, one copy, labelled
/// with its name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitchGroup {
    /// What the group stands for ("Cannot Be Frozen"), shown on the dashboard.
    pub label: String,
    /// The alternatives, at least one.
    pub any: Vec<String>,
    /// The copies needed, summed over `any` (at least 1).
    pub count: u32,
}

impl SwitchGroup {
    /// A group of one copy of the single item `name`, labelled with it.
    #[must_use]
    pub fn item(name: &str) -> Self {
        Self {
            label: name.to_owned(),
            any: vec![name.to_owned()],
            count: 1,
        }
    }

    /// The copies held of the group's items, `copies` giving each item's.
    #[must_use]
    pub fn have(&self, copies: &dyn Fn(&str) -> u32) -> u32 {
        self.any
            .iter()
            .fold(0, |sum: u32, item| sum.saturating_add(copies(item)))
    }

    /// Whether [`SwitchGroup::have`] reaches `count`.
    #[must_use]
    pub fn met(&self, copies: &dyn Fn(&str) -> u32) -> bool {
        self.have(copies) >= self.count
    }
}

/// One `[[recipes]]` entry: what the guide crafts and from what (all text, shown as a card).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Recipe {
    pub name: String,
    /// One of [`RECIPE_KINDS`].
    pub kind: String,
    /// One input per line: a rune (`"Tal"`, `"3 Tal Rune"`), a gem (`"1 Chipped Topaz"`) or
    /// a free-text base item (`"non-magic Body Armor with 2 sockets"`).
    pub inputs: Vec<String>,
    pub result: String,
    /// Its requirement as text (`"character level 17 (highest rune)"`).
    pub req: String,
    pub notes: String,
    /// The quest whose reward this recipe is (`"Tools of the Trade"` for Charsi's imbue,
    /// `"Siege on Harrogath"` for Larzuk's socket; a name from [`d2r_core::quests::QUESTS`]),
    /// empty for none. The dashboard drops the recipe once the reward is used.
    pub quest: String,
}

/// The recipe kinds the dashboard groups by.
pub const RECIPE_KINDS: [&str; 5] = ["runeword", "craft", "cube", "socket", "upgrade"];

/// One `[[tips]]` entry: a leveling tip shown while the character level is in `from..=to`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tip {
    pub from: u8,
    pub to: u8,
    pub text: String,
}

/// `[[tips]]` as written: `levels = "1-13"` (or `"75"`, `"75+"`).
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawTip {
    levels: String,
    text: String,
}

/// The `from..=to` character levels of a tip's `levels` text: `"1-13"`, `"75"` or `"75+"`
/// (to level 99). `None` when unreadable or reversed.
fn tip_levels(levels: &str) -> Option<(u8, u8)> {
    let levels = levels.trim();
    let (from, to) = if let Some(from) = levels.strip_suffix('+') {
        (from.trim().parse().ok()?, 99)
    } else if let Some((from, to)) = levels.split_once('-') {
        (from.trim().parse().ok()?, to.trim().parse().ok()?)
    } else {
        let level = levels.parse().ok()?;
        (level, level)
    };
    (from <= to).then_some((from, to))
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

/// One `[[farm]]` group of `farm.toml` (agent-authored, the app only reads it): the gear a
/// planned build (`for`) needs, farmed with other characters.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FarmGroup {
    /// The planned build's name (`for = "Fury Druid"`; `for` is a Rust keyword).
    #[serde(rename = "for")]
    pub target: String,
    pub items: Vec<FarmItem>,
}

/// One farm-list entry: an item by display name (unique/set) or runeword name.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FarmItem {
    pub name: String,
    pub kind: FarmKind,
    /// How many copies the build needs (default 1; two Lawbringers for two weapon slots).
    #[serde(default = "one")]
    pub count: u32,
    /// `merc = true`: the item is for the mercenary (taken as Act 2, as everywhere else), so
    /// a runeword's base hint skips class-only bases, keeps polearms/spears for weapons and
    /// ranks helm and body bases by defense.
    #[serde(default)]
    pub merc: bool,
    /// `slot = "helm"`: the body slot it is worn in, narrowing a runeword's base hint to bases
    /// of that slot (Fortitude as `"body"` lists armor, not weapons). Left out: any slot.
    #[serde(default)]
    pub slot: Option<FarmSlot>,
}

/// The body slot of a [`FarmItem`], as written in `farm.toml`. An unknown name fails the
/// file with serde's own message (unknown variant `hat`, expected one of ...), with its line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FarmSlot {
    Helm,
    Amulet,
    Body,
    Weapon,
    /// Also accepted as `"offhand"`, the build files' name for it.
    #[serde(alias = "offhand")]
    Shield,
    Ring,
    Belt,
    Boots,
    Gloves,
}

impl FarmSlot {
    /// The `ItemData::BODY` slot (the numbering of `bases_data::Base::slot`).
    #[must_use]
    pub const fn body(self) -> u8 {
        match self {
            Self::Helm => 1,
            Self::Amulet => 2,
            Self::Body => 3,
            Self::Weapon => 4,
            Self::Shield => 5,
            Self::Ring => 6,
            Self::Belt => 8,
            Self::Boots => 9,
            Self::Gloves => 10,
        }
    }
}

/// What kind of item a [`FarmItem`] names: the core [`d2r_core::names::ItemKind`] (which table
/// its name comes from and how its copies are counted).
pub type FarmKind = d2r_core::names::ItemKind;

/// serde's `default = "..."` needs a function path.
const fn one() -> u32 {
    1
}

/// `farm.toml` as written.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawFarm {
    farm: Vec<FarmGroup>,
}

/// Parses `farm.toml`'s text, with warnings about names not in the game tables (kept: a typo
/// shows as forever missing, the warning says why) and a `count` of 0 (read as 1).
/// # Errors
/// Fails on invalid TOML, unknown keys, an unknown `kind` or wrongly typed values.
pub fn parse_farm(text: &str) -> Result<(Vec<FarmGroup>, Vec<String>), toml::de::Error> {
    let mut groups = toml::from_str::<RawFarm>(text)?.farm;
    let mut warnings = Vec::new();
    for group in &mut groups {
        for item in &mut group.items {
            if item.count == 0 {
                warnings.push(format!(
                    "{}: {:?} has count 0; read as 1",
                    group.target, item.name
                ));
                item.count = 1;
            }
            let same = |n: &str| d2r_core::names::same_name(n, &item.name);
            // Uniques and sets by their in-game name: the table's, or its display override
            // (`DISPLAY_NAMES` covers both kinds; this is only a warning).
            let shown = || {
                d2r_core::item_names_data::DISPLAY_NAMES
                    .iter()
                    .any(|(_, n)| same(n))
            };
            let known = match item.kind {
                FarmKind::Runeword => d2r_core::runewords_data::lookup(&item.name).is_some(),
                FarmKind::Unique => {
                    d2r_core::item_names_data::UNIQUE_NAMES
                        .iter()
                        .any(|(_, n)| same(n))
                        || shown()
                }
                FarmKind::Set => d2r_core::item_names_data::SET_NAMES.iter().any(|(_, n)| same(n)) || shown(),
            };
            if !known {
                warnings.push(format!(
                    "{}: unknown {} {:?} (kept, but it can never be found)",
                    group.target,
                    item.kind.name(),
                    item.name
                ));
            }
        }
    }
    Ok((groups, warnings))
}

/// Whether `name` is a unique, set (table name or display override) or runeword name, for
/// load-time warnings about item names that can never match.
fn item_name_known(name: &str) -> bool {
    let same = |n: &str| d2r_core::names::same_name(n, name);
    d2r_core::runewords_data::lookup(name).is_some()
        || d2r_core::item_names_data::UNIQUE_NAMES
            .iter()
            .any(|(_, n)| same(n))
        || d2r_core::item_names_data::SET_NAMES.iter().any(|(_, n)| same(n))
        || d2r_core::item_names_data::DISPLAY_NAMES
            .iter()
            .any(|(_, n)| same(n))
}

/// The copy count of callers without live data (the config page, `level` unknown): nothing
/// is owned, so a `switch_requires` is never met.
#[must_use]
pub const fn owns_nothing(_item: &str) -> u32 {
    0
}

/// Where a character is in its build guide's stage chain ([`Settings::stage`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Stage<'a> {
    /// The current stage's slug and build.
    pub slug: &'a str,
    pub build: &'a Build,
    /// Whether the chain started at the class default (no `[bis.<name>]` selection).
    pub class_default: bool,
    /// The next stage, when the current one names an existing build that isn't already part
    /// of the chain (a cycle shows no next stage).
    pub next: Option<(&'a str, &'a Build)>,
}

impl Settings {
    /// The build definition (with its slug) the character's `[bis]` entry selects, if it names
    /// one that exists.
    fn selected(&self, name: &str) -> Option<(&str, &Build)> {
        let slug = self.bis.get(&name.to_lowercase())?.build.trim().to_lowercase();
        self.build_defs
            .get_key_value(&slug)
            .map(|(slug, build)| (slug.as_str(), build))
    }

    /// The default build of class id `class` (`default_for_class = true`); the first by slug
    /// when several claim it.
    #[must_use]
    pub fn class_default(&self, class: u32) -> Option<(&str, &Build)> {
        self.build_defs
            .iter()
            .find(|(_, b)| b.default_for_class && b.class == Some(class))
            .map(|(slug, b)| (slug.as_str(), b))
    }

    /// The character's current guide stage: the selected build (else its class's default),
    /// followed along `next` while the stage switches automatically (`auto_switch`), the
    /// character `level` has reached `switch_level` and every `switch_requires` group is met
    /// (`owns` gives an item's copies held, see [`SwitchGroup`]). The groups count as met, too, for a switch into a stage the
    /// character has `reached` before or into one leading there ([`Settings::leads_to`]): the
    /// shared stash is unread until opened each game, and a switch is one-way. A `next` that
    /// is already part of the chain stops it (cycle guard). `class`/`level` are `None` where
    /// unknown (the config page, which passes [`owns_nothing`] and no `reached`): no class
    /// default, no automatic switch.
    #[must_use]
    pub fn stage(
        &self,
        name: &str,
        class: Option<u32>,
        level: Option<u8>,
        owns: &dyn Fn(&str) -> u32,
        reached: Option<&str>,
    ) -> Option<Stage<'_>> {
        let (start, class_default) = match self.selected(name) {
            Some(selected) => (selected, false),
            None => (class.and_then(|c| self.class_default(c))?, true),
        };
        let (mut slug, mut build) = start;
        let mut chain = vec![slug];
        let next_of = |build: &Build| {
            let next = build.next.as_deref()?;
            self.build_defs
                .get_key_value(next)
                .map(|(slug, build)| (slug.as_str(), build))
        };
        while let Some((next_slug, next_build)) = next_of(build) {
            let due = build.auto_switch
                && build
                    .switch_level
                    .zip(level)
                    .is_some_and(|(switch, level)| level >= switch)
                && (build.switch_requires.iter().all(|group| group.met(owns))
                    || reached.is_some_and(|reached| self.leads_to(next_slug, reached)));
            if !due || chain.contains(&next_slug) {
                break;
            }
            chain.push(next_slug);
            (slug, build) = (next_slug, next_build);
        }
        Some(Stage {
            slug,
            build,
            class_default,
            next: next_of(build).filter(|(next, _)| !chain.contains(next)),
        })
    }

    /// The names of build `from` (a slug) and of every stage after it along the builds' `next`
    /// links, in order (cycles stop the walk; an unknown slug gives none): the builds one
    /// character counts as for the farm list (it keeps its gear through a respec).
    #[must_use]
    pub fn stage_chain(&self, from: &str) -> Vec<String> {
        let mut seen: Vec<&str> = Vec::new();
        let mut names = Vec::new();
        let mut slug = from;
        while let Some(build) = self.build_defs.get(slug) {
            if seen.contains(&slug) {
                break;
            }
            seen.push(slug);
            names.push(build.name.clone());
            match build.next.as_deref() {
                Some(next) => slug = next,
                None => break,
            }
        }
        names
    }

    /// Whether stage `to` is `from` or comes after it along the builds' `next` links (cycles
    /// stop the walk).
    #[must_use]
    pub fn leads_to(&self, from: &str, to: &str) -> bool {
        let mut seen = Vec::new();
        let mut slug = from;
        loop {
            if slug == to {
                return true;
            }
            if seen.contains(&slug) {
                return false;
            }
            seen.push(slug);
            match self.build_defs.get(slug).and_then(|b| b.next.as_deref()) {
                Some(next) => slug = next,
                None => return false,
            }
        }
    }

    /// The character's `BiS` resolved over the build files: its inline slots when it has any,
    /// else the current stage's slots ([`Settings::stage`], `owns`/`reached` as there; else
    /// none — a build file may omit slots).
    #[must_use]
    pub fn resolved_bis(
        &self,
        name: &str,
        class: Option<u32>,
        level: Option<u8>,
        owns: &dyn Fn(&str) -> u32,
        reached: Option<&str>,
    ) -> Option<Bis> {
        if let Some(bis) = self.bis.get(&name.to_lowercase())
            && (!bis.slots.is_empty() || !bis.merc_slots.is_empty())
        {
            return Some(bis.clone());
        }
        let def = self.stage(name, class, level, owns, reached)?.build;
        Some(Bis {
            build: def.name.clone(),
            slots: def.slots.clone(),
            merc_slots: def.merc_slots.clone(),
        })
    }

    /// The character's damage types, primary first: its own `[builds]` entry, else the current
    /// stage's damages ([`Settings::stage`], `owns`/`reached` as there), else its class's
    /// (`class` as charstats row id; `None` where the caller doesn't know a class, e.g. the
    /// config page).
    #[must_use]
    pub fn damages_for_player(
        &self,
        name: &str,
        class: Option<u32>,
        level: Option<u8>,
        owns: &dyn Fn(&str) -> u32,
        reached: Option<&str>,
    ) -> &[Damage] {
        if let Some(damages) = self.builds.names.get(&name.to_lowercase()) {
            return damages;
        }
        if let Some(stage) = self.stage(name, class, level, owns, reached)
            && !stage.build.damages.is_empty()
        {
            return &stage.build.damages;
        }
        class
            .and_then(|c| self.builds.classes.get(&c))
            .map_or(&[], Vec::as_slice)
    }
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
    class: String,
    default_for_class: bool,
    next: String,
    switch_level: Option<u8>,
    auto_switch: bool,
    switch_note: String,
    switch_requires: Vec<RawSwitchGroup>,
    skill_plan: Vec<SkillTarget>,
    recipes: Vec<Recipe>,
    tips: Vec<RawTip>,
    stats_plan: Vec<String>,
}

/// A `switch_requires` entry as written: an item name, or `{ label, any = [...], count }`.
/// Deserialized by hand rather than `#[serde(untagged)]`: untagged hides why a table failed
/// ("did not match any variant"), while handing the table to [`RawSwitchAny`] keeps serde's
/// own message (unknown field, missing field `any`).
#[derive(Debug)]
enum RawSwitchGroup {
    Item(String),
    Group(RawSwitchAny),
}

impl<'de> Deserialize<'de> for RawSwitchGroup {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        /// Picks the form by what the TOML holds: a string or a table.
        struct Form;
        impl<'de> serde::de::Visitor<'de> for Form {
            type Value = RawSwitchGroup;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("an item name or a table { label, any = [...], count }")
            }
            fn visit_str<E: serde::de::Error>(self, name: &str) -> Result<Self::Value, E> {
                Ok(RawSwitchGroup::Item(name.to_owned()))
            }
            fn visit_map<A: serde::de::MapAccess<'de>>(self, map: A) -> Result<Self::Value, A::Error> {
                RawSwitchAny::deserialize(serde::de::value::MapAccessDeserializer::new(map))
                    .map(RawSwitchGroup::Group)
            }
        }
        deserializer.deserialize_any(Form)
    }
}

/// The table form of a [`RawSwitchGroup`].
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSwitchAny {
    #[serde(default)]
    label: String,
    any: Vec<String>,
    /// Copies needed; 1 when left out.
    count: Option<u32>,
}

/// The [`SwitchGroup`]s of a build's `switch_requires`: names trimmed, a blank item string
/// dropped, a group without items warned about and dropped (it could never be met), a table
/// without a label labelled with its items, `count = 0` warned about and taken as 1; unknown
/// item names warned about and kept.
fn switch_groups(raw: Vec<RawSwitchGroup>, label: &str, warnings: &mut Vec<String>) -> Vec<SwitchGroup> {
    let mut groups = Vec::new();
    for entry in raw {
        let group = match entry {
            RawSwitchGroup::Item(name) => {
                let name = name.trim();
                if name.is_empty() {
                    continue;
                }
                SwitchGroup::item(name)
            }
            RawSwitchGroup::Group(table) => {
                let any = clean_pieces(&table.any);
                let mut group_label = table.label.trim().to_owned();
                if any.is_empty() {
                    warnings.push(format!(
                        "{label}: switch_requires group {group_label:?} lists no items (any = []); ignored"
                    ));
                    continue;
                }
                if group_label.is_empty() {
                    group_label = any.join(" / ");
                }
                let count = match table.count {
                    Some(0) => {
                        warnings.push(format!(
                            "{label}: switch_requires group {group_label:?} has count = 0; taken as 1"
                        ));
                        1
                    }
                    count => count.unwrap_or(1),
                };
                SwitchGroup {
                    label: group_label,
                    any,
                    count,
                }
            }
        };
        for item in &group.any {
            if !item_name_known(item) {
                warnings.push(format!(
                    "{label}: switch_requires names unknown item {item:?} (kept, but it can never be owned)"
                ));
            }
        }
        groups.push(group);
    }
    groups
}

/// Parses one build file's text: the lowercased build name (the slug a `[bis.<name>]` `build`
/// selection refers to), the [`Build`], and warnings about ignored entries.
/// # Errors
/// Fails on invalid TOML, unknown keys or wrongly typed values.
pub fn parse_build(text: &str) -> Result<(String, Build, Vec<String>), toml::de::Error> {
    let raw: RawBuild = toml::from_str(text)?;
    let mut warnings = Vec::new();
    let name = raw.name.trim().to_owned();
    let damages = damages(&raw.damages, &name, &mut warnings);
    let label = format!("build {name}");
    let slots = parse_slot_tables(raw.slots, &label, &mut warnings);
    let merc_slots = parse_slot_tables(raw.merc_slots, &label, &mut warnings);
    let notes = raw.notes.trim().to_owned();
    let class = class_of_build(&raw.class, &label, &mut warnings);
    check_skill_names(&raw.skill_plan, class, &label, &mut warnings);
    for recipe in &raw.recipes {
        if !RECIPE_KINDS.contains(&recipe.kind.as_str()) {
            warnings.push(format!(
                "{label}: recipe {:?} has unknown kind {:?} (known: {})",
                recipe.name,
                recipe.kind,
                RECIPE_KINDS.join(", ")
            ));
        }
        if !recipe.quest.is_empty() && d2r_core::quests::word_of(&recipe.quest).is_none() {
            warnings.push(format!(
                "{label}: recipe {:?} names unknown quest {:?} (shown regardless of quest state)",
                recipe.name, recipe.quest
            ));
        }
    }
    let mut tips = Vec::new();
    for tip in raw.tips {
        match tip_levels(&tip.levels) {
            Some((from, to)) => tips.push(Tip {
                from,
                to,
                text: tip.text.trim().to_owned(),
            }),
            None => warnings.push(format!(
                "{label}: tip levels {:?} unreadable (want \"1-13\", \"75\" or \"75+\"); ignored",
                tip.levels
            )),
        }
    }
    let next = raw.next.trim().to_lowercase();
    let switch_note = raw.switch_note.trim().to_owned();
    let switch_requires = switch_groups(raw.switch_requires, &label, &mut warnings);
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
            class,
            default_for_class: raw.default_for_class,
            next: (!next.is_empty()).then_some(next),
            switch_level: raw.switch_level,
            auto_switch: raw.auto_switch,
            switch_note: (!switch_note.is_empty()).then_some(switch_note),
            switch_requires,
            skill_plan: raw.skill_plan,
            recipes: raw.recipes,
            tips,
            stats_plan: clean_pieces(&raw.stats_plan),
        },
        warnings,
    ))
}

/// The class id of a build file's `class` text (empty: none; unknown: warned and none).
fn class_of_build(class: &str, label: &str, warnings: &mut Vec<String>) -> Option<u32> {
    if class.trim().is_empty() {
        return None;
    }
    let id = class_id(class);
    if id.is_none() {
        warnings.push(format!(
            "{label}: unknown class {class:?} ignored (known: {})",
            CLASS_NAMES.join(", ")
        ));
    }
    id
}

/// Warns about skill-plan skills that aren't skills of `class` (any class when unknown), so a
/// typo doesn't silently drop the guide's target from the skills panel.
fn check_skill_names(plan: &[SkillTarget], class: Option<u32>, label: &str, warnings: &mut Vec<String>) {
    for target in plan {
        let known = d2r_core::skills_data::SKILLS.iter().any(|s| {
            s.name.eq_ignore_ascii_case(target.skill.trim()) && class.is_none_or(|c| u32::from(s.class) == c)
        });
        if !known
            && !warnings
                .iter()
                .any(|w| w.contains(&format!("{:?}", target.skill)))
        {
            warnings.push(format!(
                "{label}: skill plan names unknown skill {:?}",
                target.skill
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::{Result, anyhow};

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
    fn resolution_inline_beats_build_beats_none() -> Result<()> {
        let mut config = Settings::default();
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
            .resolved_bis("DEMONmaster", None, None, &owns_nothing, None)
            .ok_or_else(|| anyhow!("unresolved"))?;
        assert_eq!(resolved.build, "Blood Boil", "the display name, not the slug");
        assert_eq!(
            resolved.slots.get("weapon").map(Vec::as_slice),
            Some(&["Heart of the Oak@55".to_owned()][..])
        );
        assert_eq!(
            config.damages_for_player("DEMONmaster", None, None, &owns_nothing, None),
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
            .resolved_bis("demonmaster", None, None, &owns_nothing, None)
            .ok_or_else(|| anyhow!("unresolved"))?;
        assert!(
            !resolved.slots.contains_key("weapon"),
            "inline slots replace the build's"
        );
        assert_eq!(
            resolved.slots.get("body").map(Vec::as_slice),
            Some(&["Stealth@17".to_owned()][..])
        );
        assert_eq!(
            config.damages_for_player("demonmaster", Some(7), None, &owns_nothing, None),
            [Damage::Fire]
        );
        // No bis entry at all: no resolved BiS, only the class fallback for damages.
        assert!(
            config
                .resolved_bis("other", None, None, &owns_nothing, None)
                .is_none()
        );
        config.builds.classes.insert(7, vec![Damage::Lightning]);
        assert_eq!(
            config.damages_for_player("other", Some(7), None, &owns_nothing, None),
            [Damage::Lightning]
        );
        // A selected build that doesn't exist resolves to nothing.
        config.bis.insert(
            "ghost".to_owned(),
            Bis {
                build: "nope".to_owned(),
                slots: BTreeMap::new(),
                merc_slots: BTreeMap::new(),
            },
        );
        assert!(
            config
                .resolved_bis("ghost", None, None, &owns_nothing, None)
                .is_none()
        );
        assert_eq!(
            config.damages_for_player("ghost", None, None, &owns_nothing, None),
            [] as [Damage; 0]
        );
        Ok(())
    }

    #[test]
    fn build_guide_keys_parse() -> Result<()> {
        let text = "name = \"Druid Leveling\"\nclass = \"Druid\"\ndefault_for_class = true\n\
             next = \"Druid Tornado\"\nswitch_level = 38\nauto_switch = true\nswitch_note = \" respec \"\n\
             stats_plan = [\"2-11: all Vitality\", \" \"]\n\
             skill_plan = [{ skill = \"Firestorm\", points = 1, by_level = 2, note = \"start\" }, { skill = \"Fissure\", points = 2, by_level = 13 }]\n\
             [[recipes]]\nname = \"Stealth\"\nkind = \"runeword\"\ninputs = [\"Tal\", \"Eth\", \"2os armor\"]\nresult = \"Stealth\"\nreq = \"level 17\"\n\
             [[recipes]]\nname = \"Odd\"\nkind = \"alchemy\"\n\
             [[recipes]]\nname = \"Imbue\"\nkind = \"upgrade\"\nquest = \"tools of the trade\"\n\
             [[recipes]]\nname = \"Socket\"\nkind = \"socket\"\nquest = \"Larzuk\"\n\
             [[tips]]\nlevels = \"1-13\"\ntext = \"Den of Evil\"\n\
             [[tips]]\nlevels = \"75+\"\ntext = \"late\"\n\
             [[tips]]\nlevels = \"soon\"\ntext = \"bad\"\n";
        let (slug, def, warnings) = parse_build(text)?;
        assert_eq!(slug, "druid leveling");
        assert_eq!(def.class, Some(5));
        assert!(def.default_for_class && def.auto_switch);
        assert_eq!(def.next.as_deref(), Some("druid tornado"), "the slug, lowercased");
        assert_eq!(def.switch_level, Some(38));
        assert_eq!(def.switch_note.as_deref(), Some("respec"));
        assert_eq!(def.stats_plan, ["2-11: all Vitality"]);
        assert_eq!(def.skill_plan.len(), 2);
        assert_eq!(
            def.skill_plan
                .first()
                .map(|t| (t.skill.as_str(), t.points, t.by_level, t.note.as_str())),
            Some(("Firestorm", 1, 2, "start"))
        );
        assert_eq!(def.recipes.first().map(|r| r.inputs.len()), Some(3));
        assert_eq!(
            def.tips,
            [
                Tip {
                    from: 1,
                    to: 13,
                    text: "Den of Evil".to_owned()
                },
                Tip {
                    from: 75,
                    to: 99,
                    text: "late".to_owned()
                },
            ]
        );
        assert_eq!(
            def.recipes.get(2).map(|r| r.quest.as_str()),
            Some("tools of the trade")
        );
        // The unknown recipe kind, the unknown quest and the unreadable tip levels are
        // warned about.
        assert_eq!(warnings.len(), 3, "{warnings:?}");
        assert!(
            warnings.iter().any(|w| w.contains("unknown quest \"Larzuk\"")),
            "{warnings:?}"
        );
        // Unknown skills of the build's class, unknown classes and unknown keys.
        let (_, _, warnings) = parse_build(
            "name = \"B\"\nclass = \"druid\"\nskill_plan = [{ skill = \"Blizzard\", points = 1, by_level = 2 }]\n",
        )?;
        assert!(
            warnings.iter().any(|w| w.contains("\"Blizzard\"")),
            "{warnings:?}"
        );
        let (_, def, warnings) = parse_build("name = \"B\"\nclass = \"monk\"\n")?;
        assert_eq!((def.class, warnings.len()), (None, 1));
        assert!(parse_build("name = \"B\"\n[[recipes]]\nname = \"x\"\ncolour = \"red\"\n").is_err());
        assert!(parse_build("name = \"B\"\nskill_plan = [{ skill = \"Fissure\", points = 1 }]\n").is_err());
        assert_eq!(tip_levels("38"), Some((38, 38)));
        assert_eq!(tip_levels("13-1"), None);
        Ok(())
    }

    /// A druid chain: leveling →(auto at 38) tornado →(manual) fissure.
    fn druid_chain() -> Settings {
        let build = |name: &str, next: Option<&str>, auto: bool| Build {
            name: name.to_owned(),
            class: Some(5),
            default_for_class: name == "Leveling",
            next: next.map(str::to_owned),
            switch_level: next.map(|_| 38),
            auto_switch: auto,
            switch_note: Some("note".to_owned()),
            damages: vec![Damage::Fire],
            ..Build::default()
        };
        let mut config = Settings::default();
        config
            .build_defs
            .insert("leveling".to_owned(), build("Leveling", Some("tornado"), true));
        config
            .build_defs
            .insert("tornado".to_owned(), build("Tornado", Some("fissure"), false));
        config
            .build_defs
            .insert("fissure".to_owned(), build("Fissure", None, false));
        config
    }

    #[test]
    fn stage_follows_the_auto_chain_by_level() -> Result<()> {
        let config = druid_chain();
        let stage = |level| config.stage("NewDruid", Some(5), level, &owns_nothing, None);
        // No selection: the class default; below the switch level it stays.
        let s = stage(Some(37)).ok_or_else(|| anyhow!("no stage"))?;
        assert_eq!((s.slug, s.class_default), ("leveling", true));
        assert_eq!(s.next.map(|(slug, _)| slug), Some("tornado"));
        // At 38 the auto switch moves on; tornado's next is manual, so it stops there.
        let s = stage(Some(38)).ok_or_else(|| anyhow!("no stage"))?;
        assert_eq!(s.slug, "tornado");
        assert_eq!(
            s.next.map(|(slug, b)| (slug, b.name.as_str())),
            Some(("fissure", "Fissure"))
        );
        assert_eq!(
            stage(Some(99)).map(|s| s.slug),
            Some("tornado"),
            "manual never switches"
        );
        assert_eq!(
            stage(None).map(|s| s.slug),
            Some("leveling"),
            "unknown level: no switch"
        );
        // Other classes and an unknown class get no default.
        assert!(
            config
                .stage("NewDruid", Some(1), Some(50), &owns_nothing, None)
                .is_none()
        );
        assert!(
            config
                .stage("NewDruid", None, Some(50), &owns_nothing, None)
                .is_none()
        );
        // The stage drives the BiS and the damages.
        assert_eq!(
            config
                .resolved_bis("x", Some(5), Some(40), &owns_nothing, None)
                .map(|b| b.build),
            Some("Tornado".to_owned())
        );
        assert_eq!(
            config.damages_for_player("x", Some(5), Some(40), &owns_nothing, None),
            [Damage::Fire]
        );
        Ok(())
    }

    #[test]
    fn switch_requires_parses_and_warns_about_unknown_items() -> Result<()> {
        let (_, def, warnings) = parse_build(
            "name = \"Summoner\"\nnext = \"fury\"\nswitch_level = 75\nauto_switch = true\n\
             switch_requires = [\" Raven Frost \", \"\", \"Enigma\", \"Ravenfrsot\"]\n",
        )?;
        assert_eq!(
            def.switch_requires,
            ["Raven Frost", "Enigma", "Ravenfrsot"].map(SwitchGroup::item),
            "trimmed, blanks dropped"
        );
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings.iter().all(|w| w.contains("Ravenfrsot")), "{warnings:?}");
        let (_, def, _) = parse_build("name = \"B\"\n")?;
        assert_eq!(def.switch_requires, []);
        Ok(())
    }

    #[test]
    fn switch_requires_mixes_names_and_any_of_groups() -> Result<()> {
        let (_, def, warnings) = parse_build(
            "name = \"Summoner\"\nswitch_requires = [\n\
             \"Harmony\",\n\
             { label = \" Cannot Be Frozen \", any = [\"Raven Frost\", \" Nosferatu's Coil \", \"\"] },\n\
             { any = [\"Ribcracker\", \"Grief\"] },\n\
             { label = \"Nothing\", any = [\"\"] },\n\
             { label = \"Typo\", any = [\"Ravenfrsot\"] },\n\
             { label = \"Physical immunes\", any = [\"Lawbringer\"], count = 2 },\n\
             { label = \"Zero\", any = [\"Smoke\"], count = 0 },\n]\n",
        )?;
        let group = |label: &str, any: &[&str], count| SwitchGroup {
            label: label.to_owned(),
            any: any.iter().map(|&a| a.to_owned()).collect(),
            count,
        };
        assert_eq!(
            def.switch_requires,
            [
                SwitchGroup::item("Harmony"),
                group("Cannot Be Frozen", &["Raven Frost", "Nosferatu's Coil"], 1),
                group("Ribcracker / Grief", &["Ribcracker", "Grief"], 1),
                group("Typo", &["Ravenfrsot"], 1),
                group("Physical immunes", &["Lawbringer"], 2),
                group("Zero", &["Smoke"], 1),
            ],
            "trimmed, blanks dropped, an unlabelled group named by its items, an empty one \
             dropped, count 1 by default and at least 1"
        );
        assert_eq!(warnings.len(), 3, "{warnings:?}");
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("\"Zero\"") && w.contains("count = 0")),
            "{warnings:?}"
        );
        assert!(
            warnings
                .iter()
                .any(|w| w.contains("\"Nothing\"") && w.contains("no items")),
            "{warnings:?}"
        );
        assert!(warnings.iter().any(|w| w.contains("Ravenfrsot")), "{warnings:?}");
        // A table with an unknown key or without `any` is an error naming the problem, not
        // silently a group.
        let error = |requires: &str| {
            parse_build(&format!("name = \"B\"\nswitch_requires = [{requires}]\n"))
                .err()
                .map_or_else(String::new, |e| e.to_string())
        };
        let unknown = error("{ label = \"X\", anyy = [\"Grief\"] }");
        assert!(unknown.contains("unknown field `anyy`"), "{unknown}");
        let missing = error("{ label = \"X\" }");
        assert!(missing.contains("missing field `any`"), "{missing}");
        let number = error("42");
        assert!(number.contains("an item name or a table"), "{number}");
        Ok(())
    }

    /// The copies of `item` in the name `lists` (a stand-in for [`d2r_core::stash::Holdings::count`]).
    fn copies_in(item: &str, lists: &[&[String]]) -> u32 {
        let n = lists
            .iter()
            .flat_map(|list| list.iter())
            .filter(|n| d2r_core::names::same_name(n, item))
            .count();
        u32::try_from(n).unwrap_or(u32::MAX)
    }

    #[test]
    fn stage_switch_counts_copies_over_a_group() -> Result<()> {
        let mut config = druid_chain();
        if let Some(b) = config.build_defs.get_mut("leveling") {
            b.switch_requires = vec![
                SwitchGroup {
                    label: "Physical immunes".to_owned(),
                    any: vec!["Lawbringer".to_owned()],
                    count: 2,
                },
                SwitchGroup::item("Smoke"),
            ];
        }
        let names = |n: &[&str]| n.iter().map(|&n| n.to_owned()).collect::<Vec<_>>();
        let slug = |held: &[&str]| {
            let held = names(held);
            let owns = |item: &str| copies_in(item, &[&held]);
            config
                .stage("NewDruid", Some(5), Some(38), &owns, None)
                .map(|s| s.slug)
                .ok_or_else(|| anyhow!("no stage"))
        };
        assert_eq!(slug(&["Lawbringer", "Smoke"])?, "leveling", "one copy of two");
        assert_eq!(slug(&["Lawbringer", "Lawbringer"])?, "leveling", "no Smoke");
        assert_eq!(slug(&["Lawbringer", "Smoke", "lawbringer"])?, "tornado");
        let group = SwitchGroup {
            label: "Two of either".to_owned(),
            any: vec!["Grief".to_owned(), "Ribcracker".to_owned()],
            count: 2,
        };
        let held = names(&["Grief", "Ribcracker"]);
        let copies = |item: &str| copies_in(item, &[&held]);
        assert_eq!(group.have(&copies), 2, "copies sum over the group's items");
        assert!(group.met(&copies));
        Ok(())
    }

    #[test]
    fn stage_switch_needs_every_group_and_any_item_of_each() -> Result<()> {
        let mut config = druid_chain();
        if let Some(b) = config.build_defs.get_mut("leveling") {
            b.switch_requires = vec![
                SwitchGroup {
                    label: "Cannot Be Frozen".to_owned(),
                    any: vec!["Raven Frost".to_owned(), "Nosferatu's Coil".to_owned()],
                    count: 1,
                },
                SwitchGroup {
                    label: "Weapon".to_owned(),
                    any: vec!["Ribcracker".to_owned(), "Grief".to_owned()],
                    count: 1,
                },
                SwitchGroup::item("Harmony"),
            ];
        }
        let names = |n: &[&str]| n.iter().map(|&n| n.to_owned()).collect::<Vec<_>>();
        let slug = |owned: &[&str], stash: &[&str]| {
            let (owned, stash) = (names(owned), names(stash));
            let owns = |item: &str| copies_in(item, &[&owned, &stash]);
            config
                .stage("NewDruid", Some(5), Some(38), &owns, None)
                .map(|s| s.slug)
                .ok_or_else(|| anyhow!("no stage"))
        };
        assert_eq!(slug(&[], &[])?, "leveling", "nothing owned");
        assert_eq!(
            slug(&["Raven Frost", "Grief"], &[])?,
            "leveling",
            "two of three groups"
        );
        assert_eq!(
            slug(&["Raven Frost", "Nosferatu's Coil", "Harmony"], &[])?,
            "leveling",
            "two items of one group don't make up for another"
        );
        assert_eq!(
            slug(&["Nosferatu's Coil", "Ribcracker", "Harmony"], &[])?,
            "tornado",
            "one item of each group"
        );
        assert_eq!(
            slug(&["Harmony"], &["raven frost", "Grief"])?,
            "tornado",
            "carried and shared stash combine"
        );
        assert_eq!(
            config
                .stage("NewDruid", Some(5), Some(38), &owns_nothing, Some("tornado"))
                .map(|s| s.slug),
            Some("tornado"),
            "a reached stage counts every group as met"
        );
        Ok(())
    }

    #[test]
    fn stage_switch_waits_for_required_items() -> Result<()> {
        let mut config = druid_chain();
        if let Some(b) = config.build_defs.get_mut("leveling") {
            b.switch_requires = vec![SwitchGroup::item("Raven Frost")];
        }
        let slug = |level, owned: &[String], stash: &[String]| {
            let owns = |item: &str| copies_in(item, &[owned, stash]);
            config
                .stage("NewDruid", Some(5), Some(level), &owns, None)
                .map(|s| s.slug)
                .ok_or_else(|| anyhow!("no stage"))
        };
        let raven = ["Raven Frost".to_owned()];
        assert_eq!(slug(37, &raven, &[])?, "leveling", "below the level, item or not");
        assert_eq!(slug(38, &[], &[])?, "leveling", "at the level without the item");
        assert_eq!(
            slug(38, &["Enigma".to_owned()], &[])?,
            "leveling",
            "another item doesn't count"
        );
        assert_eq!(
            slug(38, &raven, &[])?,
            "tornado",
            "carried, worn or personally stashed"
        );
        assert_eq!(
            slug(38, &[], &["ravenfrost".to_owned()])?,
            "leveling",
            "whole names only"
        );
        assert_eq!(
            slug(38, &[], &["raven frost".to_owned()])?,
            "tornado",
            "in the shared stash"
        );
        // Without live data (the config page) a requirement is never met.
        assert_eq!(
            config
                .stage("NewDruid", Some(5), Some(90), &owns_nothing, None)
                .map(|s| s.slug),
            Some("leveling")
        );
        // The BiS and damages follow the same stage.
        let owns = |item: &str| copies_in(item, &[&raven]);
        assert_eq!(
            config
                .resolved_bis("x", Some(5), Some(40), &owns, None)
                .map(|b| b.build),
            Some("Tornado".to_owned())
        );
        Ok(())
    }

    #[test]
    fn a_reached_stage_latches_its_required_items() -> Result<()> {
        let mut config = druid_chain();
        for (slug, auto) in [("leveling", true), ("tornado", true)] {
            if let Some(b) = config.build_defs.get_mut(slug) {
                b.auto_switch = auto;
                b.switch_requires = vec![SwitchGroup::item("Raven Frost")];
            }
        }
        let slug = |level, reached| {
            config
                .stage("NewDruid", Some(5), Some(level), &owns_nothing, reached)
                .map(|s| s.slug)
                .ok_or_else(|| anyhow!("no stage"))
        };
        assert_eq!(slug(38, None)?, "leveling", "items missing, nothing reached");
        assert_eq!(
            slug(38, Some("tornado"))?,
            "tornado",
            "reached: the items count as owned"
        );
        // A later stage reached latches every switch on the way there.
        assert_eq!(slug(38, Some("fissure"))?, "fissure");
        // The level is still checked.
        assert_eq!(slug(37, Some("fissure"))?, "leveling");
        // A remembered stage off the chain latches nothing.
        assert_eq!(slug(38, Some("other"))?, "leveling");
        assert!(config.leads_to("leveling", "fissure") && config.leads_to("tornado", "tornado"));
        assert!(!config.leads_to("fissure", "leveling"), "only forwards");
        // A cycle ends the walk.
        if let Some(b) = config.build_defs.get_mut("fissure") {
            b.next = Some("leveling".to_owned());
        }
        assert!(!config.leads_to("leveling", "missing"));
        Ok(())
    }

    #[test]
    fn stage_selection_beats_the_default_and_cycles_stop() -> Result<()> {
        let mut config = druid_chain();
        // A selection (the user switched on /config) starts there, not at the default.
        config.bis.insert(
            "bear".to_owned(),
            Bis {
                build: "Fissure".to_owned(),
                slots: BTreeMap::new(),
                merc_slots: BTreeMap::new(),
            },
        );
        let s = config
            .stage("Bear", Some(5), Some(90), &owns_nothing, None)
            .ok_or_else(|| anyhow!("no stage"))?;
        assert_eq!((s.slug, s.class_default, s.next), ("fissure", false, None));
        // A cycle (fissure → leveling, all automatic) stops before repeating a stage and
        // shows no next stage.
        for slug in ["tornado", "fissure"] {
            if let Some(b) = config.build_defs.get_mut(slug) {
                b.auto_switch = true;
                b.switch_level = Some(1);
            }
        }
        if let Some(b) = config.build_defs.get_mut("fissure") {
            b.next = Some("leveling".to_owned());
        }
        let s = config
            .stage("Other", Some(5), Some(60), &owns_nothing, None)
            .ok_or_else(|| anyhow!("no stage"))?;
        assert_eq!(
            s.slug, "fissure",
            "leveling → tornado → fissure, then the cycle stops"
        );
        assert_eq!(s.next, None, "the cycle's next is already in the chain");
        // A next that names no build: no next stage.
        if let Some(b) = config.build_defs.get_mut("fissure") {
            b.next = Some("missing".to_owned());
        }
        assert_eq!(
            config
                .stage("Other", Some(5), Some(60), &owns_nothing, None)
                .and_then(|s| s.next),
            None
        );
        Ok(())
    }

    #[test]
    fn stage_chain_lists_the_build_and_its_later_stages() {
        let mut config = druid_chain();
        assert_eq!(config.stage_chain("leveling"), ["Leveling", "Tornado", "Fissure"]);
        assert_eq!(config.stage_chain("tornado"), ["Tornado", "Fissure"]);
        assert_eq!(config.stage_chain("missing"), Vec::<String>::new());
        // A cycle stops before repeating a stage.
        if let Some(b) = config.build_defs.get_mut("fissure") {
            b.next = Some("leveling".to_owned());
        }
        assert_eq!(config.stage_chain("tornado"), ["Tornado", "Fissure", "Leveling"]);
    }

    #[test]
    fn farm_list_parses() -> Result<()> {
        let (groups, warnings) = parse_farm(
            "[[farm]]\nfor = \"Fury Druid\"\nitems = [\n\
             { name = \"Fortitude\", kind = \"runeword\" },\n\
             { name = \"Raven Frost\", kind = \"unique\" },\n\
             { name = \"Lawbringer\", kind = \"runeword\", count = 2 },\n\
             { name = \"Tal Rasha's Guardianship\", kind = \"set\" },\n\
             { name = \"Nonesuch\", kind = \"unique\", count = 0 },\n]\n\
             [[farm]]\nfor = \"Other\"\nitems = []\n",
        )?;
        assert_eq!(groups.len(), 2);
        let fury = groups.first().ok_or_else(|| anyhow!("no group"))?;
        assert_eq!(fury.target, "Fury Druid");
        let items: Vec<(&str, FarmKind, u32)> = fury
            .items
            .iter()
            .map(|i| (i.name.as_str(), i.kind, i.count))
            .collect();
        assert_eq!(
            items,
            [
                ("Fortitude", FarmKind::Runeword, 1),
                ("Raven Frost", FarmKind::Unique, 1),
                ("Lawbringer", FarmKind::Runeword, 2),
                ("Tal Rasha's Guardianship", FarmKind::Set, 1),
                ("Nonesuch", FarmKind::Unique, 1),
            ],
            "count defaults to 1, 0 reads as 1"
        );
        // The unknown name and the zero count are warned about, the entry kept.
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(warnings.iter().all(|w| w.contains("Nonesuch")), "{warnings:?}");
        assert_eq!(parse_farm("")?.0, vec![], "empty file: no groups");
        for bad in [
            "[[farm]]\nfor = \"X\"\nitems = [{ name = \"Smoke\", kind = \"rune\" }]\n",
            "[[farm]]\nfor = \"X\"\nitems = [{ name = \"Smoke\" }]\n",
            "[[farm]]\nitems = []\n",
            "[[farm]]\nfor = \"X\"\nitems = []\ncolour = 1\n",
            "[[farm]\n",
        ] {
            assert!(parse_farm(bad).is_err(), "{bad:?} accepted");
        }
        Ok(())
    }

    #[test]
    fn farm_entries_take_merc_and_slot() -> Result<()> {
        let (groups, warnings) = parse_farm(
            "[[farm]]\nfor = \"Fury Druid\"\nitems = [\n\
             { name = \"Bulwark\", kind = \"runeword\", merc = true, slot = \"helm\" },\n\
             { name = \"Ancients' Pledge\", kind = \"runeword\", slot = \"offhand\" },\n\
             { name = \"Fortitude\", kind = \"runeword\" },\n]\n",
        )?;
        assert_eq!(warnings, Vec::<String>::new());
        let items: Vec<(bool, Option<FarmSlot>)> = groups
            .first()
            .ok_or_else(|| anyhow!("no group"))?
            .items
            .iter()
            .map(|i| (i.merc, i.slot))
            .collect();
        assert_eq!(
            items,
            [
                (true, Some(FarmSlot::Helm)),
                (false, Some(FarmSlot::Shield)),
                (false, None)
            ],
            "offhand is the shield slot; both fields optional"
        );
        assert_eq!(FarmSlot::Body.body(), 3);
        // A bad slot name fails the file with serde's message: what was written, what is
        // allowed and where.
        let err = parse_farm(
            "[[farm]]\nfor = \"X\"\nitems = [\n{ name = \"Smoke\", kind = \"runeword\", slot = \"chest\" },\n]\n",
        )
        .err()
        .ok_or_else(|| anyhow!("slot = \"chest\" accepted"))?
        .to_string();
        assert!(
            err.contains("unknown variant `chest`") && err.contains("`body`") && err.contains("line 4"),
            "{err}"
        );
        assert!(
            parse_farm(
                "[[farm]]\nfor = \"X\"\nitems = [{ name = \"Smoke\", kind = \"runeword\", merc = \"yes\" }]\n"
            )
            .is_err()
        );
        Ok(())
    }

    /// The settings of an old `config.toml`'s tables ([`parse_tables`]), with its warnings.
    fn settings(text: &str) -> Result<(Settings, Vec<String>), toml::de::Error> {
        parse_tables(text).map(|(tables, warnings)| (tables.into_settings(), warnings))
    }

    #[test]
    fn tables_default_and_parse_valuables() -> Result<()> {
        let c = Settings::default();
        assert_eq!(c.valuables.trade, vec![Tier::S, Tier::A]);
        assert_eq!(c.valuables.twink, vec![Tier::S, Tier::A]);
        assert_eq!(parse_tables("")?, (Tables::default(), vec![]), "no tables");
        assert_eq!(settings("[valuables]\n")?.0, c, "empty section = defaults");
        assert_eq!(
            settings("[shrines]\nshow = []\nplaystyle = \"x\"\n")?,
            (c, vec![]),
            "the core's keys are skipped"
        );
        let (c, _) = settings("[valuables]\ntrade = [\"S\"]\ntwink = []\n")?;
        assert_eq!(c.valuables.trade, vec![Tier::S]);
        assert_eq!(c.valuables.twink, vec![]);
        assert!(!c.valuables.enabled(Category::Trade, Tier::A));
        assert!(c.valuables.enabled(Category::Trade, Tier::S));
        let (c, _) = settings("[valuables]\ntwink = [\"B\"]\n")?;
        assert_eq!(c.valuables.trade, vec![Tier::S, Tier::A], "missing key = default");
        assert_eq!(c.valuables.twink, vec![Tier::B]);
        for bad in [
            "[valuables\n",
            "[valuables]\ntrade = [\"X\"]\n",
            "[valuables]\ntrade = \"S\"\n",
            "[valuables]\ntrdae = [\"S\"]\n",
            "[bis.X]\nbiuld = \"b\"\n",
        ] {
            assert!(parse_tables(bad).is_err(), "{bad:?} accepted");
        }
        Ok(())
    }

    #[test]
    fn playstyle_toml_parses() -> Result<()> {
        let ((name, tables), warnings) = parse_playstyle(
            "name = \"N\"\n[valuables]\ntrade = [\"S\"]\n[damages]\nVex = [\"fire\"]\n\
             [damages.class]\ndruid = [\"physical\"]\n",
        )?;
        assert_eq!(name, "N");
        assert_eq!(tables.valuables.map(|v| v.trade), Some(vec![Tier::S]));
        let damages = tables.damages.unwrap_or_default();
        assert_eq!(
            damages.names.get("vex").map(Vec::as_slice),
            Some(&[Damage::Fire][..])
        );
        assert_eq!(
            damages.classes.get(&5).map(Vec::as_slice),
            Some(&[Damage::Physical][..])
        );
        assert_eq!(warnings, Vec::<String>::new());
        assert!(parse_playstyle("description = \"no name\"\n").is_err());
        assert!(
            parse_playstyle("name = \"N\"\n[bis.X]\nbuild = \"b\"\n").is_err(),
            "bis is state"
        );
        Ok(())
    }

    #[test]
    fn parses_builds() -> Result<()> {
        use Damage::{Cold, Fire, Lightning, Physical};
        let (c, warnings) = settings(
            "[builds]\nVex = [\"lightning\", \"Fire\", \"physical\", \"fire\"]\nOther = [\"holy\", \"cold\"]\n\n\
             [builds.class]\ndruid = [\"physical\"]\n4 = [\"physical\"]\nbard = [\"fire\"]\n",
        )?;
        assert_eq!(
            c.damages_for_player("Vex", Some(6), None, &owns_nothing, None),
            [Lightning, Fire, Physical],
            "deduped, in order"
        );
        assert_eq!(
            c.damages_for_player("vex", Some(1), None, &owns_nothing, None),
            [Lightning, Fire, Physical],
            "name in any case"
        );
        assert_eq!(
            c.damages_for_player("Other", Some(5), None, &owns_nothing, None),
            [Cold],
            "own entry beats the class"
        );
        assert_eq!(
            c.damages_for_player("Bear", Some(5), None, &owns_nothing, None),
            [Physical],
            "class by name"
        );
        assert_eq!(
            c.damages_for_player("Barb", Some(4), None, &owns_nothing, None),
            [Physical],
            "class by id"
        );
        assert_eq!(
            c.damages_for_player("Sorc", Some(1), None, &owns_nothing, None),
            [] as [Damage; 0],
            "nothing configured"
        );
        assert_eq!(
            c.damages_for_player("Sorc", None, None, &owns_nothing, None),
            [] as [Damage; 0],
            "no class known: still no damages"
        );
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(warnings.iter().any(|w| w.contains("\"holy\"")), "{warnings:?}");
        assert!(warnings.iter().any(|w| w.contains("\"bard\"")), "{warnings:?}");
        assert!(
            parse_tables("[builds]\nVex = \"lightning\"\n").is_err(),
            "a list is required"
        );
        Ok(())
    }

    #[test]
    fn bis_entries() -> Result<()> {
        let (config, warnings) = settings(
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
        let (config, warnings) = settings("[bis.X]\nbuild = \"b\"\n")?;
        assert_eq!(config.bis.get("x").map(|b| b.build.as_str()), Some("b"));
        assert_eq!(warnings, Vec::<String>::new());
        // Neither a build nor slots: ignored with a warning.
        let (_, warnings) = settings("[bis.X]\nbuild = \"\"\n[bis.X.slots]\nhelm = [\" \"]")?;
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        Ok(())
    }
}

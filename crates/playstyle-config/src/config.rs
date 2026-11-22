//! The playstyle's settings ([`Settings`]) parsed from its TOML files, and the stage logic over
//! them. Build files are agent-authored; a character only selects one (`[bis.<name>] build`).

use std::collections::BTreeMap;

use serde::Deserialize;

use crate::valuables::{Category, Tier};
use d2r_core::build::Damage;
use d2r_core::config::{CLASS_NAMES, SLOT_NAMES, class_id};
pub use d2r_core::guidance::Bis;
use d2r_core::item_stats::ItemStat;
use d2r_core::names::ItemKind;
use d2r_core::socket_bases::MercAct;
pub use d2r_core::web::SkillTarget;

/// The playstyle's settings, as loaded from its files ([`crate::files::load`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Settings {
    /// `playstyle.toml`'s `name` ("Summoner Druid → Fury Druid"); empty without one.
    pub name: String,
    /// `[valuables]`: the drop mark tiers.
    pub valuables: Valuables,
    /// `[damages]` (`playstyle.toml`; `[builds]` in an old `config.toml`).
    pub builds: Builds,
    /// `[bis.<name>]`: the build selection, plus legacy inline slots.
    pub bis: BTreeMap<String, Bis>,
    /// `builds/*.toml`, keyed by slug (the lowercased build name).
    pub build_defs: BTreeMap<String, Build>,
    /// `farm.toml`.
    pub farm: Vec<FarmGroup>,
    /// `route.toml`.
    pub route: crate::route::Route,
    /// `not-needed.toml`, already taken out of `build_defs` and `farm`; still applied to a
    /// character's own `[bis.<name>]` slots ([`Settings::resolved_bis`]).
    pub not_needed: crate::not_needed::Marks,
}

/// `[valuables]`: the tiers highlighted per category (see [`crate::valuables`]), and whether
/// the money rules count.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Valuables {
    pub trade: Vec<Tier>,
    pub twink: Vec<Tier>,
    /// The money rules (vendor-gold wands, scepters, orbs, daggers) mark and keep: the /config
    /// page's money farming switch (`settings.toml` in the data dir), no `[valuables]` key.
    #[serde(skip)]
    pub money: bool,
}

impl Default for Valuables {
    fn default() -> Self {
        Self {
            trade: vec![Tier::S, Tier::A],
            twink: vec![Tier::S, Tier::A],
            money: true,
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
            money: true,
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
    /// `playstyle.toml`'s `filter_name`: the `name` its `loot-filter.json` profile should have.
    pub filter_name: Option<String>,
}

#[cfg(test)]
impl Tables {
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

/// The playstyle's tables of an old `config.toml` or of `bis.toml`; the core reads the rest.
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
    #[serde(default, rename = "description")]
    _description: String,
    valuables: Option<Valuables>,
    damages: Option<RawBuilds>,
    filter_name: Option<String>,
}

/// `[damages]` / `[builds]`: character name = damage names, and `[class]`: class = damage names.
#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct RawBuilds {
    class: BTreeMap<String, Vec<String>>,
    #[serde(flatten)]
    names: BTreeMap<String, Vec<String>>,
}

/// `[bis.<name>]`: the selected build's slug; legacy inline `slots` win over the build's.
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

/// Trims a piece list and drops the blanks.
#[must_use]
pub fn clean_pieces(pieces: &[String]) -> Vec<String> {
    pieces
        .iter()
        .map(|p| p.trim().to_owned())
        .filter(|p| !p.is_empty())
        .collect()
}

/// Parses a `slots`/`merc_slots` table (`"Piece@<required level>"` per slot); `label` names the
/// entry in the warnings.
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

/// Parses the playstyle tables of an old `config.toml` or of `bis.toml`, with warnings about
/// ignored entries.
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
        filter_name: None,
    };
    Ok((tables, warnings))
}

/// Parses `playstyle.toml` into its display name and tables, with warnings as [`parse_tables`].
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
        filter_name: raw
            .filter_name
            .map(|n| n.trim().to_owned())
            .filter(|n| !n.is_empty()),
    };
    Ok(((raw.name, tables), warnings))
}

/// A build definition from `builds/<slug>.toml`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Build {
    /// The display name; its lowercase form is the slug.
    pub name: String,
    /// Primary first.
    pub damages: Vec<Damage>,
    pub notes: Option<String>,
    /// Slot name → pieces, best first.
    pub slots: BTreeMap<String, Vec<String>>,
    pub merc_slots: BTreeMap<String, Vec<String>>,
    /// `merc_acts = [2, 1]` (or `merc_act = 5`), primary first; empty reads as Act 2.
    pub merc_acts: Vec<MercAct>,
    /// `weapon_rule = { two_handed, melee }`: what the player's `weapon` slot takes.
    pub weapon_rule: d2r_core::socket_bases::WeaponRule,
    /// `[pieces."<name>"]`: what the dashboard shows under a piece.
    pub pieces: BTreeMap<String, PieceInfo>,
    /// Free-text goals shown above the gear grid.
    pub targets: Vec<String>,
    /// `[extras]`: group name → pieces outside the equipment slots (jewels, runes).
    pub extras: BTreeMap<String, Vec<String>>,
    /// `[[charms]]`: the charm wishlist, in the gear grid's order.
    pub charms: Vec<crate::charms::CharmWish>,
    pub class: Option<u32>,
    /// The build of a character of [`Build::class`] without a `[bis.<name>]` selection.
    pub default_for_class: bool,
    /// The slug of the next stage, see [`Settings::stage`].
    pub next: Option<String>,
    pub switch_level: Option<u8>,
    /// When false the dashboard only says when to switch on `/config`.
    pub auto_switch: bool,
    pub switch_note: Option<String>,
    /// Every group must be met for the automatic switch; empty: the level alone decides.
    pub switch_requires: Vec<SwitchGroup>,
    /// `[[skill_plan]]`: cumulative hard-point targets.
    pub skill_plan: Vec<SkillTarget>,
    pub recipes: Vec<Recipe>,
    pub tips: Vec<Tip>,
    /// One line per level range.
    pub stats_plan: Vec<String>,
}

impl Build {
    /// The primary mercenary's act: the first of [`Build::merc_acts`], Act 2 when empty.
    #[must_use]
    pub fn merc_act(&self) -> MercAct {
        self.merc_acts.first().copied().unwrap_or_default()
    }

    /// The act of the mercenary that wears merc piece `entry` in body `slot`: for a runeword the
    /// first of [`Build::merc_acts`] that can carry it there, else the primary one.
    #[must_use]
    pub fn merc_wearer(&self, entry: &str, slot: Option<u8>) -> MercAct {
        d2r_core::runewords_data::lookup(d2r_core::info::piece_name(entry))
            .and_then(|word| d2r_core::socket_bases::merc_act_for(word, &self.merc_acts, slot))
            .unwrap_or_else(|| self.merc_act())
    }
}

/// One condition of a stage switch: met when the account holds `count` copies of the items in
/// `any` together (unique, set or runeword names).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SwitchGroup {
    pub label: String,
    /// At least one.
    pub any: Vec<String>,
    /// At least 1.
    pub count: u32,
}

impl SwitchGroup {
    /// One copy of `name`, labelled with it.
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

    #[must_use]
    pub fn met(&self, copies: &dyn Fn(&str) -> u32) -> bool {
        self.have(copies) >= self.count
    }
}

/// One `[[recipes]]` entry, shown as a card.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Recipe {
    pub name: String,
    /// One of [`RECIPE_KINDS`].
    pub kind: String,
    /// One per line: a rune (`"3 Tal Rune"`), a gem (`"1 Chipped Topaz"`) or a free-text base.
    pub inputs: Vec<String>,
    pub result: String,
    pub req: String,
    pub notes: String,
    /// The quest whose reward this recipe is (empty: none); hidden once the reward is used.
    pub quest: String,
}

/// The recipe kinds the dashboard groups by.
pub const RECIPE_KINDS: [&str; 5] = ["runeword", "craft", "cube", "socket", "upgrade"];

/// A leveling tip shown while the character level is in `from..=to`.
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

/// `[pieces."<name>"]` of a build file (all optional).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PieceInfo {
    /// unique, set, runeword, crafted, rare, magic or base.
    pub kind: String,
    /// starter, standard or alternative.
    pub setup: String,
    pub base: String,
    /// The key mods with their ranges, one line each.
    pub stats: Vec<String>,
    pub why: String,
    pub notes: String,
}

/// One `[[farm]]` group of `farm.toml`: the gear a planned build needs.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FarmGroup {
    /// `for = "Fury Druid"`.
    #[serde(rename = "for")]
    pub target: String,
    pub items: Vec<FarmItem>,
}

/// One farm-list entry: a unique, set or runeword by name, or a rolled item (`kind = "rolled"`:
/// any item of `slot` with the stats in `needs`).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FarmItem {
    pub name: String,
    pub kind: FarmKind,
    /// Copies needed (default 1).
    #[serde(default = "one")]
    pub count: u32,
    /// For the mercenary of the group's build.
    #[serde(default)]
    pub merc: bool,
    /// The body slot it is worn in; narrows a runeword's base hint. Left out: any slot.
    #[serde(default)]
    pub slot: Option<FarmSlot>,
    /// `needs = ["Teleport"]`: skills a rolled entry's item must grant.
    #[serde(default)]
    pub needs: Vec<FarmStat>,
    #[serde(default)]
    pub note: Option<String>,
}

impl FarmItem {
    /// Whether an item of `code` with `stats` is a copy of this rolled entry (never for a named
    /// kind or a rolled entry without needs).
    #[must_use]
    pub fn rolled_match(&self, code: Option<&str>, stats: &[ItemStat]) -> bool {
        let slot_ok = self.slot.is_none_or(|slot| {
            code.and_then(d2r_core::bases_data::base)
                .is_some_and(|b| b.slot == slot.body())
        });
        self.kind == FarmKind::Rolled
            && !self.needs.is_empty()
            && slot_ok
            && self
                .needs
                .iter()
                .all(|n| crate::gear_score::grants_skill(stats, n.skill))
    }

    /// Whether this entry wants Teleport, which Enigma gives.
    #[must_use]
    pub fn wants_teleport(&self) -> bool {
        self.kind == FarmKind::Rolled && self.needs.iter().any(|n| n.skill == crate::gear_score::TELEPORT)
    }
}

/// One stat a rolled [`FarmItem`] needs: a skill by its skills.txt name ("Teleport").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(try_from = "String")]
pub struct FarmStat {
    pub skill: u16,
}

impl FarmStat {
    #[must_use]
    pub fn name(self) -> &'static str {
        d2r_core::skills_data::name(self.skill).unwrap_or("?")
    }
}

impl TryFrom<String> for FarmStat {
    type Error = String;

    fn try_from(name: String) -> Result<Self, Self::Error> {
        d2r_core::skills_data::SKILLS
            .iter()
            .find(|s| d2r_core::names::same_name(s.name, &name))
            .map(|s| Self { skill: s.id })
            .ok_or_else(|| {
                format!("unknown stat {name:?} in needs: expected a skill name such as \"Teleport\"")
            })
    }
}

/// The body slot of a [`FarmItem`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FarmSlot {
    Helm,
    Amulet,
    Body,
    Weapon,
    #[serde(alias = "offhand")]
    Shield,
    Ring,
    Belt,
    Boots,
    Gloves,
}

impl FarmSlot {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Helm => "helm",
            Self::Amulet => "amulet",
            Self::Body => "body",
            Self::Weapon => "weapon",
            Self::Shield => "shield",
            Self::Ring => "ring",
            Self::Belt => "belt",
            Self::Boots => "boots",
            Self::Gloves => "gloves",
        }
    }

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

/// What kind of item a [`FarmItem`] names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FarmKind {
    Runeword,
    Unique,
    Set,
    Rolled,
}

impl FarmKind {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Runeword => "runeword",
            Self::Unique => "unique",
            Self::Set => "set",
            Self::Rolled => "rolled",
        }
    }

    /// The core kind of a named entry, `None` for a rolled one.
    #[must_use]
    pub const fn item_kind(self) -> Option<ItemKind> {
        match self {
            Self::Runeword => Some(ItemKind::Runeword),
            Self::Unique => Some(ItemKind::Unique),
            Self::Set => Some(ItemKind::Set),
            Self::Rolled => None,
        }
    }
}

const fn one() -> u32 {
    1
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawFarm {
    farm: Vec<FarmGroup>,
}

/// Parses `farm.toml`'s text, with warnings about names not in the game tables (kept) and a
/// `count` of 0 (read as 1).
///
/// # Errors
/// Fails on invalid TOML, unknown keys, an unknown `kind` or wrongly typed values.
pub fn parse_farm(text: &str) -> Result<(Vec<FarmGroup>, Vec<String>), toml::de::Error> {
    let mut groups = toml::from_str::<RawFarm>(text)?.farm;
    let mut warnings = Vec::new();
    for group in &mut groups {
        for item in &mut group.items {
            check_farm_item(&group.target, item, &mut warnings);
        }
    }
    Ok((groups, warnings))
}

/// Warns about a farm entry that can never be found or has ignored fields; a `count` of 0
/// becomes 1.
fn check_farm_item(target: &str, item: &mut FarmItem, warnings: &mut Vec<String>) {
    if item.count == 0 {
        warnings.push(format!("{target}: {:?} has count 0; read as 1", item.name));
        item.count = 1;
    }
    let rolled = item.kind == FarmKind::Rolled;
    if rolled && item.needs.is_empty() {
        warnings.push(format!(
            "{target}: rolled {:?} has no needs (kept, but it can never be found)",
            item.name
        ));
    }
    if !rolled && !item.needs.is_empty() {
        warnings.push(format!(
            "{target}: {:?} is a {}; its needs are ignored (only rolled entries have them)",
            item.name,
            item.kind.name()
        ));
    }
    if !farm_name_known(item) {
        warnings.push(format!(
            "{target}: unknown {} {:?} (kept, but it can never be found)",
            item.kind.name(),
            item.name
        ));
    }
}

/// Whether a named farm entry's name is in the game tables of its kind; a rolled entry has any
/// name.
fn farm_name_known(item: &FarmItem) -> bool {
    let name = item.name.as_str();
    match item.kind {
        FarmKind::Runeword => is_runeword_name(name),
        FarmKind::Unique => is_unique_name(name) || is_display_name(name),
        FarmKind::Set => is_set_name(name) || is_display_name(name),
        FarmKind::Rolled => true,
    }
}

fn is_runeword_name(name: &str) -> bool {
    d2r_core::runewords_data::lookup(name).is_some()
}

fn is_unique_name(name: &str) -> bool {
    d2r_core::item_names_data::UNIQUE_NAMES
        .iter()
        .any(|(_, n)| d2r_core::names::same_name(n, name))
}

fn is_set_name(name: &str) -> bool {
    d2r_core::item_names_data::SET_NAMES
        .iter()
        .any(|(_, n)| d2r_core::names::same_name(n, name))
}

/// A unique's or set item's display override (the in-game name where it differs from the table's).
fn is_display_name(name: &str) -> bool {
    d2r_core::item_names_data::DISPLAY_NAMES
        .iter()
        .any(|(_, n)| d2r_core::names::same_name(n, name))
}

fn item_name_known(name: &str) -> bool {
    is_runeword_name(name) || is_unique_name(name) || is_set_name(name) || is_display_name(name)
}

/// The copy count of callers without live data (the config page): a `switch_requires` is never
/// met.
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
    /// The chain started at the class default (no `[bis.<name>]` selection).
    pub class_default: bool,
    /// `None` when the current stage names no build or one already in the chain.
    pub next: Option<(&'a str, &'a Build)>,
}

/// A build selection's playstyle folder and build slug: `"<folder>/<slug>"` (what the
/// `/config` page saves, the folder being a plain name), else the whole value is the slug of
/// a build in any playstyle.
#[must_use]
pub fn split_selection(value: &str) -> (Option<&str>, String) {
    let value = value.trim();
    match value.split_once('/') {
        Some((folder, slug))
            if !folder.is_empty()
                && folder
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_') =>
        {
            (Some(folder), slug.trim().to_lowercase())
        }
        _ => (None, value.to_lowercase()),
    }
}

/// The playstyle character `name` (of class id `class`, when known) follows among `folders`
/// (folder name and settings, the selected playstyle first): the one its selection names,
/// else the first holding the selected build, else the first with a default build for its
/// class, else the selected playstyle. `None` only without folders.
#[must_use]
pub fn playstyle_of<'a>(
    folders: &[(&'a str, &'a Settings)],
    name: &str,
    class: Option<u32>,
) -> Option<(&'a str, &'a Settings)> {
    let (_, first) = folders.first()?;
    if let Some(bis) = first.bis.get(&name.to_lowercase()) {
        let (folder, slug) = split_selection(&bis.build);
        let named = folder.and_then(|key| folders.iter().find(|(k, _)| *k == key));
        let holding = || folders.iter().find(|(_, c)| c.build_defs.contains_key(&slug));
        if let Some(found) = named.or_else(holding) {
            return Some(*found);
        }
    }
    class
        .and_then(|class| folders.iter().find(|(_, c)| c.class_default(class).is_some()))
        .or_else(|| folders.first())
        .copied()
}

impl Settings {
    fn selected(&self, name: &str) -> Option<(&str, &Build)> {
        let (_, slug) = split_selection(&self.bis.get(&name.to_lowercase())?.build);
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
    /// followed along `next` while the switch is due, until the chain would repeat.
    /// `class`/`level` are `None` where unknown: no class default, no automatic switch.
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
        while let Some((next_slug, next_build)) = self.next_stage(build) {
            if !self.switch_due(build, next_slug, level, owns, reached) || chain.contains(&next_slug) {
                break;
            }
            chain.push(next_slug);
            (slug, build) = (next_slug, next_build);
        }
        Some(Stage {
            slug,
            build,
            class_default,
            next: self.next_stage(build).filter(|(next, _)| !chain.contains(next)),
        })
    }

    fn next_stage(&self, build: &Build) -> Option<(&str, &Build)> {
        let next = build.next.as_deref()?;
        self.build_defs
            .get_key_value(next)
            .map(|(slug, build)| (slug.as_str(), build))
    }

    /// Whether `build` switches on to `next_slug`: automatic, `level` reached, and every
    /// `switch_requires` group met (`owns` gives an item's copies held).
    /// A stage `reached` before at or after `next_slug` counts the groups as met: a switch is
    /// one-way, and the shared stash is unread until opened each game.
    fn switch_due(
        &self,
        build: &Build,
        next_slug: &str,
        level: Option<u8>,
        owns: &dyn Fn(&str) -> u32,
        reached: Option<&str>,
    ) -> bool {
        let level_reached = build
            .switch_level
            .zip(level)
            .is_some_and(|(switch, level)| level >= switch);
        let requirements_met = build.switch_requires.iter().all(|group| group.met(owns))
            || reached.is_some_and(|reached| self.leads_to(next_slug, reached));
        build.auto_switch && level_reached && requirements_met
    }

    /// The names of build `from` and of every stage after it (an unknown slug gives none): the
    /// builds one character counts as for the farm list, as it keeps its gear through a respec.
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

    /// Whether stage `to` is `from` or comes after it along the builds' `next` links.
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

    /// The character's `BiS`: its inline slots when it has any, else the current stage's.
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
            let mut bis = bis.clone();
            // The dashboard's ✕ writes under `bis.build`: without a selection, the character.
            if bis.build.is_empty() {
                name.clone_into(&mut bis.build);
            }
            if let Some(marks) = self.not_needed.get(&bis.build.to_lowercase()) {
                crate::not_needed::bis_without(&mut bis, marks);
            }
            return Some(bis);
        }
        let def = self.stage(name, class, level, owns, reached)?.build;
        Some(Bis {
            build: def.name.clone(),
            slots: def.slots.clone(),
            merc_slots: def.merc_slots.clone(),
        })
    }

    /// The character's damage types, primary first: its own `[builds]` entry, else the current
    /// stage's, else its class's.
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

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawBuild {
    name: String,
    damages: Vec<String>,
    notes: String,
    slots: BTreeMap<String, Vec<String>>,
    merc_slots: BTreeMap<String, Vec<String>>,
    merc_act: Option<u8>,
    merc_acts: Vec<u8>,
    weapon_rule: RawWeaponRule,
    pieces: BTreeMap<String, PieceInfo>,
    targets: Vec<String>,
    extras: BTreeMap<String, Vec<String>>,
    charms: Vec<crate::charms::CharmWish>,
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

/// `weapon_rule = { two_handed = true, melee = true }`: what the player's `weapon` slot takes.
#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct RawWeaponRule {
    two_handed: bool,
    melee: bool,
}

/// A `switch_requires` entry: an item name, or `{ label, any = [...], count }`. Hand-written
/// rather than `#[serde(untagged)]`, which would hide serde's message for a bad table.
#[derive(Debug)]
enum RawSwitchGroup {
    Item(String),
    Group(RawSwitchAny),
}

impl<'de> Deserialize<'de> for RawSwitchGroup {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
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
    count: Option<u32>,
}

/// The [`SwitchGroup`]s of a build's `switch_requires`; blank names and empty groups are dropped,
/// unknown item names warned about and kept.
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
            RawSwitchGroup::Group(table) => match switch_group_of_table(&table, label, warnings) {
                Some(group) => group,
                None => continue,
            },
        };
        warn_unknown_switch_items(&group, label, warnings);
        groups.push(group);
    }
    groups
}

/// The group of a `{ label, any, count }` table: `None` (warned) without items; labelled with
/// its items when unlabelled; `count = 0` warned and taken as 1.
fn switch_group_of_table(
    table: &RawSwitchAny,
    label: &str,
    warnings: &mut Vec<String>,
) -> Option<SwitchGroup> {
    let any = clean_pieces(&table.any);
    let mut group_label = table.label.trim().to_owned();
    if any.is_empty() {
        warnings.push(format!(
            "{label}: switch_requires group {group_label:?} lists no items (any = []); ignored"
        ));
        return None;
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
    Some(SwitchGroup {
        label: group_label,
        any,
        count,
    })
}

fn warn_unknown_switch_items(group: &SwitchGroup, label: &str, warnings: &mut Vec<String>) {
    for item in &group.any {
        if !item_name_known(item) {
            warnings.push(format!(
                "{label}: switch_requires names unknown item {item:?} (kept, but it can never be owned)"
            ));
        }
    }
}

/// A build's mercenary acts, primary first: `merc_acts` when given, else the single
/// `merc_act`; an unknown act or a repeat is warned about and dropped; none left = `[Act 2]`.
fn merc_acts(single: Option<u8>, list: &[u8], label: &str, warnings: &mut Vec<String>) -> Vec<MercAct> {
    let key = if list.is_empty() { "merc_act" } else { "merc_acts" };
    if single.is_some() && !list.is_empty() {
        warnings.push(format!(
            "{label}: both merc_act and merc_acts given; merc_act ignored"
        ));
    }
    let numbers = if list.is_empty() { single.as_slice() } else { list };
    let mut acts: Vec<MercAct> = Vec::new();
    for &n in numbers {
        match MercAct::from_number(n) {
            Some(act) if !acts.contains(&act) => acts.push(act),
            Some(_) => warnings.push(format!("{label}: {key} lists act {n} twice")),
            None => warnings.push(format!("{label}: {key} {n} unknown (known: 1, 2, 3, 5); ignored")),
        }
    }
    if acts.is_empty() {
        acts.push(MercAct::default());
    }
    acts
}

/// Parses one build file's text: its slug, the [`Build`], and warnings about ignored entries.
///
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
    let merc_acts = merc_acts(raw.merc_act, &raw.merc_acts, &label, &mut warnings);
    let notes = raw.notes.trim().to_owned();
    let class = class_of_build(&raw.class, &label, &mut warnings);
    check_skill_names(&raw.skill_plan, class, &label, &mut warnings);
    check_recipes(&raw.recipes, &label, &mut warnings);
    let tips = parse_tips(raw.tips, &label, &mut warnings);
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
            merc_acts,
            weapon_rule: d2r_core::socket_bases::WeaponRule {
                two_handed: raw.weapon_rule.two_handed,
                melee: raw.weapon_rule.melee,
            },
            pieces: raw.pieces,
            targets: clean_pieces(&raw.targets),
            extras: clean_extras(raw.extras),
            charms: crate::charms::clean(raw.charms, &label, &mut warnings),
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

fn check_recipes(recipes: &[Recipe], label: &str, warnings: &mut Vec<String>) {
    for recipe in recipes {
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
}

/// The tips with readable `levels`; the others are warned about and dropped.
fn parse_tips(raw: Vec<RawTip>, label: &str, warnings: &mut Vec<String>) -> Vec<Tip> {
    let mut tips = Vec::new();
    for tip in raw {
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
    tips
}

/// The `[extras]` groups with their pieces cleaned; groups left empty are dropped.
fn clean_extras(raw: BTreeMap<String, Vec<String>>) -> BTreeMap<String, Vec<String>> {
    raw.into_iter()
        .map(|(group, pieces)| (group, clean_pieces(&pieces)))
        .filter(|(_, pieces)| !pieces.is_empty())
        .collect()
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

/// Warns about skill-plan skills that aren't skills of `class` (any class when unknown).
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
        assert!(
            parse_build("name = \"B\"\n[pieces.X]\ncolour = \"red\"\n").is_err(),
            "unknown key in a piece"
        );
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
        assert_eq!(warnings.len(), 3, "{warnings:?}");
        assert!(
            warnings.iter().any(|w| w.contains("unknown quest \"Larzuk\"")),
            "{warnings:?}"
        );
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
        let s = stage(Some(37)).ok_or_else(|| anyhow!("no stage"))?;
        assert_eq!((s.slug, s.class_default), ("leveling", true));
        assert_eq!(s.next.map(|(slug, _)| slug), Some("tornado"));
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

    /// A stand-in for [`d2r_core::stash::Holdings::count`].
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
        assert_eq!(
            config
                .stage("NewDruid", Some(5), Some(90), &owns_nothing, None)
                .map(|s| s.slug),
            Some("leveling")
        );
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
        assert_eq!(
            slug(38, Some("fissure"))?,
            "fissure",
            "a later stage reached latches"
        );
        assert_eq!(
            slug(37, Some("fissure"))?,
            "leveling",
            "the level is still checked"
        );
        assert_eq!(
            slug(38, Some("other"))?,
            "leveling",
            "a stage off the chain latches nothing"
        );
        assert!(config.leads_to("leveling", "fissure") && config.leads_to("tornado", "tornado"));
        assert!(!config.leads_to("fissure", "leveling"), "only forwards");
        if let Some(b) = config.build_defs.get_mut("fissure") {
            b.next = Some("leveling".to_owned());
        }
        assert!(!config.leads_to("leveling", "missing"));
        Ok(())
    }

    #[test]
    fn stage_selection_beats_the_default_and_cycles_stop() -> Result<()> {
        let mut config = druid_chain();
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

    #[test]
    fn rolled_farm_entries_take_slot_needs_and_note() -> Result<()> {
        let (groups, warnings) = parse_farm(
            "[[farm]]\nfor = \"Fury Druid\"\nitems = [\n\
             { name = \"Teleport Amulet\", kind = \"rolled\", slot = \"amulet\", needs = [\"teleport\"], note = \"until Enigma\" },\n\
             { name = \"Anything\", kind = \"rolled\" },\n\
             { name = \"Smoke\", kind = \"runeword\", needs = [\"Teleport\"] },\n]\n",
        )?;
        let items = &groups.first().ok_or_else(|| anyhow!("no group"))?.items;
        let amulet = items.first().ok_or_else(|| anyhow!("no entry"))?;
        assert_eq!(amulet.kind, FarmKind::Rolled);
        assert_eq!(amulet.slot, Some(FarmSlot::Amulet));
        assert_eq!(
            amulet.needs,
            [FarmStat {
                skill: crate::gear_score::TELEPORT
            }],
            "skill names match case-insensitively"
        );
        assert_eq!(amulet.needs.first().map(|n| n.name()), Some("Teleport"));
        assert_eq!(amulet.note.as_deref(), Some("until Enigma"));
        assert!(amulet.wants_teleport());
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(
            warnings.iter().any(|w| w.contains("\"Anything\" has no needs")),
            "{warnings:?}"
        );
        assert!(
            warnings.iter().any(|w| w.contains("\"Smoke\" is a runeword")),
            "{warnings:?}"
        );
        let charges = ItemStat {
            id: d2r_core::item_stats::stat::CHARGED_SKILL,
            param: (crate::gear_score::TELEPORT << 6) | 3,
            value: 12,
            base: false,
        };
        assert!(amulet.rolled_match(Some("amu"), &[charges]));
        assert!(
            !amulet.rolled_match(Some("rin"), &[charges]),
            "a ring is no amulet"
        );
        assert!(!amulet.rolled_match(Some("amu"), &[]));
        let err = parse_farm(
            "[[farm]]\nfor = \"X\"\nitems = [\n{ name = \"T\", kind = \"rolled\", needs = [\"Telport\"] },\n]\n",
        )
        .err()
        .ok_or_else(|| anyhow!("needs = [\"Telport\"] accepted"))?
        .to_string();
        assert!(
            err.contains("unknown stat \"Telport\"") && err.contains("Teleport") && err.contains("line 4"),
            "{err}"
        );
        Ok(())
    }

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
            "name = \"N\"\nfilter_name = \" F \"\n[valuables]\ntrade = [\"S\"]\n[damages]\nVex = [\"fire\"]\n\
             [damages.class]\ndruid = [\"physical\"]\n",
        )?;
        assert_eq!((name.as_str(), tables.filter_name.as_deref()), ("N", Some("F")));
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
        let (config, warnings) = settings("[bis.X]\nbuild = \"b\"\n")?;
        assert_eq!(config.bis.get("x").map(|b| b.build.as_str()), Some("b"));
        assert_eq!(warnings, Vec::<String>::new());
        let (_, warnings) = settings("[bis.X]\nbuild = \"\"\n[bis.X.slots]\nhelm = [\" \"]")?;
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        Ok(())
    }

    #[test]
    fn merc_act_defaults_to_act_2() -> Result<(), toml::de::Error> {
        let (_, five, warnings) = parse_build("name = \"B\"\nmerc_act = 5\n")?;
        assert_eq!((five.merc_acts, warnings), (vec![MercAct::Act5], Vec::new()));
        let (_, none, _) = parse_build("name = \"B\"\n")?;
        assert_eq!(
            (none.merc_acts.clone(), none.merc_act()),
            (vec![MercAct::Act2], MercAct::Act2)
        );
        assert_eq!(Build::default().merc_act(), MercAct::Act2, "empty: Act 2");
        let (_, four, warnings) = parse_build("name = \"B\"\nmerc_act = 4\n")?;
        assert_eq!(four.merc_acts, vec![MercAct::Act2]);
        assert_eq!(
            warnings,
            vec!["build B: merc_act 4 unknown (known: 1, 2, 3, 5); ignored".to_owned()]
        );
        Ok(())
    }

    #[test]
    fn a_build_lists_several_merc_acts_primary_first() -> Result<(), toml::de::Error> {
        let text = "name = \"B\"\nmerc_acts = [2, 1]\n[merc_slots]\nweapon = [\"Insight@27\", \"Harmony@39\", \"Smoke@37\"]\n";
        let (_, build, warnings) = parse_build(text)?;
        assert_eq!(warnings, Vec::<String>::new());
        assert_eq!(build.merc_acts, vec![MercAct::Act2, MercAct::Act1]);
        assert_eq!(build.merc_act(), MercAct::Act2);
        assert_eq!(build.merc_wearer("Insight@27", Some(4)), MercAct::Act2);
        assert_eq!(build.merc_wearer("Harmony@39", Some(4)), MercAct::Act1);
        assert_eq!(build.merc_wearer("The Battlebranch@17", Some(4)), MercAct::Act2);
        let (_, odd, warnings) = parse_build("name = \"B\"\nmerc_act = 3\nmerc_acts = [5, 4, 5]\n")?;
        assert_eq!(odd.merc_acts, vec![MercAct::Act5]);
        assert_eq!(warnings.len(), 3, "{warnings:?}");
        Ok(())
    }

    #[test]
    fn a_build_names_its_weapon_rule() -> Result<(), toml::de::Error> {
        use d2r_core::socket_bases::WeaponRule;
        let (_, build, _) = parse_build("name = \"B\"\nweapon_rule = { two_handed = true, melee = true }\n")?;
        assert_eq!(
            build.weapon_rule,
            WeaponRule {
                two_handed: true,
                melee: true
            }
        );
        let (_, plain, _) = parse_build("name = \"B\"\n")?;
        assert_eq!(plain.weapon_rule, WeaponRule::default());
        Ok(())
    }
}

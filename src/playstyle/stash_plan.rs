//! Where each of the player's items belongs (user rule, 2026-11-20): shared-stash pages 1–2
//! hold what the player keeps for their own builds, pages 3–4 what is kept for trading, page 5
//! is the player's own "ignore page" ([`IGNORED_PAGE`]), and the rest is sold or given away.
//! Playstyle logic (`PLAN.md`, plugin refactor step 5).
//!
//! The items judged are the inventory, cube and personal stash ([`Snapshot::owned_items`]) and
//! the shared stash's normal pages 1–4 ([`Snapshot::stash`], session-cached); the gems / runes /
//! materials tab and page 5 are left alone (page 5's items get no verdict, nothing is moved
//! there, but they still count as held: a runeword there drops its base need). Each item gets
//! one [`Action`]:
//! - **Keep · own**: a piece of the active build or a later stage (`BiS` slots, extras,
//!   `switch_requires`) or of the farm list, as many copies as needed beyond the worn ones;
//!   and one white base per runeword need (the runeword's base type, exactly its socket count,
//!   usable by the wearer: the player class or the Act 2 mercenary), the best one held
//!   ([`base_rank`]: damage or defense first). Fine on pages 1–2 and in the personal stash;
//!   in the inventory or cube it is a Move to 1–2 (user rule, 2026-11-20).
//! - **Keep · trade**: what the valuables rules rate S or A ([`valuables::keep_tiers`]): trade
//!   goods, valuable uniques and sets, good bases; but a white base of a twink A rule only as a
//!   base need (own), else it is sold ([`trade_worthy`]). A base that is trade-worthy goes to
//!   trade, never own. Belongs on pages 3–4.
//! - **Move**: a keeper in the wrong place: an own item on pages 3–4, in the inventory or in the
//!   cube; a trade item anywhere but pages 3–4. The dashboard shows the place in red.
//! - **Sell**: a white base nobody needs (another copy of an own-use base, a worse base for a
//!   runeword whose better base is held, a plain base on a trade page) and a unique or set no rule wants ([`valuables::verdict`]).
//! - **Give away**: a unique or set only a B rule wants, and the extra copies of a keep-one
//!   item (Sigon's pieces).
//!
//! Magic, rare and crafted items (jewels and charms too; user request, 2026-11-20) go to
//! trade when a rule rates them so; else they are scored for the build ([`crate::gear_score`],
//! [`Scoring`]): kept for own use when unidentified (not judgeable yet), when they score at
//! least [`NEAR_WORN`] of what is worn in their slot, or when their affixes alone are worth
//! [`NOTABLE`] (skills, FCR, resistances …); a charm or jewel when it scores at all. The
//! mercenary's gear has no stats in the snapshot, so a base it can wear (helm, body, polearm,
//! spear) that fails the player's test is kept unscored. The rest is sold, with the reason.
//! Two hard rules come first (user rules, 2026-11-20): a weapon whose type no build of the
//! stage chain uses ([`build_weapon_types`]: axes, daggers, javelins, claws, orbs … for the
//! druid) is sold as "not a build weapon", identified or not (the merc's polearms and spears
//! excepted); identified boots without faster run/walk are sold as "no run speed".
//! Gems, runes, charms in the inventory (they work there), finished runewords that aren't
//! build pieces, quest items and anything else no rule speaks about get no action.

use std::cmp::Reverse;
use std::collections::BTreeMap;

use crate::config::{Bis, Build};
use crate::game::{ItemDrop, Place, Snapshot};
use crate::gear_score::{self, Piece, Profile};
use crate::item_stats::{ItemStat, stat};
use crate::items::DropKind;
use crate::names::same_name;
use crate::offsets::item_data;
use crate::playstyle::config::Settings;
use crate::runewords_data::{self, Runeword};
use crate::socket_bases::{self, Wearer};
use crate::stash::{StashTab, TabKind};
use crate::valuables::{self, Tier, Verdict};

/// The last shared-stash page for the player's own items; the pages after it are for trade.
const LAST_OWN_PAGE: u8 = 2;

/// The shared-stash page the plan ignores (user rule, 2026-11-20: "things i keep around for
/// stupidity"): its items get no verdict and nothing is moved there. They still count as held
/// ([`Snapshot::holdings`]), so a runeword there still drops its base need.
const IGNORED_PAGE: u8 = 5;

/// Where an item is: on the character (inventory, Horadric Cube, personal stash) or on a
/// shared-stash page (1-based, the game's UI order).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Spot {
    Inventory,
    Cube,
    Stash,
    Page(u8),
}

impl Spot {
    /// The short label shown after the item name: `inv`, `cube`, `stash`, `page 4`.
    #[must_use]
    pub fn label(self) -> String {
        match self {
            Self::Inventory => "inv".to_owned(),
            Self::Cube => "cube".to_owned(),
            Self::Stash => "stash".to_owned(),
            Self::Page(n) => format!("page {n}"),
        }
    }

    /// The side a shared page is for; `None` on the character and on the ignored page.
    const fn page_side(self) -> Option<Side> {
        match self {
            Self::Page(n) if n <= LAST_OWN_PAGE => Some(Side::Own),
            Self::Page(n) if n != IGNORED_PAGE => Some(Side::Trade),
            Self::Page(_) | Self::Inventory | Self::Cube | Self::Stash => None,
        }
    }
}

/// What a kept item is kept for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Side {
    /// The player's own builds: shared pages 1–2.
    Own,
    /// Trading: shared pages 3–4.
    Trade,
}

impl Side {
    /// The shared pages of the side, as shown: `1–2` / `3–4`.
    #[must_use]
    pub const fn pages(self) -> &'static str {
        match self {
            Self::Own => "1–2",
            Self::Trade => "3–4",
        }
    }
}

/// What to do with an item. Declared in the dashboard's order (the derived `Ord` sorts by it).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Action {
    Sell,
    /// Kept, but on the wrong place: move it to the side's pages.
    Move(Side),
    /// Kept where it is.
    Keep(Side),
    GiveAway,
}

/// One line of the dashboard's Sell / Keep panel: identical items (same name, spot and action)
/// merged with their count.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Advice {
    pub name: String,
    pub spot: Spot,
    pub action: Action,
    /// Why a scored magic / rare item is sold ("no build stats", "worse than worn").
    pub reason: Option<&'static str>,
    pub count: u32,
}

/// One item the plan judges, with where it is and its display name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Held {
    pub item: ItemDrop,
    pub spot: Spot,
    /// Unique/set name, runeword name (shared pages only: the reader names those from their
    /// runes) or base name with `eth` and sockets ([`valuables::display_name`]).
    pub name: String,
}

/// The items of `snap` the plan judges: the inventory, cube and personal-stash items of
/// [`Snapshot::owned_items`] and every item on the shared stash's normal pages
/// ([`Snapshot::stash`], which the session keeps after the tab units vanish; the shared items
/// of `owned_items` are only the live ones and carry no page). The materials tab is left out.
#[must_use]
pub fn held(snap: &Snapshot) -> Vec<Held> {
    held_from(&snap.owned_items, &snap.stash)
}

/// [`held`] over the owned items and the shared-stash tabs.
fn held_from(owned: &[(ItemDrop, Place)], tabs: &[StashTab]) -> Vec<Held> {
    let carried = owned.iter().filter_map(|(item, place)| {
        let spot = match place {
            Place::Inventory => Spot::Inventory,
            Place::Cube => Spot::Cube,
            Place::Stash => Spot::Stash,
            Place::Shared => return None,
        };
        Some(Held {
            item: item.clone(),
            spot,
            name: label(item, valuables::display_name(item)),
        })
    });
    let shared = tabs
        .iter()
        .filter_map(|tab| match tab.kind {
            TabKind::Page(n) => Some((n, &tab.items)),
            TabKind::Materials => None,
        })
        .flat_map(|(n, items)| {
            items.iter().map(move |s| {
                let item = s.to_drop();
                let name = label(&item, s.name.clone());
                Held {
                    item,
                    spot: Spot::Page(n),
                    name,
                }
            })
        });
    carried.chain(shared).collect()
}

/// `name` with the quality in front for a magic, rare or crafted item ("Rare Ring"; their
/// display name is the base's).
fn label(item: &ItemDrop, name: String) -> String {
    let quality = match item.quality {
        item_data::QUALITY_MAGIC => "Magic",
        item_data::QUALITY_RARE => "Rare",
        item_data::QUALITY_CRAFTED => "Crafted",
        _ => return name,
    };
    format!("{quality} {name}")
}

/// What the magic / rare scoring needs: the build's [`Profile`], the player's worn items
/// `(body slot, code, stats)` and the weapon types the builds use ([`build_weapon_types`]).
#[derive(Debug, Clone, Default)]
pub struct Scoring {
    pub profile: Profile,
    pub worn: Vec<(u8, Option<String>, Vec<ItemStat>)>,
    /// items.txt weapon types (`swor`, `staf`, …); empty = no builds known, every type judged.
    pub weapon_types: Vec<&'static str>,
}

/// The items.txt types of the weapons `builds` use (user rule, 2026-11-20: "druids barely use
/// javelins"): the weapon-slot bases of every piece in a weapon, swap or offhand slot and of
/// the single-item `switch_requires` groups. A unique or set piece gives its own base's type; a
/// runeword every weapon base that can carry it for the build's class (`class` when the build
/// names none, [`socket_bases::carries`]). A `switch_requires` group with alternatives counts
/// not at all: its runewords (Breath of the Dying, Hustle, Eternity …) allow almost any weapon,
/// and its uniques (Rune Master, The Jade Tan Do, Tomb Reaver) are wanted by name, not their
/// base type (user rule, 2026-11-20: "a rare axe or dagger is useless to this druid").
/// Mercenary slots are left out: its weapons have their own rule ([`socket_bases::merc_wears`]).
/// A runeword in a weapon or swap slot that no weapon base can carry for the class (an
/// unknown class, a runeword of another class's weapons) leaves the set incomplete: the rule
/// is off then (empty), rather than selling the weapons that piece stands for.
fn build_weapon_types<'a>(builds: impl Iterator<Item = &'a Build>, class: u32) -> Vec<&'static str> {
    let mut types: Vec<&'static str> = Vec::new();
    for build in builds {
        let wearer = Wearer::Player(build.class.unwrap_or(class));
        // (piece, in the weapon or swap slot): only there must a runeword have carriers.
        let held = build
            .slots
            .iter()
            .filter_map(|(slot, pieces)| match slot_body(slot) {
                Some(body @ (4 | 5)) => Some((body == 4, pieces)),
                _ => None,
            })
            .flat_map(|(weapon, pieces)| pieces.iter().map(move |p| (p, weapon)));
        let switch = build
            .switch_requires
            .iter()
            .filter(|g| g.any.len() == 1)
            .flat_map(|g| &g.any)
            .map(|p| (p, false));
        for (entry, weapon_slot) in held.chain(switch) {
            let name = piece_name(entry);
            let carriers = |word| {
                crate::bases_data::SOCKETABLE
                    .iter()
                    .map(|row| row.0)
                    .filter(|code| socket_bases::carries(word, code, wearer, Some(4)))
                    .collect()
            };
            let word = runewords_data::lookup(name);
            let codes: Vec<&str> = word.map_or_else(|| crate::names::named_base_codes(name), carriers);
            if weapon_slot && word.is_some() && codes.is_empty() {
                return Vec::new();
            }
            for base in codes.into_iter().filter_map(crate::bases_data::base) {
                if base.slot == 4 && !types.contains(&base.kind) {
                    types.push(base.kind);
                }
            }
        }
    }
    types.sort_unstable();
    types
}

impl Scoring {
    /// The scoring for `snap`'s character: its class, its allocated skills plus the skill plans
    /// of every build in the stage `chain` (by name, [`Settings::stage_chain`]), the worn
    /// weapon's kind, the chain's weapon types ([`build_weapon_types`]). Level, strength and dexterity are left out: keeping an item doesn't
    /// need it wearable today.
    #[must_use]
    pub fn new(snap: &Snapshot, config: &Settings, chain: &[String]) -> Self {
        let class = snap.player.class;
        let mut skills: Vec<u16> = snap.skills.points.iter().map(|&(id, _)| id).collect();
        let builds = || {
            chain
                .iter()
                .filter_map(|name| config.build_defs.get(&name.to_lowercase()))
        };
        let plans = builds().flat_map(|b| &b.skill_plan);
        for target in plans {
            let planned = crate::skills_data::SKILLS
                .iter()
                .find(|s| u32::from(s.class) == class && same_name(s.name, &target.skill));
            if let Some(s) = planned
                && !skills.contains(&s.id)
            {
                skills.push(s.id);
            }
        }
        let weapon = snap
            .equipped_items
            .iter()
            .find(|e| e.body == 4)
            .and_then(|e| crate::bases_data::base(e.code.as_deref()?))
            .map(|b| b.kind);
        Self {
            profile: Profile {
                class,
                level: None,
                strength: None,
                dexterity: None,
                skills,
                weapon,
                caster: gear_score::is_caster(class, weapon),
            },
            worn: snap
                .equipped_items
                .iter()
                .map(|e| (e.body, e.code.clone(), e.stats.clone()))
                .collect(),
            weapon_types: build_weapon_types(builds(), class),
        }
    }
}

/// A magic / rare item is kept when it scores at least this share of what is worn in its slot.
pub const NEAR_WORN: f64 = 0.8;

/// A magic / rare item is kept whatever is worn when its affixes alone score this much: one
/// planned +skill, +1 class skill, 20 resistance or life, 20 caster FCR.
pub const NOTABLE: f64 = 20.0;

/// The own-use verdict on a magic, rare or crafted item no trade rule wants: `Ok(())` keep,
/// `Err(reason)` sell.
fn judge(item: &ItemDrop, scoring: &Scoring) -> Result<(), &'static str> {
    let code = item.code.as_deref();
    let base = code.and_then(crate::bases_data::base);
    let merc = code.is_some_and(socket_bases::merc_wears);
    // A weapon of a type no build uses (user rule, 2026-11-20): sold, identified or not (no
    // point identifying it). The mercenary's polearms and spears keep their rule below;
    // javelins are a kind of spear in itemtypes.txt, but not one to keep for the merc.
    if let Some(b) = base
        && b.slot == 4
        && !(merc && b.kind != "jave")
        && !scoring.weapon_types.is_empty()
        && !scoring.weapon_types.contains(&b.kind)
    {
        return Err("not a build weapon");
    }
    // Unidentified, or stats unread (a real magic / rare item always has affix stats): it
    // can't be judged yet.
    if item.flags & item_data::FLAG_IDENTIFIED == 0 || item.stats.is_empty() {
        return Ok(());
    }
    // Boots without faster run/walk (user rule, 2026-11-20: "a must on the boots"), whatever
    // else they roll.
    if base.is_some_and(|b| b.slot == 9) && !item.stats.iter().any(|s| s.id == stat::FRW && s.value > 0) {
        return Err("no run speed");
    }
    let who = &scoring.profile;
    let piece = Piece {
        code: item.code.as_deref(),
        stats: &item.stats,
    };
    let (total, affixes) = gear_score::scores(piece, who);
    let no_stats = "no build stats";
    if matches!(item.kind, DropKind::Charm { .. }) || item.code.as_deref() == Some("jew") {
        return if total > 0.0 { Ok(()) } else { Err(no_stats) };
    }
    let worn: Vec<(u8, Piece<'_>)> = scoring
        .worn
        .iter()
        .map(|(body, code, stats)| {
            (
                *body,
                Piece {
                    code: code.as_deref(),
                    stats,
                },
            )
        })
        .collect();
    match gear_score::replaced_score(piece, &worn, who) {
        Some(old) if (total > 0.0 && total >= NEAR_WORN * old) || affixes >= NOTABLE => Ok(()),
        _ if merc => Ok(()),
        Some(_) if affixes > 0.0 => Err("worse than worn"),
        Some(_) => Err(no_stats),
        // Another class's item, or a shield beside a two-hander; no base known: keep.
        None => match item.code.as_deref().and_then(crate::bases_data::base) {
            Some(base) if base.class.is_some_and(|c| u32::from(c) != who.class) => Err("other class"),
            Some(_) if affixes >= NOTABLE => Ok(()),
            Some(_) => Err(no_stats),
            None => Ok(()),
        },
    }
}

/// One runeword the player's builds want made, in a base for `wearer` worn in `slot`
/// (`ItemData::BODY` numbering; `None` = any slot).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BaseNeed {
    word: &'static Runeword,
    wearer: Wearer,
    slot: Option<u8>,
}

/// What the player's own builds want ([`Needs::from_config`]).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Needs {
    /// Build pieces (unique, set and runeword names) with the copies still wanted.
    pieces: Vec<(String, u32)>,
    /// One white base each.
    bases: Vec<BaseNeed>,
}

/// A build file's piece without its `@<required level>`.
fn piece_name(entry: &str) -> &str {
    entry.split_once('@').map_or(entry, |(name, _)| name).trim()
}

/// The `ItemData::BODY` slot of a build slot name (both rings: the first), `None` if unknown.
fn slot_body(slot: &str) -> Option<u8> {
    match slot {
        "helm" => Some(1),
        "amulet" => Some(2),
        "body" => Some(3),
        "weapon" | "swap" => Some(4),
        "offhand" => Some(5),
        "ring1" | "ring2" => Some(6),
        "belt" => Some(8),
        "boots" => Some(9),
        "gloves" => Some(10),
        _ => None,
    }
}

impl Needs {
    /// Wants `count` copies of piece `entry` (at least as many as asked before).
    fn piece(&mut self, entry: &str, count: u32) {
        let name = piece_name(entry);
        if name.is_empty() {
            return;
        }
        match self.pieces.iter_mut().find(|(n, _)| same_name(n, name)) {
            Some((_, c)) => *c = (*c).max(count),
            None => self.pieces.push((name.to_owned(), count)),
        }
    }

    /// Wants a base for `entry` if it names a runeword.
    fn base(&mut self, entry: &str, wearer: Wearer, slot: Option<u8>) {
        let Some(word) = runewords_data::lookup(piece_name(entry)) else {
            return;
        };
        let need = BaseNeed { word, wearer, slot };
        if !self.bases.contains(&need) {
            self.bases.push(need);
        }
    }

    /// Every piece of build `slots` (slot name → pieces) and a base for each runeword in it.
    fn slots(&mut self, slots: &BTreeMap<String, Vec<String>>, wearer: Wearer) {
        for (slot, pieces) in slots {
            for entry in pieces {
                self.piece(entry, 1);
                self.base(entry, wearer, slot_body(slot));
            }
        }
    }

    /// The player's own needs: the resolved `bis` (the current stage, or the character's
    /// inline slots), every build of the stage `chain` (the current stage and the later ones,
    /// by name: [`Settings::stage_chain`]) with its slots, extras and `switch_requires`, and the
    /// whole farm list. A player piece is for class `class` (a farm group's: its build's class);
    /// a mercenary one for the Act 2 mercenary. Copies `worn` by the character or its
    /// mercenary need no stash copy, and each runeword copy `owns` counts (worn, carried,
    /// stashed, the mercenary's) drops one base need of that runeword.
    ///
    /// A `switch_requires` group with alternatives (any one of twenty Cannot Be Frozen items)
    /// wants each alternative as a piece but no base for its runewords: that would keep a base
    /// for every alternative runeword.
    #[must_use]
    pub fn from_config(
        config: &Settings,
        chain: &[String],
        bis: Option<&Bis>,
        class: u32,
        worn: &[String],
        owns: &dyn Fn(&str) -> u32,
    ) -> Self {
        let mut needs = Self::default();
        if let Some(bis) = bis {
            needs.slots(&bis.slots, Wearer::Player(class));
            needs.slots(&bis.merc_slots, Wearer::Act2Merc);
        }
        for build in chain
            .iter()
            .filter_map(|name| config.build_defs.get(&name.to_lowercase()))
        {
            let player = Wearer::Player(build.class.unwrap_or(class));
            needs.slots(&build.slots, player);
            needs.slots(&build.merc_slots, Wearer::Act2Merc);
            for entry in build.extras.values().flatten() {
                needs.piece(entry, 1);
                needs.base(entry, player, None);
            }
            for group in &build.switch_requires {
                for item in &group.any {
                    needs.piece(item, if group.any.len() == 1 { group.count } else { 1 });
                    if group.any.len() == 1 {
                        needs.base(item, player, None);
                    }
                }
            }
        }
        for group in &config.farm {
            let farm_class = config
                .build_defs
                .get(&group.target.to_lowercase())
                .and_then(|b| b.class)
                .unwrap_or(class);
            for item in &group.items {
                let wearer = if item.merc {
                    Wearer::Act2Merc
                } else {
                    Wearer::Player(farm_class)
                };
                needs.piece(&item.name, item.count);
                needs.base(&item.name, wearer, item.slot.map(crate::config::FarmSlot::body));
            }
        }
        for (name, count) in &mut needs.pieces {
            let on = worn.iter().filter(|w| same_name(w, name)).count();
            *count = count.saturating_sub(u32::try_from(on).unwrap_or(u32::MAX));
        }
        // A base is only the start until the runeword exists: each copy held (worn, carried,
        // stashed, the merc's) drops one base need of that runeword, in order.
        let mut held: Vec<(&str, u32)> = Vec::new();
        needs.bases.retain(|need| {
            let name = need.word.name;
            if !held.iter().any(|(n, _)| *n == name) {
                held.push((name, owns(name)));
            }
            match held.iter_mut().find(|(n, _)| *n == name) {
                Some((_, left)) if *left > 0 => {
                    *left -= 1;
                    false
                }
                _ => true,
            }
        });
        needs
    }
}

/// A white (low, normal or superior) item without a runeword whose code is a socketable
/// weapon or armor base.
fn plain_base(item: &ItemDrop) -> bool {
    crate::game::maybe_white_base(item.quality, item.flags)
        && item.code.as_deref().is_some_and(socket_bases::socketable)
}

/// What the valuables rules keep for trade: a certain S or A match, or an uncertain S (an
/// unidentified unique on a valuable base); an uncertain A (a magic grand charm whose stats
/// decide) is no keeper. A white base matching only a twink A rule (a plain Bill 4os for the
/// merc's Insight) is gear for the player's own characters, not worth trading (user rule,
/// 2026-11-20): kept only when it fills a base need ([`assign_bases`]), else sold. Twink A
/// items that aren't bases (Sigon's set, keep one) still go to trade.
fn trade_worthy(item: &ItemDrop) -> bool {
    let mut tiers = valuables::keep_tiers();
    if plain_base(item) {
        tiers.twink.retain(|&t| t == Tier::S);
    }
    valuables::best(item, &tiers).is_some_and(|h| !h.uncertain || h.tier == Tier::S)
}

/// How good a runeword base is, better is greater (user rule, 2026-11-20: a better base for the
/// same runeword replaces the kept one): the base damage (min + max, [`weapons_data::damage`])
/// of a weapon or the maximum base defense of an armor, helm or shield
/// ([`socket_bases::max_defense`]); then superior or ethereal over normal over low quality;
/// then the lower strength + dexterity requirement. A need open to both weapons and armor
/// (Fortitude without a slot) compares damage with defense: rare, and either base works.
fn base_rank(item: &ItemDrop) -> (u32, u8, Reverse<u32>) {
    let code = item.code.as_deref().unwrap_or_default();
    let raw = crate::weapons_data::damage(code)
        .or_else(|| socket_bases::max_defense(code).map(u32::from))
        .unwrap_or(0);
    // Ethereal bases have +50% base damage / defense in game, so an eth base competes with that.
    let power = if item.ethereal {
        raw.saturating_mul(3) / 2
    } else {
        raw
    };
    let grade = if item.quality == item_data::QUALITY_SUPERIOR || item.ethereal {
        2
    } else {
        u8::from(item.quality != item_data::QUALITY_LOW)
    };
    let req = crate::bases_data::base(code).map_or(0, |b| b.str.saturating_add(b.dex));
    (power, grade, Reverse(req))
}

/// Whether `base` can be made into the runeword of `need`: exactly its socket count, and a
/// base of its type the wearer can use in the slot ([`socket_bases::fits`]).
fn fills(need: &BaseNeed, base: &ItemDrop) -> bool {
    usize::from(base.sockets) == need.word.runes.len()
        && base
            .code
            .as_deref()
            .is_some_and(|code| socket_bases::fits(need.word, code, need.wearer, need.slot))
}

/// An item the plan leaves alone: gems and runes (their own tab), charms in the inventory
/// (they work there) and finished runewords that aren't a build piece.
fn left_alone(h: &Held) -> bool {
    match h.item.kind {
        DropKind::Gem(_) | DropKind::Rune(_) => true,
        DropKind::Charm { .. } => h.spot == Spot::Inventory,
        DropKind::Base | DropKind::Set | DropKind::Unique => h.item.flags & item_data::FLAG_RUNEWORD != 0,
    }
}

/// Keep for `side` where the item is, or move it there. Own items belong on pages 1–2 or in
/// the personal stash; one in the inventory or cube is in the wrong place too (user rule,
/// 2026-11-20: "if something is at the wrong place (inv, stash)": move it to 1–2).
const fn keep_or_move(side: Side, spot: Spot) -> Action {
    match (spot, spot.page_side(), side) {
        (Spot::Stash, _, Side::Own)
        | (_, Some(Side::Own), Side::Own)
        | (_, Some(Side::Trade), Side::Trade) => Action::Keep(side),
        _ => Action::Move(side),
    }
}

/// The indices of `items`, best place for `side` first (then in input order): for own items
/// pages 1–2, then the personal stash, the inventory and cube, the trade pages; for trade items
/// the trade pages, then pages 1–2, the personal stash, the cube and the inventory. Items on
/// the ignored page ([`IGNORED_PAGE`]) are left out: nothing about them is decided.
fn by_preference(items: &[Held], side: Side) -> Vec<usize> {
    let rank = |spot: Spot| match (side, spot.page_side(), spot) {
        (Side::Own, Some(Side::Own), _) | (Side::Trade, Some(Side::Trade), _) => 0,
        (Side::Own, None, Spot::Stash) | (Side::Trade, Some(Side::Own), _) => 1,
        (Side::Own, None, _) | (Side::Trade, None, Spot::Stash) => 2,
        (Side::Own, Some(Side::Trade), _) | (Side::Trade, None, Spot::Cube) => 3,
        (Side::Trade, None, _) => 4,
    };
    let mut order: Vec<usize> = (0..items.len())
        .filter(|&i| items.get(i).is_some_and(|h| h.spot != Spot::Page(IGNORED_PAGE)))
        .collect();
    order.sort_by_key(|&i| (items.get(i).map_or(u8::MAX, |h| rank(h.spot)), i));
    order
}

/// The item plan of `snap`'s character ([`held`], [`plan`]): needs from the resolved `bis`,
/// the stage `chain` ([`Settings::stage_chain`]) and the farm list, minus the copies the
/// character and its mercenary wear; magic / rare items scored for the chain's builds.
#[must_use]
pub fn for_snapshot(snap: &Snapshot, config: &Settings, chain: &[String], bis: Option<&Bis>) -> Vec<Advice> {
    let worn: Vec<String> = crate::game::worn_names(&snap.equipped_items)
        .into_iter()
        .chain(snap.merc_worn.iter().cloned())
        .collect();
    let holdings = snap.holdings();
    let owns = |name: &str| holdings.count(name);
    let needs = Needs::from_config(config, chain, bis, snap.player.class, &worn, &owns);
    plan(&held(snap), &needs, &Scoring::new(snap, config, chain))
}

/// An item's action with the reason of a scored sell.
type Decided = (Action, Option<&'static str>);

/// One base per runeword need, the needs with the fewest fitting bases first (a base that fits
/// two needs must not starve the narrower one), each taking its best free base ([`base_rank`];
/// on a tie the best placed, `order`, [`by_preference`]). The other bases are sold by [`plan`],
/// a worse one already kept on pages 1–2 included. A trade-worthy base is left for the trade
/// side.
fn assign_bases(items: &[Held], needs: &Needs, order: &[usize], actions: &mut [Option<Decided>]) {
    let candidates: Vec<usize> = order
        .iter()
        .copied()
        .filter(|&i| {
            items
                .get(i)
                .is_some_and(|h| plain_base(&h.item) && !trade_worthy(&h.item))
                && actions.get(i).is_some_and(Option::is_none)
        })
        .collect();
    let mut by_choice: Vec<Vec<usize>> = needs
        .bases
        .iter()
        .map(|need| {
            candidates
                .iter()
                .copied()
                .filter(|&i| items.get(i).is_some_and(|h| fills(need, &h.item)))
                .collect()
        })
        .collect();
    by_choice.sort_by_key(Vec::len);
    for fits in by_choice {
        // `min_by_key` keeps the first of equal keys: the best-placed of equal bases.
        let free = fits
            .into_iter()
            .filter(|&i| actions.get(i).is_some_and(Option::is_none))
            .min_by_key(|&i| Reverse(items.get(i).map(|h| base_rank(&h.item))));
        if let Some((h, slot)) = free.and_then(|i| items.get(i).zip(actions.get_mut(i))) {
            *slot = Some((keep_or_move(Side::Own, h.spot), None));
        }
    }
}

/// The action for each of `items` (see the module docs), merged into dashboard lines and
/// sorted by action, then name, then spot.
#[must_use]
pub fn plan(items: &[Held], needs: &Needs, scoring: &Scoring) -> Vec<Advice> {
    let mut actions: Vec<Option<Decided>> = vec![None; items.len()];
    let own_order = by_preference(items, Side::Own);
    // Build pieces first: they win over trade (the player needs them). A keep-one item kept
    // here counts as its one copy.
    let mut kept_one: Vec<&str> = Vec::new();
    let mut wanted: Vec<u32> = needs.pieces.iter().map(|&(_, c)| c).collect();
    for &i in &own_order {
        let (Some(h), Some(slot)) = (items.get(i), actions.get_mut(i)) else {
            continue;
        };
        let named =
            valuables::unique_set_name(&h.item).is_some() || h.item.flags & item_data::FLAG_RUNEWORD != 0;
        let left = needs
            .pieces
            .iter()
            .position(|(n, _)| same_name(n, &h.name))
            .and_then(|p| wanted.get_mut(p));
        if let (true, Some(left @ 1..)) = (named, left) {
            *left -= 1;
            *slot = Some((keep_or_move(Side::Own, h.spot), None));
            kept_one.extend(valuables::keep_one_name(&h.item));
        }
    }
    assign_bases(items, needs, &own_order, &mut actions);
    // The rest: trade, sell or give away. A keep-one item keeps its copy best placed for trade.
    for i in by_preference(items, Side::Trade) {
        let (Some(h), Some(slot)) = (items.get(i), actions.get_mut(i)) else {
            continue;
        };
        if slot.is_some() || left_alone(h) {
            continue;
        }
        let action = if trade_worthy(&h.item) {
            match valuables::keep_one_name(&h.item) {
                Some(name) if kept_one.contains(&name) => Some(Action::GiveAway),
                one => {
                    kept_one.extend(one);
                    Some(keep_or_move(Side::Trade, h.spot))
                }
            }
        } else if plain_base(&h.item) {
            Some(Action::Sell)
        } else if crate::game::magic_or_better(h.item.quality) {
            *slot = Some(match judge(&h.item, scoring) {
                Ok(()) => (keep_or_move(Side::Own, h.spot), None),
                Err(reason) => (Action::Sell, Some(reason)),
            });
            continue;
        } else {
            valuables::verdict(&h.item).map(|v| match v {
                Verdict::Sell => Action::Sell,
                Verdict::GiveAway => Action::GiveAway,
            })
        };
        *slot = action.map(|a| (a, None));
    }
    let mut out: Vec<Advice> = Vec::new();
    for (h, action) in items.iter().zip(actions) {
        let Some((action, reason)) = action else { continue };
        match out
            .iter_mut()
            .find(|a| a.action == action && a.spot == h.spot && a.name == h.name && a.reason == reason)
        {
            Some(a) => a.count = a.count.saturating_add(1),
            None => out.push(Advice {
                name: h.name.clone(),
                spot: h.spot,
                action,
                reason,
                count: 1,
            }),
        }
    }
    out.sort_by(|a, b| {
        a.action
            .cmp(&b.action)
            .then_with(|| a.name.cmp(&b.name))
            .then(a.spot.cmp(&b.spot))
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    const DRUID: Wearer = Wearer::Player(5);

    /// A white item of `code` with `sockets`, not ethereal.
    fn base(code: &str, sockets: u8) -> ItemDrop {
        ItemDrop {
            id: 0,
            code: Some(code.into()),
            kind: DropKind::Base,
            quality: item_data::QUALITY_NORMAL,
            flags: item_data::FLAG_IDENTIFIED,
            ethereal: false,
            sockets,
            unique_index: None,
            stats: Vec::new(),
            item_data_head: String::new(),
            x: 0,
            y: 0,
        }
    }

    /// An identified unique (`quality` 7) or set (5) item of `code` and table row `index`.
    fn named(code: &str, quality: u32, index: u32) -> ItemDrop {
        ItemDrop {
            kind: if quality == item_data::QUALITY_SET {
                DropKind::Set
            } else {
                DropKind::Unique
            },
            quality,
            unique_index: Some(index),
            ..base(code, 0)
        }
    }

    fn at(item: ItemDrop, spot: Spot) -> Held {
        let name = label(&item, valuables::display_name(&item));
        Held { item, spot, name }
    }

    /// The plan as `(name, spot, action, count)` rows.
    fn rows(items: &[Held], needs: &Needs) -> Vec<(String, Spot, Action, u32)> {
        plan(items, needs, &druid())
            .into_iter()
            .map(|a| (a.name, a.spot, a.action, a.count))
            .collect()
    }

    fn row(name: &str, spot: Spot, action: Action, count: u32) -> (String, Spot, Action, u32) {
        (name.to_owned(), spot, action, count)
    }

    /// Harmony (4 sockets, missile weapons) for the druid's weapon slot.
    fn harmony_needs() -> Needs {
        let mut needs = Needs::default();
        needs.piece("Harmony@39", 1);
        needs.base("Harmony@39", DRUID, Some(4));
        needs
    }

    #[test]
    fn two_identical_own_bases_keep_one_and_sell_the_other() {
        let bow = || base("hbw", 4);
        let items = [at(bow(), Spot::Page(4)), at(bow(), Spot::Page(1))];
        assert_eq!(
            rows(&items, &harmony_needs()),
            vec![
                row("Hunter's Bow 4os", Spot::Page(4), Action::Sell, 1),
                row("Hunter's Bow 4os", Spot::Page(1), Action::Keep(Side::Own), 1),
            ],
            "the copy already on page 1 is the one kept"
        );
        // Only on a trade page: kept, but moved to 1–2.
        assert_eq!(
            rows(&[at(bow(), Spot::Page(4))], &harmony_needs()),
            vec![row("Hunter's Bow 4os", Spot::Page(4), Action::Move(Side::Own), 1)]
        );
        // Wrong socket count, or not a missile weapon: not the Harmony base.
        assert_eq!(
            rows(&[at(base("hbw", 3), Spot::Page(1))], &harmony_needs()),
            vec![row("Hunter's Bow 3os", Spot::Page(1), Action::Sell, 1)]
        );
    }

    #[test]
    fn a_trade_base_on_page_1_moves_to_the_trade_pages() {
        // eth 4os Colossus Voulge: an S twink base, worth trading even though the merc could
        // use it. Crystal Sword 4os is only A twink: one copy is the Spirit base, the other
        // isn't worth trading and is sold.
        let mut voulge = base("7vo", 4);
        voulge.ethereal = true;
        let mut needs = Needs::default();
        needs.base("Spirit", DRUID, Some(4));
        let items = [
            at(voulge, Spot::Page(1)),
            at(base("crs", 4), Spot::Page(2)),
            at(base("crs", 4), Spot::Page(3)),
        ];
        assert_eq!(
            rows(&items, &needs),
            vec![
                row("Crystal Sword 4os", Spot::Page(3), Action::Sell, 1),
                row(
                    "eth Colossus Voulge 4os",
                    Spot::Page(1),
                    Action::Move(Side::Trade),
                    1
                ),
                row("Crystal Sword 4os", Spot::Page(2), Action::Keep(Side::Own), 1),
            ]
        );
    }

    /// Insight wanted for the merc's weapon (farm list), `held` copies already owned.
    fn insight_needs(held: u32) -> Needs {
        use crate::config::{FarmGroup, FarmItem, FarmSlot};
        let config = Settings {
            farm: vec![FarmGroup {
                target: "Fury".to_owned(),
                items: vec![FarmItem {
                    name: "Insight".to_owned(),
                    kind: crate::names::ItemKind::Runeword,
                    count: 1,
                    merc: true,
                    slot: Some(FarmSlot::Weapon),
                }],
            }],
            ..Settings::default()
        };
        Needs::from_config(&config, &[], None, 5, &[], &move |n| {
            u32::from(n == "Insight") * held
        })
    }

    #[test]
    fn a_twink_a_base_is_no_trade_good() {
        // Bill / Poleaxe 4os match "Polearms 4os (any tier)", A twink: gear for the player's
        // own characters. With Insight held they are sold, not moved to the trade pages.
        let items = [
            at(base("9vo", 4), Spot::Page(1)),
            at(base("pax", 4), Spot::Page(4)),
        ];
        assert_eq!(
            rows(&items, &insight_needs(1)),
            vec![
                row("Bill 4os", Spot::Page(1), Action::Sell, 1),
                row("Poleaxe 4os", Spot::Page(4), Action::Sell, 1),
            ]
        );
        // Insight still wanted: the better base (Bill 14–53 over Poleaxe 18–39) is kept, the other sold.
        assert_eq!(
            rows(&items, &insight_needs(0)),
            vec![
                row("Poleaxe 4os", Spot::Page(4), Action::Sell, 1),
                row("Bill 4os", Spot::Page(1), Action::Keep(Side::Own), 1),
            ]
        );
        // An eth elite 4os polearm is S twink: a trade good, need or not.
        let mut voulge = base("7vo", 4);
        voulge.ethereal = true;
        for needs in [insight_needs(0), insight_needs(1)] {
            assert_eq!(
                rows(&[at(voulge.clone(), Spot::Inventory)], &needs),
                vec![row(
                    "eth Colossus Voulge 4os",
                    Spot::Inventory,
                    Action::Move(Side::Trade),
                    1
                )]
            );
        }
    }

    #[test]
    fn tradable_a_bases_are_kept_for_trade_without_a_need() {
        // Monarch 4os (Spirit shield) and Crystal Sword 5os (Call to Arms): A trade rules.
        let items = [
            at(base("uit", 4), Spot::Page(4)),
            at(base("crs", 5), Spot::Inventory),
        ];
        assert_eq!(
            rows(&items, &Needs::default()),
            vec![
                row("Crystal Sword 5os", Spot::Inventory, Action::Move(Side::Trade), 1),
                row("Monarch 4os", Spot::Page(4), Action::Keep(Side::Trade), 1),
            ]
        );
    }

    #[test]
    fn the_better_base_for_a_runeword_wins() {
        // Spirit: Broad Sword (7–14) beats Crystal Sword (5–15) on average damage, even
        // from a trade page against a sword already kept on page 1.
        let mut spirit = Needs::default();
        spirit.base("Spirit", DRUID, Some(4));
        let swords = [
            at(base("crs", 4), Spot::Page(1)),
            at(base("bsd", 4), Spot::Page(4)),
        ];
        assert_eq!(
            rows(&swords, &spirit),
            vec![
                row("Crystal Sword 4os", Spot::Page(1), Action::Sell, 1),
                row("Broad Sword 4os", Spot::Page(4), Action::Move(Side::Own), 1),
            ]
        );
        // Fortitude in the body slot: Ancient Armor (233 def) beats Full Plate Mail (161).
        let mut fortitude = Needs::default();
        fortitude.base("Fortitude", DRUID, Some(3));
        let armors = [
            at(base("ful", 4), Spot::Page(2)),
            at(base("aar", 4), Spot::Inventory),
        ];
        assert_eq!(
            rows(&armors, &fortitude),
            vec![
                row("Full Plate Mail 4os", Spot::Page(2), Action::Sell, 1),
                row("Ancient Armor 4os", Spot::Inventory, Action::Move(Side::Own), 1),
            ]
        );
        // The same base: superior beats plain.
        let mut superior = base("ful", 4);
        superior.quality = item_data::QUALITY_SUPERIOR;
        let plates = [at(base("ful", 4), Spot::Page(1)), at(superior, Spot::Page(4))];
        let plates_plan = plan(&plates, &fortitude, &druid());
        let actions: Vec<_> = plates_plan.iter().map(|a| (a.spot, a.action)).collect();
        assert_eq!(
            actions,
            vec![
                (Spot::Page(1), Action::Sell),
                (Spot::Page(4), Action::Move(Side::Own)),
            ]
        );
        // Ethereal counts +50%: an eth Full Plate Mail (161 × 1.5 = 241) beats Ancient Armor (233).
        let mut eth = base("ful", 4);
        eth.ethereal = true;
        let mixed = [at(base("aar", 4), Spot::Page(1)), at(eth, Spot::Page(2))];
        let eth_plan = plan(&mixed, &fortitude, &druid());
        let actions: Vec<_> = eth_plan.iter().map(|a| (a.spot, a.action)).collect();
        assert_eq!(
            actions,
            vec![
                (Spot::Page(1), Action::Sell),
                (Spot::Page(2), Action::Keep(Side::Own)),
            ]
        );
    }

    #[test]
    fn a_junk_base_on_a_trade_page_is_sold() {
        let items = [
            at(base("crs", 2), Spot::Page(4)),
            at(base("hbw", 0), Spot::Page(3)),
            at(base("hbw", 0), Spot::Page(3)),
        ];
        assert_eq!(
            rows(&items, &harmony_needs()),
            vec![
                row("Crystal Sword 2os", Spot::Page(4), Action::Sell, 1),
                row("Hunter's Bow", Spot::Page(3), Action::Sell, 2),
            ]
        );
    }

    #[test]
    fn a_build_piece_in_the_inventory_moves_to_the_own_pages() {
        // Gravenspine: no valuables rule (sold when nobody needs it).
        let grave = || named("hax", 7, 12);
        let mut needs = Needs::default();
        assert_eq!(
            rows(&[at(grave(), Spot::Inventory)], &needs),
            vec![row("Gravenspine", Spot::Inventory, Action::Sell, 1)]
        );
        needs.piece("Gravenspine@31", 1);
        // Kept, but the inventory (or cube) is the wrong place: moved to 1–2. The personal
        // stash is as good as pages 1–2.
        assert_eq!(
            rows(&[at(grave(), Spot::Inventory)], &needs),
            vec![row("Gravenspine", Spot::Inventory, Action::Move(Side::Own), 1)]
        );
        assert_eq!(
            rows(&[at(grave(), Spot::Cube)], &needs),
            vec![row("Gravenspine", Spot::Cube, Action::Move(Side::Own), 1)]
        );
        assert_eq!(
            rows(&[at(grave(), Spot::Stash)], &needs),
            vec![row("Gravenspine", Spot::Stash, Action::Keep(Side::Own), 1)]
        );
        // One copy wanted: the one on page 2 is it; the inventory and page-4 copies are spare.
        let items = [
            at(grave(), Spot::Inventory),
            at(grave(), Spot::Page(4)),
            at(grave(), Spot::Page(2)),
        ];
        assert_eq!(
            rows(&items, &needs),
            vec![
                row("Gravenspine", Spot::Inventory, Action::Sell, 1),
                row("Gravenspine", Spot::Page(4), Action::Sell, 1),
                row("Gravenspine", Spot::Page(2), Action::Keep(Side::Own), 1),
            ]
        );
        // Only on a trade page: moved to 1–2.
        assert_eq!(
            rows(&[at(grave(), Spot::Page(3))], &needs),
            vec![row("Gravenspine", Spot::Page(3), Action::Move(Side::Own), 1)]
        );
    }

    #[test]
    fn valuable_uniques_and_sets_are_kept_for_trade() {
        // Harlequin Crest (S); Sigon's Visor (A twink, keep one: the second copy is a give-away);
        // Magefist (B only): give away.
        let visor = || named("ghm", 5, 36);
        let items = [
            at(named("uap", 7, 248), Spot::Inventory),
            at(visor(), Spot::Page(1)),
            at(visor(), Spot::Page(3)),
            at(named("tgl", 7, 105), Spot::Stash),
        ];
        assert_eq!(
            rows(&items, &Needs::default()),
            vec![
                row("Harlequin Crest", Spot::Inventory, Action::Move(Side::Trade), 1),
                row("Sigon's Visor", Spot::Page(3), Action::Keep(Side::Trade), 1),
                row("Magefist", Spot::Stash, Action::GiveAway, 1),
                row("Sigon's Visor", Spot::Page(1), Action::GiveAway, 1),
            ]
        );
    }

    /// A druid with nothing worn and no builds (every weapon type judged).
    fn druid() -> Scoring {
        Scoring {
            profile: Profile {
                class: 5,
                ..Profile::default()
            },
            ..Scoring::default()
        }
    }

    /// An identified item of `code` and `quality` with `stats` (id, param, value).
    fn rolled(code: &str, quality: u32, stats: &[(u16, u16, i32)]) -> ItemDrop {
        ItemDrop {
            quality,
            stats: stats
                .iter()
                .map(|&(id, param, value)| ItemStat {
                    id,
                    param,
                    value,
                    base: false,
                })
                .collect(),
            ..base(code, 0)
        }
    }

    /// The plan of `items` as `(name, spot, action, reason)` rows.
    fn judged(items: &[Held], scoring: &Scoring) -> Vec<(String, Spot, Action, Option<&'static str>)> {
        plan(items, &Needs::default(), scoring)
            .into_iter()
            .map(|a| (a.name, a.spot, a.action, a.reason))
            .collect()
    }

    use item_data::{QUALITY_MAGIC, QUALITY_RARE};

    /// Light radius: a stat no build weighs.
    const LIGHT_RADIUS: u16 = 89;

    #[test]
    fn a_rare_ring_with_useless_stats_is_sold_with_a_reason() {
        let ring = rolled("rin", QUALITY_RARE, &[(LIGHT_RADIUS, 0, 2)]);
        assert_eq!(
            judged(&[at(ring, Spot::Page(4))], &druid()),
            vec![(
                "Rare Ring".to_owned(),
                Spot::Page(4),
                Action::Sell,
                Some("no build stats")
            )]
        );
    }

    #[test]
    fn a_rare_with_druid_skills_is_kept_and_moved_to_the_own_pages() {
        let ring = rolled("rin", QUALITY_RARE, &[(stat::CLASS_SKILLS, 5, 2)]);
        assert_eq!(
            judged(&[at(ring.clone(), Spot::Page(4))], &druid()),
            vec![(
                "Rare Ring".to_owned(),
                Spot::Page(4),
                Action::Move(Side::Own),
                None
            )]
        );
        // Another class's skills are worth nothing to the druid.
        let amazon = rolled("rin", QUALITY_RARE, &[(stat::CLASS_SKILLS, 0, 2)]);
        assert_eq!(
            judged(&[at(amazon, Spot::Page(1))], &druid()),
            vec![(
                "Rare Ring".to_owned(),
                Spot::Page(1),
                Action::Sell,
                Some("no build stats")
            )]
        );
        // Notable stats stay even when a better ring is on each finger.
        let mut scoring = druid();
        let great = vec![ItemStat {
            id: stat::CLASS_SKILLS,
            param: 5,
            value: 4,
            base: false,
        }];
        scoring.worn = vec![
            (6, Some("rin".to_owned()), great.clone()),
            (7, Some("rin".to_owned()), great),
        ];
        assert_eq!(
            judged(&[at(ring, Spot::Inventory)], &scoring),
            vec![(
                "Rare Ring".to_owned(),
                Spot::Inventory,
                Action::Move(Side::Own),
                None
            )]
        );
    }

    #[test]
    fn a_weak_item_against_a_better_worn_one_is_sold() {
        let mut scoring = druid();
        let boots = |res| {
            vec![ItemStat {
                id: stat::RES_FIRE,
                param: 0,
                value: res,
                base: false,
            }]
        };
        scoring.worn = vec![(9, Some("lbt".to_owned()), boots(30))];
        let weak = rolled(
            "lbt",
            QUALITY_MAGIC,
            &[(stat::FRW, 0, 10), (stat::RES_FIRE, 0, 10)],
        );
        let near = rolled(
            "lbt",
            QUALITY_MAGIC,
            &[(stat::FRW, 0, 10), (stat::RES_FIRE, 0, 25)],
        );
        assert_eq!(
            judged(&[at(weak, Spot::Page(1)), at(near, Spot::Page(2))], &scoring),
            vec![
                (
                    "Magic Boots".to_owned(),
                    Spot::Page(1),
                    Action::Sell,
                    Some("worse than worn")
                ),
                (
                    "Magic Boots".to_owned(),
                    Spot::Page(2),
                    Action::Keep(Side::Own),
                    None
                ),
            ],
            "25 of 30 is within 80%"
        );
    }

    #[test]
    fn a_magic_item_with_build_stats_in_the_inventory_is_kept_on_the_own_pages() {
        let amulet = rolled("amu", QUALITY_MAGIC, &[(stat::RES_FIRE, 0, 15)]);
        assert_eq!(
            judged(&[at(amulet, Spot::Inventory)], &druid()),
            vec![(
                "Magic Amulet".to_owned(),
                Spot::Inventory,
                Action::Move(Side::Own),
                None
            )],
            "nothing worn: any score beats an empty slot"
        );
    }

    #[test]
    fn an_unidentified_rare_is_kept() {
        let mut ring = rolled("rin", QUALITY_RARE, &[(LIGHT_RADIUS, 0, 2)]);
        ring.flags = 0;
        assert_eq!(
            judged(&[at(ring, Spot::Page(4))], &druid()),
            vec![(
                "Rare Ring".to_owned(),
                Spot::Page(4),
                Action::Move(Side::Own),
                None
            )]
        );
    }

    #[test]
    fn charms_and_jewels_are_kept_when_they_score_at_all() {
        let mut charm = rolled("cm1", QUALITY_MAGIC, &[(stat::RES_FIRE, 0, 8)]);
        charm.kind = DropKind::Charm {
            size: crate::items::CharmSize::Small,
            unique: false,
        };
        let jewel = rolled("jew", QUALITY_MAGIC, &[(LIGHT_RADIUS, 0, 1)]);
        assert_eq!(
            judged(
                &[
                    at(charm.clone(), Spot::Stash),
                    at(charm, Spot::Inventory),
                    at(jewel, Spot::Page(2)),
                ],
                &druid()
            ),
            vec![
                (
                    "Magic Jewel".to_owned(),
                    Spot::Page(2),
                    Action::Sell,
                    Some("no build stats")
                ),
                (
                    "Magic Small Charm".to_owned(),
                    Spot::Stash,
                    Action::Keep(Side::Own),
                    None
                ),
            ],
            "the carried charm stays out"
        );
    }

    #[test]
    fn merc_bases_failing_the_player_are_kept_unscored() {
        let helm = rolled("cap", QUALITY_RARE, &[(LIGHT_RADIUS, 0, 1)]);
        assert_eq!(
            judged(&[at(helm, Spot::Page(1))], &druid()),
            vec![(
                "Rare Cap".to_owned(),
                Spot::Page(1),
                Action::Keep(Side::Own),
                None
            )]
        );
    }

    #[test]
    fn gems_runes_carried_charms_and_other_items_are_left_alone() {
        let mut vex = base("r26", 0);
        vex.kind = DropKind::Rune(26);
        let mut gem = base("gpv", 0);
        gem.kind = DropKind::Gem(crate::items::GemColour::Amethyst);
        let mut torch = named("cm2", 7, 381);
        torch.kind = DropKind::Charm {
            size: crate::items::CharmSize::Large,
            unique: true,
        };
        let mut potion = base("hp1", 0);
        potion.kind = DropKind::Base;
        let mut spirit = base("crs", 4);
        spirit.flags |= item_data::FLAG_RUNEWORD;
        let items = [
            at(vex.clone(), Spot::Page(1)),
            at(vex, Spot::Inventory),
            at(gem, Spot::Page(4)),
            at(torch, Spot::Inventory),
            at(potion, Spot::Inventory),
            at(spirit, Spot::Page(4)),
        ];
        assert_eq!(rows(&items, &harmony_needs()), vec![]);
    }

    #[test]
    fn the_materials_tab_and_live_shared_items_are_not_judged() {
        use crate::stash::StashItem;
        let item = |name: &str, code: &str, quality: u8, sockets| StashItem {
            name: name.to_owned(),
            code: Some(code.to_owned()),
            quality,
            identified: true,
            ethereal: false,
            runeword: false,
            quantity: None,
            sockets,
            unique_index: None,
            stats: Vec::new(),
        };
        let tabs = vec![
            StashTab {
                kind: TabKind::Page(4),
                items: vec![item("Hunter's Bow 4os", "hbw", 2, 4)],
            },
            StashTab {
                kind: TabKind::Materials,
                items: vec![item("Crystal Sword 2os", "crs", 2, 2)],
            },
        ];
        let owned = vec![(base("crs", 2), Place::Shared), (base("crs", 3), Place::Cube)];
        let held = held_from(&owned, &tabs);
        let spots: Vec<_> = held.iter().map(|h| (h.name.as_str(), h.spot)).collect();
        assert_eq!(
            spots,
            vec![
                ("Crystal Sword 3os", Spot::Cube),
                ("Hunter's Bow 4os", Spot::Page(4))
            ]
        );
        assert_eq!(
            rows(&held, &harmony_needs()),
            vec![
                row("Crystal Sword 3os", Spot::Cube, Action::Sell, 1),
                row("Hunter's Bow 4os", Spot::Page(4), Action::Move(Side::Own), 1),
            ]
        );
    }

    #[test]
    fn needs_come_from_the_bis_chain_farm_list_and_worn_copies() {
        use crate::config::{Build, FarmGroup, FarmItem, FarmSlot};
        use crate::playstyle::config::SwitchGroup;
        let slots = |pairs: &[(&str, &[&str])]| -> BTreeMap<String, Vec<String>> {
            pairs
                .iter()
                .map(|(k, v)| ((*k).to_owned(), v.iter().map(|&s| s.to_owned()).collect()))
                .collect()
        };
        let mut config = Settings::default();
        config.build_defs.insert(
            "fury".to_owned(),
            Build {
                name: "Fury".to_owned(),
                class: Some(5),
                slots: slots(&[("swap", &["Harmony@39"]), ("helm", &["Jalal's Mane@42"])]),
                merc_slots: slots(&[("helm", &["Bulwark@35"])]),
                switch_requires: vec![
                    SwitchGroup {
                        label: "CBF".to_owned(),
                        any: vec!["Raven Frost".to_owned(), "Wisdom".to_owned()],
                        count: 1,
                    },
                    SwitchGroup {
                        label: "PI".to_owned(),
                        any: vec!["Lawbringer".to_owned()],
                        count: 2,
                    },
                ],
                ..Build::default()
            },
        );
        config.farm = vec![FarmGroup {
            target: "Fury".to_owned(),
            items: vec![FarmItem {
                name: "Insight".to_owned(),
                kind: crate::names::ItemKind::Runeword,
                count: 1,
                merc: true,
                slot: Some(FarmSlot::Weapon),
            }],
        }];
        let needs = Needs::from_config(
            &config,
            &["Fury".to_owned()],
            None,
            0,
            &["Jalal's Mane".to_owned()],
            &|_| 0,
        );
        let pieces: Vec<_> = needs.pieces.iter().map(|(n, c)| (n.as_str(), *c)).collect();
        assert_eq!(
            pieces,
            vec![
                ("Jalal's Mane", 0),
                ("Harmony", 1),
                ("Bulwark", 1),
                ("Raven Frost", 1),
                ("Wisdom", 1),
                ("Lawbringer", 2),
                ("Insight", 1),
            ],
            "the worn Jalal's needs no stash copy"
        );
        let bases: Vec<_> = needs
            .bases
            .iter()
            .map(|b| (b.word.name, b.wearer, b.slot))
            .collect();
        assert_eq!(
            bases,
            vec![
                ("Harmony", DRUID, Some(4)),
                ("Bulwark", Wearer::Act2Merc, Some(1)),
                ("Lawbringer", DRUID, None),
                ("Insight", Wearer::Act2Merc, Some(4)),
            ],
            "no base for Wisdom, one alternative of several; the druid class from the build"
        );
    }

    #[test]
    fn an_identified_rare_with_unread_stats_is_kept() {
        let ring = rolled("rin", QUALITY_RARE, &[]);
        assert_eq!(
            judged(&[at(ring, Spot::Page(1))], &druid()),
            vec![(
                "Rare Ring".to_owned(),
                Spot::Page(1),
                Action::Keep(Side::Own),
                None
            )],
            "empty stats are a failed read, not a junk item"
        );
    }

    #[test]
    fn a_held_runeword_needs_no_base_any_more() {
        use crate::config::{FarmGroup, FarmItem, FarmSlot};
        let config = Settings {
            farm: vec![FarmGroup {
                target: "Fury".to_owned(),
                items: vec![FarmItem {
                    name: "Harmony".to_owned(),
                    kind: crate::names::ItemKind::Runeword,
                    count: 1,
                    merc: false,
                    slot: Some(FarmSlot::Weapon),
                }],
            }],
            ..Settings::default()
        };
        let needs = |held: u32| {
            Needs::from_config(&config, &[], None, 5, &[], &move |n| {
                u32::from(n == "Harmony") * held
            })
        };
        assert_eq!(needs(0).bases.len(), 1);
        assert_eq!(needs(1).bases.len(), 0, "Harmony exists: no base kept for it");
        let bow = [at(base("hbw", 4), Spot::Page(1))];
        assert_eq!(
            rows(&bow, &needs(0)),
            vec![row("Hunter's Bow 4os", Spot::Page(1), Action::Keep(Side::Own), 1)]
        );
        assert_eq!(
            rows(&bow, &needs(1)),
            vec![row("Hunter's Bow 4os", Spot::Page(1), Action::Sell, 1)]
        );
    }

    #[test]
    fn a_keep_one_piece_kept_for_own_use_is_its_one_copy() {
        // Sigon's Visor (A twink, keep one) as a build piece: the page-1 copy is the player's,
        // the page-3 copy a give-away (not a second trade keeper).
        let visor = || named("ghm", 5, 36);
        let mut needs = Needs::default();
        needs.piece("Sigon's Visor@6", 1);
        let items = [at(visor(), Spot::Page(3)), at(visor(), Spot::Page(1))];
        assert_eq!(
            rows(&items, &needs),
            vec![
                row("Sigon's Visor", Spot::Page(1), Action::Keep(Side::Own), 1),
                row("Sigon's Visor", Spot::Page(3), Action::GiveAway, 1),
            ]
        );
    }

    #[test]
    fn the_narrowest_base_need_picks_first() {
        // Crescent Moon takes axes or swords, Lawbringer swords only (3 sockets each). The sword
        // on page 1 is the better-placed base for both; Lawbringer, with one choice, gets it.
        let mut needs = Needs::default();
        needs.base("Crescent Moon", DRUID, None);
        needs.base("Lawbringer", DRUID, None);
        let items = [
            at(base("crs", 3), Spot::Page(1)),
            at(base("lax", 3), Spot::Page(2)),
        ];
        assert_eq!(
            rows(&items, &needs),
            vec![
                row("Crystal Sword 3os", Spot::Page(1), Action::Keep(Side::Own), 1),
                row("Large Axe 3os", Spot::Page(2), Action::Keep(Side::Own), 1),
            ]
        );
    }

    #[test]
    fn page_5_is_ignored() {
        // A junk base there gets no entry; a trade item on page 1 moves to 3–4, not 5.
        let mut voulge = base("7vo", 4);
        voulge.ethereal = true;
        let items = [
            at(base("crs", 2), Spot::Page(5)),
            at(named("uap", 7, 248), Spot::Page(5)),
            at(voulge, Spot::Page(1)),
        ];
        let advice = plan(&items, &Needs::default(), &druid());
        assert_eq!(
            advice
                .iter()
                .map(|a| (a.name.as_str(), a.spot, a.action))
                .collect::<Vec<_>>(),
            vec![(
                "eth Colossus Voulge 4os",
                Spot::Page(1),
                Action::Move(Side::Trade)
            )]
        );
        assert_eq!(Side::Trade.pages(), "3–4");
        // A needed build piece only on page 5 is neither kept nor moved from there.
        let mut needs = Needs::default();
        needs.piece("Gravenspine@31", 1);
        assert_eq!(rows(&[at(named("hax", 7, 12), Spot::Page(5))], &needs), vec![]);
    }

    #[test]
    fn a_runeword_on_page_5_still_satisfies_its_base_need() {
        use crate::config::{FarmGroup, FarmItem, FarmSlot};
        use crate::stash::StashItem;
        let config = Settings {
            farm: vec![FarmGroup {
                target: "Fury".to_owned(),
                items: vec![FarmItem {
                    name: "Harmony".to_owned(),
                    kind: crate::names::ItemKind::Runeword,
                    count: 1,
                    merc: false,
                    slot: Some(FarmSlot::Weapon),
                }],
            }],
            ..Settings::default()
        };
        // Harmony in a Hunter's Bow on page 5: counted by the holdings like any stash item.
        let stash = [StashTab {
            kind: TabKind::Page(5),
            items: vec![StashItem {
                name: "Harmony".to_owned(),
                code: Some("hbw".to_owned()),
                quality: 2,
                identified: true,
                ethereal: false,
                runeword: true,
                quantity: None,
                sockets: 4,
                unique_index: None,
                stats: Vec::new(),
            }],
        }];
        let holdings = crate::stash::Holdings {
            stash: &stash,
            owned: &[],
            runeword_copies: &[],
            merc_worn: &[],
        };
        let needs = Needs::from_config(&config, &[], None, 5, &[], &|n| holdings.count(n));
        assert_eq!(needs.bases.len(), 0, "Harmony on page 5 is held: no base wanted");
        assert_eq!(
            rows(&[at(base("hbw", 4), Spot::Page(1))], &needs),
            vec![row("Hunter's Bow 4os", Spot::Page(1), Action::Sell, 1)]
        );
    }

    /// The druid scored with the weapon types of a summoner whose weapon is Spirit (swords).
    fn summoner() -> Scoring {
        let build = Build {
            class: Some(5),
            slots: BTreeMap::from([("weapon".to_owned(), vec!["Spirit@25".to_owned()])]),
            ..Build::default()
        };
        Scoring {
            weapon_types: build_weapon_types([&build].into_iter(), 5),
            ..druid()
        }
    }

    #[test]
    fn a_weapon_of_a_type_no_build_uses_is_sold() {
        let scoring = summoner();
        assert_eq!(scoring.weapon_types, ["swor"], "Spirit: swords (4 sockets) only");
        let javelin = rolled("jav", QUALITY_RARE, &[(stat::CLASS_SKILLS, 5, 2)]);
        let mut unid = javelin.clone();
        unid.flags = 0;
        let sword = rolled("crs", QUALITY_RARE, &[(stat::CLASS_SKILLS, 5, 2)]);
        let junk_sword = rolled("crs", QUALITY_RARE, &[(LIGHT_RADIUS, 0, 2)]);
        let spear = rolled("spr", QUALITY_RARE, &[(LIGHT_RADIUS, 0, 2)]);
        let reasons: Vec<_> = [javelin, unid, sword, junk_sword, spear]
            .iter()
            .map(|item| judge(item, &scoring))
            .collect();
        assert_eq!(
            reasons,
            vec![
                Err("not a build weapon"),
                Err("not a build weapon"),
                Ok(()),
                Err("no build stats"),
                Ok(()),
            ],
            "javelins sold even with druid skills or unidentified; swords judged normally; \
             the merc's spear keeps its rule"
        );
        // An alternatives group counts not at all, neither its uniques (Tomb Reaver: a
        // polearm) nor a runeword open to any weapon (Breath of the Dying); a single required
        // runeword (Harmony: bows, crossbows) counts in full.
        let fury = Build {
            class: Some(5),
            switch_requires: vec![
                crate::playstyle::config::SwitchGroup {
                    label: "Powerful weapon".to_owned(),
                    any: vec!["Tomb Reaver".to_owned(), "Breath of the Dying".to_owned()],
                    count: 1,
                },
                crate::playstyle::config::SwitchGroup {
                    label: "Harmony".to_owned(),
                    any: vec!["Harmony".to_owned()],
                    count: 1,
                },
            ],
            ..Build::default()
        };
        assert_eq!(build_weapon_types([&fury].into_iter(), 5), ["bow", "xbow"]);
        // A weapon-slot runeword no base can carry for the class (an unknown class, here a
        // claw runeword for a druid) turns the rule off instead of selling everything but swords.
        for (class, label) in [(None, "druid"), (Some(99), "unknown class")] {
            let partial = Build {
                class,
                slots: BTreeMap::from([(
                    "weapon".to_owned(),
                    vec!["Chaos@57".to_owned(), "Spirit@25".to_owned()],
                )]),
                ..Build::default()
            };
            assert_eq!(
                build_weapon_types([&partial].into_iter(), class.unwrap_or(5)),
                Vec::<&str>::new(),
                "{label}"
            );
        }
        // An off-hand runeword without weapon carriers (a shield runeword) is no gap.
        let shield = Build {
            class: Some(5),
            slots: BTreeMap::from([
                ("weapon".to_owned(), vec!["Spirit@25".to_owned()]),
                ("offhand".to_owned(), vec!["Rhyme@29".to_owned()]),
            ]),
            ..Build::default()
        };
        assert_eq!(build_weapon_types([&shield].into_iter(), 5), ["swor"]);
        // No builds known: every weapon type is judged.
        let javelin = rolled("jav", QUALITY_RARE, &[(stat::CLASS_SKILLS, 5, 2)]);
        assert_eq!(judge(&javelin, &druid()), Ok(()));
    }

    #[test]
    fn boots_without_run_speed_are_sold() {
        let res = rolled("lbt", QUALITY_RARE, &[(stat::RES_COLD, 0, 24)]);
        let fast = rolled(
            "lbt",
            QUALITY_RARE,
            &[(stat::FRW, 0, 20), (stat::RES_COLD, 0, 24)],
        );
        let mut unid = res.clone();
        unid.flags = 0;
        assert_eq!(
            [res, fast, unid]
                .iter()
                .map(|b| judge(b, &druid()))
                .collect::<Vec<_>>(),
            vec![Err("no run speed"), Ok(()), Ok(())],
            "24 cold res without FRW is sold; 20 FRW + res judged normally; unidentified kept"
        );
        // Named boots (Sigon's Sabot) are build pieces or valuables, never judged.
        let mut needs = Needs::default();
        needs.piece("Sigon's Sabot@6", 1);
        assert_eq!(
            rows(&[at(named("hbt", 5, 38), Spot::Page(1))], &needs),
            vec![row("Sigon's Sabot", Spot::Page(1), Action::Keep(Side::Own), 1)]
        );
    }
}

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
//!   usable by the wearer: the player class or the mercenary of the build's act, one-handed
//!   where the wearer has only one hand free), the best one held
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
//! mercenary's gear has no stats in the snapshot, so a base it can wear (helm, body, its act's
//! weapon) that fails the player's test is kept unscored. The rest is sold, with the reason.
//! Two hard rules come first (user rules, 2026-11-20): a weapon whose type no build of the
//! stage chain uses ([`build_weapon_types`]: axes, daggers, javelins, claws, orbs … for the
//! druid) is sold as "not a build weapon", identified or not (the merc's weapons excepted); identified boots without faster run/walk are sold as "no run speed".
//! Right after the weapon-type rule (and before the trade rules): an identified magic / rare /
//! crafted item giving Teleport (oskill or charges) is Keep · own with the reason "Teleport"
//! while the player has no Enigma of its own (user, 2026-11-20; [`keeps_teleport`],
//! [`gear_score::teleport_wanted`]); with one it is scored like any other. A Teleport weapon
//! of a type no build uses is still sold as "not a build weapon".
//! On the map nothing changes: a ground drop's stats aren't read before pickup.
//! **Stow** (user rule, 2026-11-22): D2R's stash window has its own Gems, Materials and Runes
//! tabs, and the game sorts every stackable into one by type ([`stash::tab_of`]): gems and
//! skulls, runes, and the materials (uber keys, organs, essences, token, Standard of Heroes,
//! statues, worldstone shards, Colossal Jewel upgrade materials, rejuvenation potions). Such an
//! item anywhere else (inventory, cube, personal stash, pages 1–4) is moved to its tab, before
//! any other rule: never to a trade page, never sold. Rejuvenation potions in the inventory
//! stay (they are drunk from there). What already sits in those tabs is not judged; it counts
//! as held (the recipes' runes and gems, `web::recipes::materials_of`).
//! Charms in the inventory (they work there), finished runewords that aren't build pieces,
//! quest items and anything else no rule speaks about get no action.

use std::cmp::Reverse;
use std::collections::BTreeMap;

pub use d2r_core::guidance::{Action, Advice, Side, Spot};

use crate::config::{Bis, Build};
use crate::config::{FarmKind, Settings};
use crate::gear_score::{self, Piece, Profile};
use crate::valuables::{self, Tier, Verdict};
use crate::web::farm::{FarmStatus, farm_status};
use d2r_core::game::{ItemDrop, Place, Snapshot};
use d2r_core::guidance::Tab;
use d2r_core::item_stats::{ItemStat, stat};
use d2r_core::items::DropKind;
use d2r_core::names::same_name;
use d2r_core::offsets::item_data;
use d2r_core::runewords_data::{self, Runeword};
use d2r_core::socket_bases::{self, MercAct, Wearer};
use d2r_core::stash::{self, StashTab, TabKind};

/// The last shared-stash page for the player's own items; the pages after it are for trade.
const LAST_OWN_PAGE: u8 = 2;

/// The shared-stash page the plan ignores (user rule, 2026-11-20: "things i keep around for
/// stupidity"): its items get no verdict and nothing is moved there. They still count as held
/// ([`Snapshot::holdings`]), so a runeword there still drops its base need.
const IGNORED_PAGE: u8 = 5;

/// The side a shared page is for; `None` on the character and on the ignored page.
const fn page_side(spot: Spot) -> Option<Side> {
    match spot {
        Spot::Page(n) if n <= LAST_OWN_PAGE => Some(Side::Own),
        Spot::Page(n) if n != IGNORED_PAGE => Some(Side::Trade),
        Spot::Page(_) | Spot::Inventory | Spot::Cube | Spot::Stash => None,
    }
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
    /// The mercenary acts of the chain's builds ([`Build::merc_acts`]); empty = Act 2.
    pub merc_acts: Vec<MercAct>,
}

impl Scoring {
    /// Whether a mercenary of the chain could wear base `code` ([`socket_bases::merc_wears`]).
    fn merc_wears(&self, code: &str) -> bool {
        if self.merc_acts.is_empty() {
            return socket_bases::merc_wears(code, MercAct::default());
        }
        self.merc_acts
            .iter()
            .any(|&act| socket_bases::merc_wears(code, act))
    }
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
                d2r_core::bases_data::SOCKETABLE
                    .iter()
                    .map(|row| row.0)
                    .filter(|code| socket_bases::carries(word, code, wearer, Some(4)))
                    .collect()
            };
            let word = runewords_data::lookup(name);
            let codes: Vec<&str> = word.map_or_else(|| d2r_core::names::named_base_codes(name), carriers);
            if weapon_slot && word.is_some() && codes.is_empty() {
                return Vec::new();
            }
            for base in codes.into_iter().filter_map(d2r_core::bases_data::base) {
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
            let planned = d2r_core::skills_data::SKILLS
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
            .and_then(|e| d2r_core::bases_data::base(e.code.as_deref()?))
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
                teleport: gear_score::teleport_wanted(&snap.holdings()),
            },
            worn: snap
                .equipped_items
                .iter()
                .map(|e| (e.body, e.code.clone(), e.stats.clone()))
                .collect(),
            weapon_types: build_weapon_types(builds(), class),
            merc_acts: {
                let mut acts: Vec<MercAct> = Vec::new();
                for &act in builds().flat_map(|b| &b.merc_acts) {
                    if !acts.contains(&act) {
                        acts.push(act);
                    }
                }
                acts
            },
        }
    }
}

/// A magic / rare item is kept when it scores at least this share of what is worn in its slot.
pub const NEAR_WORN: f64 = 0.8;

/// A magic / rare item is kept whatever is worn when its affixes alone score this much: one
/// planned +skill, +1 class skill, 20 resistance or life, 20 caster FCR.
pub const NOTABLE: f64 = 20.0;

/// Whether `item` is a must-keep for its Teleport (user, 2026-11-20: "an amulet with teleport
/// on it ... very high for the summoner until he gets enigma. for the fury too"): an identified
/// magic, rare or crafted item giving Teleport as an oskill or charges
/// ([`gear_score::grants_skill`]) while no Enigma is held ([`Profile::teleport`]); but not a
/// weapon no build uses ([`off_build_weapon`]: it is still sold).
fn keeps_teleport(item: &ItemDrop, scoring: &Scoring) -> bool {
    scoring.profile.teleport
        && d2r_core::game::magic_or_better(item.quality)
        && item.flags & item_data::FLAG_IDENTIFIED != 0
        && gear_score::grants_skill(&item.stats, gear_score::TELEPORT)
        && !off_build_weapon(item, scoring)
}

/// A weapon of a type no build uses (user rule, 2026-11-20): sold, identified or not (no point
/// identifying it). The mercenary's weapons (its act's: polearms and spears for Act 2, never
/// javelins) keep the other rules.
fn off_build_weapon(item: &ItemDrop, scoring: &Scoring) -> bool {
    let code = item.code.as_deref();
    code.and_then(d2r_core::bases_data::base).is_some_and(|b| {
        b.slot == 4
            && !code.is_some_and(|c| scoring.merc_wears(c))
            && !scoring.weapon_types.is_empty()
            && !scoring.weapon_types.contains(&b.kind)
    })
}

/// The own-use verdict on a magic, rare or crafted item no trade rule wants: `Ok(reason)` keep
/// (`Some("Teleport")` for a Teleport item, [`keeps_teleport`]; it beats every rule after the
/// weapon type), `Err(reason)` sell.
fn judge(item: &ItemDrop, scoring: &Scoring) -> Result<Option<&'static str>, &'static str> {
    if off_build_weapon(item, scoring) {
        return Err("not a build weapon");
    }
    if keeps_teleport(item, scoring) {
        return Ok(Some("Teleport"));
    }
    let code = item.code.as_deref();
    let base = code.and_then(d2r_core::bases_data::base);
    let merc = code.is_some_and(|c| scoring.merc_wears(c));
    // Unidentified, or stats unread (a real magic / rare item always has affix stats): it
    // can't be judged yet.
    if item.flags & item_data::FLAG_IDENTIFIED == 0 || item.stats.is_empty() {
        return Ok(None);
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
        return if total > 0.0 { Ok(None) } else { Err(no_stats) };
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
        Some(old) if (total > 0.0 && total >= NEAR_WORN * old) || affixes >= NOTABLE => Ok(None),
        _ if merc => Ok(None),
        Some(_) if affixes > 0.0 => Err("worse than worn"),
        Some(_) => Err(no_stats),
        // Another class's item, or a shield beside a two-hander; no base known: keep.
        None => match item.code.as_deref().and_then(d2r_core::bases_data::base) {
            Some(base) if base.class.is_some_and(|c| u32::from(c) != who.class) => Err("other class"),
            Some(_) if affixes >= NOTABLE => Ok(None),
            Some(_) => Err(no_stats),
            None => Ok(None),
        },
    }
}

/// One runeword the player's builds want made, in a base for `wearer` (or `also`) worn in
/// `slot` (`ItemData::BODY` numbering; `None` = any slot).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BaseNeed {
    word: &'static Runeword,
    wearer: Wearer,
    slot: Option<u8>,
    /// A second wearer the same copy can go to: an alternative mercenary's (a build's second
    /// `merc_acts` entry) for a runeword someone else wants too ([`Needs::share`]).
    also: Option<Wearer>,
}

impl BaseNeed {
    /// Its wearers: `wearer`, then `also`.
    fn wearers(&self) -> impl Iterator<Item = Wearer> {
        std::iter::once(self.wearer).chain(self.also)
    }
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

    /// Wants a base for `entry` if it names a runeword; whether it does.
    fn base(&mut self, entry: &str, wearer: Wearer, slot: Option<u8>) -> bool {
        let Some(word) = runewords_data::lookup(piece_name(entry)) else {
            return false;
        };
        let need = BaseNeed {
            word,
            wearer,
            slot,
            also: None,
        };
        if !self.bases.contains(&need) {
            self.bases.push(need);
        }
        true
    }

    /// Every piece of build `slots` (slot name → pieces) and a base for each runeword in it
    /// ([`Needs::slot_bases`]).
    fn slots(&mut self, slots: &BTreeMap<String, Vec<String>>, wearer: Wearer) {
        for entry in slots.values().flatten() {
            self.piece(entry, 1);
        }
        self.slot_bases(slots, wearer, None);
    }

    /// A base for each runeword in build `slots` (only runeword `only` when set) for `wearer`;
    /// whether there was any. The player's weapon slot of a stage with off-hand pieces wants a
    /// one-handed base (user rule, 2026-11-20: a druid can't hold a Zweihander and a shield):
    /// [`Wearer::PlayerWithOffhand`]. The swap slot keeps both hands (Harmony is a bow).
    fn slot_bases(
        &mut self,
        slots: &BTreeMap<String, Vec<String>>,
        wearer: Wearer,
        only: Option<&str>,
    ) -> bool {
        // `parse_slot_tables` drops empty lists, so a present "offhand" has pieces.
        let offhand = slots.contains_key("offhand");
        let mut any = false;
        for (slot, pieces) in slots {
            let wearer = match wearer {
                Wearer::Player(class) if offhand && slot == "weapon" => Wearer::PlayerWithOffhand(class),
                other => other,
            };
            for entry in pieces {
                if only.is_none_or(|word| same_name(piece_name(entry), word)) {
                    any |= self.base(entry, wearer, slot_body(slot));
                }
            }
        }
        any
    }

    /// Every piece of mercenary `slots` and a base for each runeword in it
    /// ([`Needs::merc_bases`]).
    fn merc_slots(
        &mut self,
        slots: &BTreeMap<String, Vec<String>>,
        build: Option<&Build>,
        spare: &mut Vec<BaseNeed>,
    ) {
        for entry in slots.values().flatten() {
            self.piece(entry, 1);
        }
        self.merc_bases(slots, build, None, spare);
    }

    /// A base for each runeword in mercenary `slots` (only runeword `only` when set) of
    /// `build` ([`Needs::merc_base`]); whether there was any.
    fn merc_bases(
        &mut self,
        slots: &BTreeMap<String, Vec<String>>,
        build: Option<&Build>,
        only: Option<&str>,
        spare: &mut Vec<BaseNeed>,
    ) -> bool {
        let mut any = false;
        for (slot, pieces) in slots {
            for entry in pieces {
                if only.is_none_or(|word| same_name(piece_name(entry), word)) {
                    any |= self.merc_base(entry, build, slot_body(slot), spare);
                }
            }
        }
        any
    }

    /// Wants a base for mercenary piece `entry` in `slot` if it names a runeword; whether it
    /// does. The wearer is the mercenary of `build`'s act that can carry it
    /// ([`Build::merc_wearer`]; Act 2 without a build). A need of an alternative act (not the
    /// build's first: Harmony for the Act 1 rogue of `merc_acts = [2, 1]`) goes to `spare`
    /// for [`Needs::share`].
    fn merc_base(
        &mut self,
        entry: &str,
        build: Option<&Build>,
        slot: Option<u8>,
        spare: &mut Vec<BaseNeed>,
    ) -> bool {
        let act = build.map_or_else(MercAct::default, |b| b.merc_wearer(entry, slot));
        if build.is_none_or(|b| b.merc_act() == act) {
            return self.base(entry, Wearer::Merc(act), slot);
        }
        let Some(word) = runewords_data::lookup(piece_name(entry)) else {
            return false;
        };
        spare.push(BaseNeed {
            word,
            wearer: Wearer::Merc(act),
            slot,
            also: None,
        });
        true
    }

    /// Adds the alternative mercenaries' needs (`spare`, [`Needs::merc_base`]). One that a
    /// need of the same runeword already covers (the same slot, else any slot) adds no second
    /// base: one copy is swapped between them (user, 2026-11-20: Harmony on the druid's weapon
    /// swap, or on the Act 1 rogue in Normal or for its buff; never both at once). That need
    /// then also takes a base only the alternative mercenary can use (an amazon bow).
    fn share(&mut self, spare: Vec<BaseNeed>) {
        for need in spare {
            // The same slot first: a slot-less need of a placed runeword is dropped later.
            let at = |slot_less: bool| {
                move |n: &BaseNeed| {
                    n.word.name == need.word.name && (n.slot == need.slot || (slot_less && n.slot.is_none()))
                }
            };
            let same = match self.bases.iter().position(at(false)) {
                Some(i) => Some(i),
                None => self.bases.iter().position(at(true)),
            };
            match same.and_then(|i| self.bases.get_mut(i)) {
                Some(n) => {
                    if n.wearer != need.wearer && n.also.is_none() {
                        n.also = Some(need.wearer);
                    }
                }
                None => self.bases.push(need),
            }
        }
    }

    /// The player's own needs: the resolved `bis` (the current stage, or the character's
    /// inline slots), every build of the stage `chain` (the current stage and the later ones,
    /// by name: [`Settings::stage_chain`]) with its slots, extras and `switch_requires`, and the
    /// whole farm list. A player piece is for class `class` (a farm group's: its build's class);
    /// a mercenary one for the mercenary of its build's act that can carry it
    /// ([`Build::merc_wearer`]; Act 2 for an inline `bis` without a build file), where an
    /// alternative mercenary's runeword shares the need of the same runeword
    /// ([`Needs::share`]). A farm runeword its group's build places in its
    /// own slots takes that build's base needs (the second Lawbringer is the Act 5 merc's
    /// one-handed sword), not a looser one for any slot. Copies `worn` by the character or its
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
        let mut spare = Vec::new();
        let def = |build: &str| config.build_defs.get(&build.to_lowercase());
        if let Some(bis) = bis {
            needs.slots(&bis.slots, Wearer::Player(class));
            needs.merc_slots(&bis.merc_slots, def(&bis.build), &mut spare);
        }
        for build in chain
            .iter()
            .filter_map(|name| config.build_defs.get(&name.to_lowercase()))
        {
            let player = Wearer::Player(build.class.unwrap_or(class));
            needs.slots(&build.slots, player);
            needs.merc_slots(&build.merc_slots, Some(build), &mut spare);
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
            let target = config.build_defs.get(&group.target.to_lowercase());
            let farm_class = target.and_then(|b| b.class).unwrap_or(class);
            // A rolled entry names no item: the magic / rare judge keeps its copies
            // ([`keeps_teleport`]).
            for item in group.items.iter().filter(|i| i.kind != FarmKind::Rolled) {
                needs.piece(&item.name, item.count);
                if let Some(build) = target {
                    let player = Wearer::Player(farm_class);
                    let placed = needs.slot_bases(&build.slots, player, Some(&item.name));
                    let merc_placed =
                        needs.merc_bases(&build.merc_slots, Some(build), Some(&item.name), &mut spare);
                    if placed || merc_placed {
                        continue;
                    }
                }
                let slot = item.slot.map(crate::config::FarmSlot::body);
                if item.merc {
                    needs.merc_base(&item.name, target, slot, &mut spare);
                } else {
                    needs.base(&item.name, Wearer::Player(farm_class), slot);
                }
            }
        }
        needs.share(spare);
        // A runeword a build places in a slot wants its base for that slot (and hands); a
        // slot-less want of it (a `switch_requires` condition such as "2x Lawbringer", a farm
        // entry without a slot) adds no looser need that a two-hander could fill.
        let placed: Vec<&str> = needs
            .bases
            .iter()
            .filter(|n| n.slot.is_some())
            .map(|n| n.word.name)
            .collect();
        needs
            .bases
            .retain(|n| n.slot.is_some() || !placed.contains(&n.word.name));
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
pub fn plain_base(item: &ItemDrop) -> bool {
    d2r_core::game::maybe_white_base(item.quality, item.flags)
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

/// [`base_rank`]'s key: power, grade, requirement (lower is better).
type Rank = (u32, u8, Reverse<u32>);

/// How good a runeword base is, better is greater (user rule, 2026-11-20: a better base for the
/// same runeword replaces the kept one): the base damage (min + max, [`weapons_data::damage`])
/// of a weapon or the maximum base defense of an armor, helm or shield
/// ([`socket_bases::max_defense`]); then superior or ethereal over normal over low quality;
/// then the lower strength + dexterity requirement. A need open to both weapons and armor
/// (Fortitude without a slot) compares damage with defense: rare, and either base works.
fn base_rank(item: &ItemDrop) -> Rank {
    let code = item.code.as_deref().unwrap_or_default();
    let raw = d2r_core::weapons_data::damage(code)
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
    let req = d2r_core::bases_data::base(code).map_or(0, |b| b.str.saturating_add(b.dex));
    (power, grade, Reverse(req))
}

/// Whether `base` can be made into the runeword of `need`: exactly its socket count, and a
/// base of its type the wearer can use in the slot ([`socket_bases::fits`]).
fn fills(need: &BaseNeed, base: &ItemDrop) -> bool {
    usize::from(base.sockets) == need.word.runes.len()
        && base.code.as_deref().is_some_and(|code| {
            need.wearers()
                .any(|w| socket_bases::fits(need.word, code, w, need.slot))
        })
}

/// The stash tab `h` belongs in when it is a gem, rune or material ([`stash::tab_of`]); `None`
/// for anything else, for anything in the cube (a recipe in progress) and for a rejuvenation
/// potion in the inventory (drunk from there).
fn stow(h: &Held) -> Option<Tab> {
    let code = h.item.code.as_deref()?;
    let potion = matches!(code, "rvs" | "rvl");
    stash::tab_of(code).filter(|_| h.spot != Spot::Cube && !(potion && h.spot == Spot::Inventory))
}

/// An item the plan leaves alone: charms in the inventory (they work there), finished
/// runewords that aren't a build piece, and gems, runes and materials that [`stow`] doesn't
/// move (in the cube, an inventory rejuv, or a gem or rune without a code): they are never
/// judged by the trade or sell rules.
fn left_alone(h: &Held) -> bool {
    let stackable = h.item.code.as_deref().and_then(stash::tab_of).is_some();
    if stackable || matches!(h.item.kind, DropKind::Gem(_) | DropKind::Rune(_)) {
        return stow(h).is_none();
    }
    match h.item.kind {
        DropKind::Gem(_) | DropKind::Rune(_) => false,
        DropKind::Charm { .. } => h.spot == Spot::Inventory,
        DropKind::Base | DropKind::Set | DropKind::Unique => h.item.flags & item_data::FLAG_RUNEWORD != 0,
    }
}

/// Keep for `side` where the item is, or move it there. Own items belong on pages 1–2 or in
/// the personal stash; one in the inventory or cube is in the wrong place too (user rule,
/// 2026-11-20: "if something is at the wrong place (inv, stash)": move it to 1–2).
const fn keep_or_move(side: Side, spot: Spot) -> Action {
    match (spot, page_side(spot), side) {
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
    let rank = |spot: Spot| match (side, page_side(spot), spot) {
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
/// character and its mercenary wear; magic / rare items scored for the chain's builds. With it
/// what a ground drop would still add ([`Wants`], the map's keep and farm marks).
#[must_use]
pub fn for_snapshot(
    snap: &Snapshot,
    config: &Settings,
    chain: &[String],
    bis: Option<&Bis>,
) -> (Vec<Advice>, Wants) {
    let worn: Vec<String> = d2r_core::game::worn_names(&snap.equipped_items)
        .into_iter()
        .chain(snap.merc_worn.iter().cloned())
        .collect();
    let holdings = snap.holdings();
    let owns = |name: &str| holdings.count(name);
    let needs = Needs::from_config(config, chain, bis, snap.player.class, &worn, &owns);
    let (advice, mut wants) = plan(&held(snap), &needs, &Scoring::new(snap, config, chain));
    // A farm-list unique or set nobody holds yet (the farm panel's "missing").
    let missing = |name: &str| {
        config.farm.iter().flat_map(|g| &g.items).any(|item| {
            matches!(item.kind, FarmKind::Unique | FarmKind::Set) && same_name(&item.name, name) && {
                let (stashed, carried) = holdings.copies(&item.name, item.kind.item_kind());
                farm_status(item.count, stashed, carried) == FarmStatus::Missing
            }
        })
    };
    for piece in &mut wants.pieces {
        piece.farm = missing(&piece.name);
    }
    (advice, wants)
}

/// What a ground drop would add to the item plan ([`for_snapshot`]), computed once per
/// evaluation and asked per drop and tick ([`crate::marks`]): a base for an open
/// runeword need better than the one kept for it, a unique or set build piece still wanted.
/// Magic and rare drops are not judged: their stats aren't known before pickup.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Wants {
    /// Every runeword base need with the rank of the base the plan keeps for it (`None`: none
    /// held).
    bases: Vec<(BaseNeed, Option<Rank>)>,
    /// The unique and set build pieces with copies still wanted.
    pieces: Vec<WantedPiece>,
}

/// A unique or set piece of [`Wants`].
#[derive(Debug, Clone, PartialEq, Eq)]
struct WantedPiece {
    name: String,
    /// Quality and base code of each unique / set row of that name ([`d2r_core::names::named_bases_cached`]):
    /// what an unidentified copy looks like.
    bases: &'static [(u32, &'static str)],
    /// A still-missing farm-list entry ([`farm_status`]).
    farm: bool,
}

impl Wants {
    /// The wanted piece `drop` is: `(rule, farm entry, uncertain)`. An identified unique or
    /// set by its name (the rule is the name); an unidentified one (no name yet) by its quality
    /// and base code, uncertain: the rule names every wanted piece on that base with a "?"
    /// ("Raven Frost? / Nagelring?"), the farm entries only when any is one.
    #[must_use]
    pub fn piece(&self, drop: &ItemDrop) -> Option<(String, bool, bool)> {
        if !matches!(drop.quality, item_data::QUALITY_UNIQUE | item_data::QUALITY_SET) {
            return None;
        }
        if let Some(index) = d2r_core::names::identity(drop.quality, drop.flags, drop.unique_index) {
            let shown = d2r_core::names::shown(index);
            return self
                .pieces
                .iter()
                .find(|p| same_name(&p.name, index) || same_name(&p.name, shown))
                .map(|p| (p.name.clone(), p.farm, false));
        }
        let code = drop.code.as_deref()?;
        let fits = |p: &&WantedPiece| p.bases.contains(&(drop.quality, code));
        let farm = self.pieces.iter().filter(fits).any(|p| p.farm);
        let names: Vec<String> = self
            .pieces
            .iter()
            .filter(fits)
            .filter(|p| p.farm || !farm)
            .map(|p| format!("{}?", p.name))
            .collect();
        (!names.is_empty()).then(|| (names.join(" / "), farm, true))
    }

    /// The runeword whose open base need `drop` fills ([`fills`]: exactly its sockets, which
    /// on a ground drop are natural ones) better than the base kept for it ([`base_rank`]),
    /// `None` for anything else. A trade-worthy base is no own base ([`trade_worthy`]).
    #[must_use]
    pub fn base(&self, drop: &ItemDrop) -> Option<&'static str> {
        if !plain_base(drop) {
            return None;
        }
        let rank = base_rank(drop);
        let (need, _) = self
            .bases
            .iter()
            .find(|(need, kept)| fills(need, drop) && kept.as_ref().is_none_or(|k| rank > *k))?;
        // Last: the rules lookup is the costly part.
        (!trade_worthy(drop)).then_some(need.word.name)
    }
}

/// The reason a plain base is sold: "two-handed" when it would fill a runeword need but for
/// its hands (user request, 2026-11-20: a Zweihander for a dual-wielding merc), else none.
fn base_sell_reason(base: &ItemDrop, needs: &Needs) -> Option<&'static str> {
    let code = base.code.as_deref()?;
    needs
        .bases
        .iter()
        .any(|n| {
            usize::from(base.sockets) == n.word.runes.len()
                && n.wearers()
                    .any(|w| socket_bases::two_handed_misfit(n.word, code, w, n.slot))
        })
        .then_some("two-handed")
}

/// An item's action with the reason of a scored sell.
type Decided = (Action, Option<&'static str>);

/// One base per runeword need, the needs with the fewest fitting bases first (a base that fits
/// two needs must not starve the narrower one), each taking its best free base ([`base_rank`];
/// on a tie the best placed, `order`, [`by_preference`]). The other bases are sold by [`plan`],
/// a worse one already kept on pages 1–2 included. A trade-worthy base is left for the trade
/// side.
fn assign_bases(
    items: &[Held],
    needs: &Needs,
    order: &[usize],
    actions: &mut [Option<Decided>],
) -> Vec<Option<usize>> {
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
    let mut by_choice: Vec<(usize, Vec<usize>)> = needs
        .bases
        .iter()
        .enumerate()
        .map(|(n, need)| {
            let fits = candidates
                .iter()
                .copied()
                .filter(|&i| items.get(i).is_some_and(|h| fills(need, &h.item)))
                .collect();
            (n, fits)
        })
        .collect();
    by_choice.sort_by_key(|(_, fits)| fits.len());
    let mut kept = vec![None; needs.bases.len()];
    for (n, fits) in by_choice {
        // `min_by_key` keeps the first of equal keys: the best-placed of equal bases.
        let free = fits
            .into_iter()
            .filter(|&i| actions.get(i).is_some_and(Option::is_none))
            .min_by_key(|&i| Reverse(items.get(i).map(|h| base_rank(&h.item))));
        if let Some((i, (h, slot))) = free.and_then(|i| Some((i, items.get(i).zip(actions.get_mut(i))?))) {
            *slot = Some((keep_or_move(Side::Own, h.spot), None));
            if let Some(k) = kept.get_mut(n) {
                *k = Some(i);
            }
        }
    }
    kept
}

/// The action for each of `items` (see the module docs), merged into dashboard lines and
/// sorted by action, then name, then spot.
/// With them the [`Wants`] left after the plan (farm flags unset: [`for_snapshot`] sets them).
#[must_use]
pub fn plan(items: &[Held], needs: &Needs, scoring: &Scoring) -> (Vec<Advice>, Wants) {
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
    let kept = assign_bases(items, needs, &own_order, &mut actions);
    let wants = Wants {
        bases: needs
            .bases
            .iter()
            .zip(kept)
            .map(|(need, k)| (*need, k.and_then(|i| items.get(i)).map(|h| base_rank(&h.item))))
            .collect(),
        pieces: needs
            .pieces
            .iter()
            .zip(&wanted)
            .filter(|&((name, _), &left)| left > 0 && runewords_data::lookup(name).is_none())
            .map(|((name, _), _)| WantedPiece {
                name: name.clone(),
                bases: d2r_core::names::named_bases_cached(name),
                farm: false,
            })
            .collect(),
    };
    // The rest: trade, sell or give away. A keep-one item keeps its copy best placed for trade.
    for i in by_preference(items, Side::Trade) {
        let (Some(h), Some(slot)) = (items.get(i), actions.get_mut(i)) else {
            continue;
        };
        if slot.is_some() || left_alone(h) {
            continue;
        }
        // A gem, rune or material goes to its tab before any other rule; a Teleport item is
        // own gear before any trade rule ([`keeps_teleport`]).
        let action = if let Some(tab) = stow(h) {
            Some(Action::Stow(tab))
        } else if trade_worthy(&h.item) && !keeps_teleport(&h.item, scoring) {
            match valuables::keep_one_name(&h.item) {
                Some(name) if kept_one.contains(&name) => Some(Action::GiveAway),
                one => {
                    kept_one.extend(one);
                    Some(keep_or_move(Side::Trade, h.spot))
                }
            }
        } else if plain_base(&h.item) {
            *slot = Some((Action::Sell, base_sell_reason(&h.item, needs)));
            continue;
        } else if d2r_core::game::magic_or_better(h.item.quality) {
            *slot = Some(match judge(&h.item, scoring) {
                Ok(reason) => (keep_or_move(Side::Own, h.spot), reason),
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
    (out, wants)
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
            .0
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
                    kind: FarmKind::Runeword,
                    count: 1,
                    merc: true,
                    slot: Some(FarmSlot::Weapon),
                    needs: Vec::new(),
                    note: None,
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
    fn a_ground_base_is_wanted_only_when_better_than_the_kept_one() {
        // Spirit in the weapon slot, a Crystal Sword 4os kept on page 1.
        let mut spirit = Needs::default();
        spirit.base("Spirit", DRUID, Some(4));
        let (_, wants) = plan(&[at(base("crs", 4), Spot::Page(1))], &spirit, &druid());
        assert_eq!(
            wants.base(&base("bsd", 4)),
            Some("Spirit"),
            "Broad Sword beats it"
        );
        assert_eq!(wants.base(&base("crs", 4)), None, "the same base: no better");
        assert_eq!(wants.base(&base("bsd", 3)), None, "wrong socket count");
        assert_eq!(wants.base(&base("crs", 5)), None, "5os: no Spirit");
        let mut magic = base("bsd", 4);
        magic.quality = item_data::QUALITY_MAGIC;
        assert_eq!(wants.base(&magic), None, "a magic item takes no runeword");
        // Nothing kept yet: any fitting base is wanted.
        let (_, open) = plan(&[], &spirit, &druid());
        assert_eq!(open.base(&base("crs", 4)), Some("Spirit"));
        // A trade-worthy base is never an own base: eth elite polearm 4os (twink S) for Insight.
        let mut insight = Needs::default();
        insight.base("Insight", Wearer::Merc(MercAct::Act2), Some(4));
        let (_, merc) = plan(&[], &insight, &druid());
        assert_eq!(merc.base(&base("9vo", 4)), Some("Insight"), "a plain Bill 4os");
        let mut eth = base("7vo", 4);
        eth.ethereal = true;
        assert_eq!(merc.base(&eth), None, "an eth Colossus Voulge 4os goes to trade");
    }

    #[test]
    fn a_ground_unique_is_wanted_while_copies_are_missing() {
        let mut needs = Needs::default();
        needs.piece("Harlequin Crest", 1);
        needs.piece("Spirit", 1);
        let (_, wants) = plan(&[], &needs, &druid());
        let shako = named("uap", item_data::QUALITY_UNIQUE, 248);
        assert_eq!(
            wants.piece(&shako),
            Some(("Harlequin Crest".to_owned(), false, false)),
            "by name"
        );
        let unidentified = ItemDrop {
            flags: 0,
            unique_index: None,
            ..shako.clone()
        };
        assert_eq!(
            wants.piece(&unidentified),
            Some(("Harlequin Crest?".to_owned(), false, true)),
            "by quality and base, uncertain"
        );
        let mut set_cap = unidentified;
        set_cap.quality = item_data::QUALITY_SET;
        assert_eq!(wants.piece(&set_cap), None, "a set item on the same base");
        assert_eq!(wants.piece(&base("uap", 0)), None, "a white Shako");
        // One copy held: no more wanted.
        let (_, held) = plan(&[at(shako.clone(), Spot::Page(1))], &needs, &druid());
        assert_eq!(held.piece(&shako), None);
        assert!(
            wants.pieces.iter().all(|p| p.name != "Spirit"),
            "runewords don't drop"
        );
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
        let plates_plan = plan(&plates, &fortitude, &druid()).0;
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
        let eth_plan = plan(&mixed, &fortitude, &druid()).0;
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
            .0
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
    fn a_teleport_item_is_kept_own_until_enigma_is_held() {
        // Level 1 Teleport charges: param = 54 << 6 | 1.
        let charges = (stat::CHARGED_SKILL, (gear_score::TELEPORT << 6) | 1, 10);
        let amulet = rolled("amu", QUALITY_MAGIC, &[charges]);
        let mut scoring = druid();
        scoring.profile.teleport = true;
        // A far better amulet is worn: the Teleport one is kept anyway, and moved to 1–2.
        scoring.worn = vec![(
            2,
            Some("amu".to_owned()),
            vec![ItemStat {
                id: stat::CLASS_SKILLS,
                param: 5,
                value: 2,
                base: false,
            }],
        )];
        let items = [
            at(amulet.clone(), Spot::Inventory),
            at(amulet.clone(), Spot::Page(1)),
        ];
        assert_eq!(
            judged(&items, &scoring),
            vec![
                (
                    "Magic Amulet".to_owned(),
                    Spot::Inventory,
                    Action::Move(Side::Own),
                    Some("Teleport")
                ),
                (
                    "Magic Amulet".to_owned(),
                    Spot::Page(1),
                    Action::Keep(Side::Own),
                    Some("Teleport")
                ),
            ]
        );
        // As an oskill too.
        let oskill = rolled(
            "amu",
            QUALITY_MAGIC,
            &[(stat::NONCLASS_SKILL, gear_score::TELEPORT, 1)],
        );
        assert_eq!(
            judged(&[at(oskill, Spot::Page(2))], &scoring),
            vec![(
                "Magic Amulet".to_owned(),
                Spot::Page(2),
                Action::Keep(Side::Own),
                Some("Teleport")
            )]
        );
        // A Teleport weapon of a type no build uses is still sold; a build weapon is kept.
        scoring.weapon_types = vec!["staf"];
        let axe = rolled("hax", QUALITY_MAGIC, &[charges]);
        let staff = rolled("sst", QUALITY_MAGIC, &[charges]);
        assert_eq!(
            judged(&[at(axe, Spot::Page(1)), at(staff, Spot::Page(1))], &scoring)
                .into_iter()
                .map(|(_, _, action, reason)| (action, reason))
                .collect::<Vec<_>>(),
            vec![
                (Action::Sell, Some("not a build weapon")),
                (Action::Keep(Side::Own), Some("Teleport")),
            ],
            "the axe is sold, the staff kept"
        );
        scoring.weapon_types = Vec::new();
        // Enigma held: scored like any other amulet, against the worn one.
        scoring.profile.teleport = false;
        assert_eq!(
            judged(&[at(amulet, Spot::Page(1))], &scoring),
            vec![(
                "Magic Amulet".to_owned(),
                Spot::Page(1),
                Action::Sell,
                Some("worse than worn")
            )]
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
            size: d2r_core::items::CharmSize::Small,
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
    fn carried_charms_and_other_items_are_left_alone() {
        let mut torch = named("cm2", 7, 381);
        torch.kind = DropKind::Charm {
            size: d2r_core::items::CharmSize::Large,
            unique: true,
        };
        let mut spirit = base("crs", 4);
        spirit.flags |= item_data::FLAG_RUNEWORD;
        let items = [
            at(torch, Spot::Inventory),
            at(base("hp1", 0), Spot::Inventory),
            at(base("rvl", 0), Spot::Inventory),
            at(spirit, Spot::Page(4)),
        ];
        assert_eq!(rows(&items, &harmony_needs()), vec![]);
    }

    /// User report, 2026-11-22: a Key of Terror was sent to the trade pages, but the game has a
    /// Materials tab for it (and Gems and Runes tabs).
    #[test]
    fn gems_runes_and_materials_go_to_their_stash_tab() {
        let mut vex = base("r26", 0);
        vex.kind = DropKind::Rune(26);
        let mut gem = base("gpv", 0);
        gem.kind = DropKind::Gem(d2r_core::items::GemColour::Amethyst);
        let items = [
            at(base("pk1", 0), Spot::Inventory),
            at(base("pk1", 0), Spot::Page(3)),
            at(base("mbr", 0), Spot::Page(1)),
            // In the cube (a recipe in progress) it stays, and no other rule judges it.
            at(base("xa2", 0), Spot::Cube),
            at(base("rvl", 0), Spot::Stash),
            at(vex.clone(), Spot::Page(1)),
            at(vex, Spot::Inventory),
            at(gem, Spot::Page(4)),
            // The ignored page stays ignored.
            at(base("pk2", 0), Spot::Page(IGNORED_PAGE)),
        ];
        assert_eq!(
            rows(&items, &harmony_needs()),
            vec![
                row("Perfect Amethyst", Spot::Page(4), Action::Stow(Tab::Gems), 1),
                row(
                    "Full Rejuvenation Potion",
                    Spot::Stash,
                    Action::Stow(Tab::Materials),
                    1
                ),
                row("Key of Terror", Spot::Inventory, Action::Stow(Tab::Materials), 1),
                row("Key of Terror", Spot::Page(3), Action::Stow(Tab::Materials), 1),
                row("Mephisto's Brain", Spot::Page(1), Action::Stow(Tab::Materials), 1),
                row("Vex Rune", Spot::Inventory, Action::Stow(Tab::Runes), 1),
                row("Vex Rune", Spot::Page(1), Action::Stow(Tab::Runes), 1),
            ]
        );
    }

    #[test]
    fn the_materials_tab_and_live_shared_items_are_not_judged() {
        use d2r_core::stash::StashItem;
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
        use crate::config::SwitchGroup;
        use crate::config::{Build, FarmGroup, FarmItem, FarmSlot};
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
                kind: FarmKind::Runeword,
                count: 1,
                merc: true,
                slot: Some(FarmSlot::Weapon),
                needs: Vec::new(),
                note: None,
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
                ("Bulwark", Wearer::Merc(MercAct::Act2), Some(1)),
                ("Lawbringer", DRUID, None),
                ("Insight", Wearer::Merc(MercAct::Act2), Some(4)),
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
                    kind: FarmKind::Runeword,
                    count: 1,
                    merc: false,
                    slot: Some(FarmSlot::Weapon),
                    needs: Vec::new(),
                    note: None,
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
        let advice = plan(&items, &Needs::default(), &druid()).0;
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
        use d2r_core::stash::StashItem;
        let config = Settings {
            farm: vec![FarmGroup {
                target: "Fury".to_owned(),
                items: vec![FarmItem {
                    name: "Harmony".to_owned(),
                    kind: FarmKind::Runeword,
                    count: 1,
                    merc: false,
                    slot: Some(FarmSlot::Weapon),
                    needs: Vec::new(),
                    note: None,
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
        let holdings = d2r_core::stash::Holdings {
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
                Ok(None),
                Err("no build stats"),
                Ok(None),
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
                crate::config::SwitchGroup {
                    label: "Powerful weapon".to_owned(),
                    any: vec!["Tomb Reaver".to_owned(), "Breath of the Dying".to_owned()],
                    count: 1,
                },
                crate::config::SwitchGroup {
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
        assert_eq!(judge(&javelin, &druid()), Ok(None));
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
            vec![Err("no run speed"), Ok(None), Ok(None)],
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

    /// The shipped playstyle (`playstyles/summoner-fury/`): build files and farm list.
    fn shipped() -> Settings {
        crate::files::load(&d2r_core::playstyle::Files {
            shipped: Some(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../playstyles/summoner-fury"),
            ),
            quiet: true,
            ..d2r_core::playstyle::Files::default()
        })
    }

    /// The needs of a druid on the shipped Summoner → Fury chain at stage `bis_build`, holding
    /// no copy of anything.
    fn shipped_needs(config: &Settings, bis_build: &str) -> anyhow::Result<Needs> {
        let def = config
            .build_defs
            .get(bis_build)
            .ok_or_else(|| anyhow::anyhow!("no build {bis_build}"))?;
        let bis = Bis {
            build: def.name.clone(),
            slots: def.slots.clone(),
            merc_slots: def.merc_slots.clone(),
        };
        let chain = ["summoner druid".to_owned(), "fury druid".to_owned()];
        Ok(Needs::from_config(config, &chain, Some(&bis), 5, &[], &|_| 0))
    }

    /// A plan row with its sell reason.
    type Reasoned = (String, Spot, Action, Option<&'static str>, u32);

    /// `(name, spot, action, reason, count)` rows of the plan.
    fn reasoned(items: &[Held], needs: &Needs) -> Vec<Reasoned> {
        plan(items, needs, &druid())
            .0
            .into_iter()
            .map(|a| (a.name, a.spot, a.action, a.reason, a.count))
            .collect()
    }

    #[test]
    fn fury_merc_is_act_5_and_lawbringer_needs_are_one_handed() -> anyhow::Result<()> {
        let config = shipped();
        let fury = config
            .build_defs
            .get("fury druid")
            .ok_or_else(|| anyhow::anyhow!("fury"))?;
        assert_eq!(fury.merc_acts, vec![MercAct::Act5]);
        let summoner = config
            .build_defs
            .get("summoner druid")
            .ok_or_else(|| anyhow::anyhow!("summoner"))?;
        assert_eq!(
            summoner.merc_acts,
            vec![MercAct::Act2, MercAct::Act1],
            "Insight's Act 2 merc first, the Act 1 rogue for Harmony"
        );
        for stage in ["summoner druid", "fury druid"] {
            let needs = shipped_needs(&config, stage)?;
            let lawbringer: Vec<(Wearer, Option<u8>)> = needs
                .bases
                .iter()
                .filter(|n| n.word.name == "Lawbringer")
                .map(|n| (n.wearer, n.slot))
                .collect();
            // Both are the dual-wielding Act 5 merc's (off-hand and weapon; the druid fights
            // two-handed); the farm entry and the summoner's "2x Lawbringer" switch condition
            // add no looser third one.
            assert_eq!(
                lawbringer,
                vec![
                    (Wearer::Merc(MercAct::Act5), Some(5)),
                    (Wearer::Merc(MercAct::Act5), Some(4)),
                ],
                "{stage}"
            );

            // Harmony in the swap slot keeps both hands (a bow).
            assert!(
                needs
                    .bases
                    .iter()
                    .any(|n| n.word.name == "Harmony" && n.wearer == DRUID),
                "{stage}"
            );
            // The summoner's merc keeps Act 2 for Insight.
            assert!(
                needs
                    .bases
                    .iter()
                    .any(|n| n.word.name == "Insight" && n.wearer == Wearer::Merc(MercAct::Act2)),
                "{stage}"
            );
        }
        // The Fury has no off-hand: its own weapon-slot runewords keep both hands (the farm
        // list's summoner entries, Spirit and Heart of the Oak beside a shield, don't).
        let bis = Bis {
            build: fury.name.clone(),
            slots: fury.slots.clone(),
            merc_slots: fury.merc_slots.clone(),
        };
        let own = Needs::from_config(&config, &["fury druid".to_owned()], Some(&bis), 5, &[], &|_| 0);
        let swap: Vec<Wearer> = own
            .bases
            .iter()
            .filter(|n| matches!(n.word.name, "Harmony" | "Call to Arms"))
            .map(|n| n.wearer)
            .collect();
        assert_eq!(swap, vec![DRUID, DRUID]);
        Ok(())
    }

    #[test]
    fn one_harmony_base_serves_the_druid_swap_and_the_act1_rogue() -> anyhow::Result<()> {
        let config = shipped();
        let needs = shipped_needs(&config, "summoner druid")?;
        // The Fury's swap Harmony and the summoner's Act 1 rogue Harmony are one need: one
        // copy is swapped between them.
        let harmony: Vec<_> = needs
            .bases
            .iter()
            .filter(|n| n.word.name == "Harmony")
            .map(|n| (n.wearer, n.slot, n.also))
            .collect();
        assert_eq!(harmony, vec![(DRUID, Some(4), Some(Wearer::Merc(MercAct::Act1)))]);
        let own = Action::Keep(Side::Own);
        let mine = |n: &str, action| (n.to_owned(), Spot::Page(1), action, 1);
        // A 4os Hunter's Bow is kept for Harmony; so is a 4os Stag Bow, an amazon bow
        // only the rogue can use.
        assert_eq!(
            rows(&[at(base("hbw", 4), Spot::Page(1))], &needs),
            vec![mine("Hunter's Bow 4os", own)]
        );
        assert_eq!(
            rows(&[at(base("am1", 4), Spot::Page(1))], &needs),
            vec![mine("Stag Bow 4os", own)]
        );
        // Both held: one Harmony, one base (the stronger amazon bow; a 4os Matriarchal Bow would go to trade); the other is sold.
        assert_eq!(
            rows(
                &[
                    at(base("hbw", 4), Spot::Page(1)),
                    at(base("am1", 4), Spot::Page(1))
                ],
                &needs
            ),
            vec![mine("Hunter's Bow 4os", Action::Sell), mine("Stag Bow 4os", own)]
        );
        // On the Fury stage alone (Act 5 merc only) the amazon bow is no base: the druid
        // can't use it.
        let fury = config
            .build_defs
            .get("fury druid")
            .ok_or_else(|| anyhow::anyhow!("fury"))?;
        let bis = Bis {
            build: fury.name.clone(),
            slots: fury.slots.clone(),
            merc_slots: fury.merc_slots.clone(),
        };
        let fury_needs = Needs::from_config(&config, &["fury druid".to_owned()], Some(&bis), 5, &[], &|_| 0);
        assert_eq!(
            rows(&[at(base("am1", 4), Spot::Page(1))], &fury_needs),
            vec![mine("Stag Bow 4os", Action::Sell)]
        );
        Ok(())
    }

    #[test]
    fn an_alternative_merc_runeword_alone_is_its_own_need() -> anyhow::Result<()> {
        // A build whose only Harmony is the Act 1 rogue's: a need of its own, for the rogue.
        let (_, build, _) = crate::config::parse_build(
            "name = \"B\"\nmerc_acts = [2, 1]\n[merc_slots]\nweapon = [\"Insight@27\", \"Harmony@39\"]\n",
        )?;
        let mut config = Settings::default();
        config.build_defs.insert("b".to_owned(), build);
        let needs = Needs::from_config(&config, &["b".to_owned()], None, 5, &[], &|_| 0);
        let wanted: Vec<_> = needs
            .bases
            .iter()
            .map(|n| (n.word.name, n.wearer, n.also))
            .collect();
        assert_eq!(
            wanted,
            vec![
                ("Insight", Wearer::Merc(MercAct::Act2), None),
                ("Harmony", Wearer::Merc(MercAct::Act1), None),
            ]
        );
        Ok(())
    }

    #[test]
    fn live_two_broad_swords_kept_and_the_zweihander_sold_as_two_handed() -> anyhow::Result<()> {
        // The live stash (2026-11-20): two Broad Sword 3os on page 2, a Zweihander 3os in the
        // inventory, no Lawbringer held. Regression (2026-11-20): the shipped build files must
        // load (a field the parser rejected dropped every build, and both swords were sold);
        // the Fury in the chain has the Act 5 merc.
        let config = shipped();
        assert_eq!(
            config.build_defs.get("fury druid").map(|b| b.merc_acts.clone()),
            Some(vec![MercAct::Act5])
        );
        let needs = shipped_needs(&config, "summoner druid")?;
        let items = [
            at(base("bsd", 3), Spot::Page(2)),
            at(base("bsd", 3), Spot::Page(2)),
            at(base("9fb", 3), Spot::Inventory),
        ];
        let own = Action::Keep(Side::Own);
        let mine = |n: &str, spot, action, reason, count| (n.to_owned(), spot, action, reason, count);
        // Both Broad Swords kept (the Act 5 merc's two Lawbringer bases), merged into one line;
        // no 3-socket need of the builds takes a two-handed base.
        assert_eq!(
            reasoned(&items, &needs),
            vec![
                mine(
                    "Zweihander 3os",
                    Spot::Inventory,
                    Action::Sell,
                    Some("two-handed"),
                    1
                ),
                mine("Broad Sword 3os", Spot::Page(2), own, None, 2),
            ]
        );
        // A better one-handed 3os sword (War Sword, 8-20 over 7-14) replaces one Broad Sword.
        let better = [
            at(base("bsd", 3), Spot::Page(2)),
            at(base("bsd", 3), Spot::Page(2)),
            at(base("9fb", 3), Spot::Inventory),
            at(base("wsd", 3), Spot::Inventory),
        ];
        assert_eq!(
            reasoned(&better, &needs),
            vec![
                mine("Broad Sword 3os", Spot::Page(2), Action::Sell, None, 1),
                mine(
                    "Zweihander 3os",
                    Spot::Inventory,
                    Action::Sell,
                    Some("two-handed"),
                    1
                ),
                mine("War Sword 3os", Spot::Inventory, Action::Move(Side::Own), None, 1),
                mine("Broad Sword 3os", Spot::Page(2), own, None, 1),
            ]
        );
        // On the map: a Zweihander 3os is no Lawbringer base, a War Sword 3os is.
        let (_, wants) = plan(&items, &needs, &druid());
        assert_eq!(wants.base(&base("9fb", 3)), None);
        assert_eq!(wants.base(&base("wsd", 3)), Some("Lawbringer"));
        assert_eq!(wants.base(&base("bsd", 3)), None, "no better than the kept ones");
        Ok(())
    }

    #[test]
    fn a_two_handed_base_without_any_offhand_stays_a_candidate() {
        // No off-hand in the stage: the weapon slot takes both hands, the Zweihander wins.
        let mut needs = Needs::default();
        let slots: BTreeMap<String, Vec<String>> =
            [("weapon".to_owned(), vec!["Lawbringer@43".to_owned()])].into();
        needs.slots(&slots, DRUID);
        let items = [
            at(base("bsd", 3), Spot::Page(2)),
            at(base("9fb", 3), Spot::Page(1)),
        ];
        assert_eq!(
            rows(&items, &needs),
            vec![
                row("Broad Sword 3os", Spot::Page(2), Action::Sell, 1),
                row("Zweihander 3os", Spot::Page(1), Action::Keep(Side::Own), 1),
            ]
        );
        // With an off-hand piece: the Broad Sword, and the Zweihander says why it goes.
        let mut needs = Needs::default();
        let mut slots = slots;
        slots.insert("offhand".to_owned(), vec!["Ancients' Pledge@21".to_owned()]);
        needs.slots(&slots, DRUID);
        assert_eq!(
            reasoned(&items, &needs),
            vec![
                (
                    "Zweihander 3os".to_owned(),
                    Spot::Page(1),
                    Action::Sell,
                    Some("two-handed"),
                    1
                ),
                (
                    "Broad Sword 3os".to_owned(),
                    Spot::Page(2),
                    Action::Keep(Side::Own),
                    None,
                    1
                ),
            ]
        );
    }
}

//! Keep / move / stow / sell / give-away verdicts for the player's items: shared pages 1–2 are
//! own, 3–4 trade, 5 ignored. The rules are in `PLAN.md` ("Item places: sell / move / keep").

use std::cmp::Reverse;
use std::collections::BTreeMap;

pub use d2r_core::guidance::{Action, Advice, Side, Spot};

use crate::charms::CharmWish;
use crate::config::{Bis, Build};
use crate::config::{FarmGroup, FarmItem, FarmKind, Settings};
use crate::gear_score::{self, Piece, Profile};
use crate::valuables::{self, Tier, Verdict};
use crate::web::farm::{FarmStatus, farm_status};
use d2r_core::game::{ItemDrop, Place, Snapshot};
use d2r_core::guidance::Tab;
use d2r_core::item_stats::stat;
use d2r_core::items::DropKind;
use d2r_core::names::same_name;
use d2r_core::offsets::item_data;
use d2r_core::runewords_data::{self, Runeword};
use d2r_core::socket_bases::{self, MercAct, WeaponRule, Wearer};
use d2r_core::stash::{self, Holdings, StashTab, TabKind};

/// The last shared-stash page for the player's own items; the pages after it are for trade.
const LAST_OWN_PAGE: u8 = 2;

/// The player's "ignore page": its items get no verdict and nothing moves there, but they still
/// count as held ([`Snapshot::holdings`]).
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
    pub name: String,
}

/// The items of `snap` the plan judges: carried and personal-stash items, and every item on the
/// shared stash's normal pages (session-cached: the live shared items carry no page).
#[must_use]
pub fn held(snap: &Snapshot) -> Vec<Held> {
    held_from(&snap.owned_items, &snap.stash)
}

pub(crate) fn held_from(owned: &[(ItemDrop, Place)], tabs: &[StashTab]) -> Vec<Held> {
    carried(owned).chain(on_pages(tabs)).collect()
}

fn carried(owned: &[(ItemDrop, Place)]) -> impl Iterator<Item = Held> + '_ {
    owned.iter().filter_map(|(item, place)| {
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
    })
}

fn on_pages(tabs: &[StashTab]) -> impl Iterator<Item = Held> + '_ {
    tabs.iter()
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
        })
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

/// What the magic / rare / crafted verdicts need.
#[derive(Debug, Clone)]
pub struct Scoring {
    pub profile: Profile,
    /// The farm list's `kind = "rolled"` entries still wanted (a Teleport one only while no
    /// Enigma is held): a matching item is kept for own use.
    pub rolled: Vec<FarmItem>,
    /// The charm wishlists of the chain's builds: a matching charm is kept for own use.
    pub charms: Vec<CharmWish>,
    /// The money farming switch ([`crate::config::Valuables::money`]): off, the money rules
    /// keep nothing and such items are judged like any other.
    pub money: bool,
}

impl Default for Scoring {
    fn default() -> Self {
        Self {
            profile: Profile::default(),
            rolled: Vec::new(),
            charms: Vec::new(),
            money: true,
        }
    }
}

impl Scoring {
    /// The scoring for `snap`'s character and the builds of the stage `chain`
    /// ([`Settings::stage_chain`]). Level, strength and dexterity are left out: keeping an item
    /// doesn't need it wearable today.
    #[must_use]
    pub fn new(snap: &Snapshot, config: &Settings, chain: &[String]) -> Self {
        let class = snap.player.class;
        let builds: Vec<&Build> = chain_builds(config, chain).collect();
        let weapon = worn_weapon_kind(snap);
        let teleport = gear_score::teleport_wanted(&snap.holdings());
        Self {
            profile: Profile {
                class,
                level: None,
                strength: None,
                dexterity: None,
                skills: planned_skills(
                    snap.skills.points.iter().map(|&(id, _)| id).collect(),
                    &builds,
                    class,
                ),
                weapon,
                caster: gear_score::is_caster(class, weapon),
                teleport,
            },
            rolled: config
                .farm
                .iter()
                .flat_map(|g| &g.items)
                .filter(|i| i.kind == FarmKind::Rolled && (teleport || !i.wants_teleport()))
                .cloned()
                .collect(),
            charms: builds.iter().flat_map(|b| b.charms.iter().cloned()).collect(),
            money: config.valuables.money,
        }
    }
}

fn build_def<'a>(config: &'a Settings, name: &str) -> Option<&'a Build> {
    config.build_defs.get(&name.to_lowercase())
}

fn chain_builds<'a>(config: &'a Settings, chain: &'a [String]) -> impl Iterator<Item = &'a Build> {
    chain.iter().filter_map(|name| build_def(config, name))
}

fn worn_weapon_kind(snap: &Snapshot) -> Option<&'static str> {
    snap.equipped_items
        .iter()
        .find(|e| e.body == 4)
        .and_then(|e| d2r_core::bases_data::base(e.code.as_deref()?))
        .map(|b| b.kind)
}

/// The character's `allocated` skills plus every skill the `builds` plan for `class`.
fn planned_skills(allocated: Vec<u16>, builds: &[&Build], class: u32) -> Vec<u16> {
    let mut skills = allocated;
    for target in builds.iter().flat_map(|b| &b.skill_plan) {
        let planned = d2r_core::skills_data::SKILLS
            .iter()
            .find(|s| u32::from(s.class) == class && same_name(s.name, &target.skill));
        if let Some(s) = planned
            && !skills.contains(&s.id)
        {
            skills.push(s.id);
        }
    }
    skills
}

/// One runeword the player's builds want made, in a base for `wearer` (or `also`) worn in
/// `slot` (`ItemData::BODY` numbering; `None` = any slot).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BaseNeed {
    word: &'static Runeword,
    wearer: Wearer,
    slot: Option<u8>,
    /// An alternative mercenary the same copy is swapped to ([`Needs::share`]).
    also: Option<Wearer>,
    /// The build's rule for the player's `weapon` slot (the default elsewhere).
    rule: WeaponRule,
}

impl BaseNeed {
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

/// `slots` of `build` without the pieces whose leveling window (`setup = "lvl a-b"`) ends
/// below the character's `level`; a rune never outgrows, and a slot left empty leaves.
fn current(
    slots: &BTreeMap<String, Vec<String>>,
    build: Option<&Build>,
    level: Option<u8>,
) -> BTreeMap<String, Vec<String>> {
    let outgrown = |entry: &str| {
        let info = build.and_then(|b| b.pieces.get(piece_name(entry)));
        info.zip(level).is_some_and(|(info, level)| {
            info.kind != "rune"
                && crate::web::level_range(&info.setup).is_some_and(|(_, to)| u32::from(level) > to)
        })
    };
    slots
        .iter()
        .map(|(slot, pieces)| {
            let left: Vec<String> = pieces.iter().filter(|e| !outgrown(e)).cloned().collect();
            (slot.clone(), left)
        })
        .filter(|(_, pieces)| !pieces.is_empty())
        .collect()
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

    /// Wants a base for `entry` under `rule` if it names a runeword; whether it does.
    fn base(&mut self, entry: &str, wearer: Wearer, slot: Option<u8>, rule: WeaponRule) -> bool {
        let Some(word) = runewords_data::lookup(piece_name(entry)) else {
            return false;
        };
        let need = BaseNeed {
            word,
            wearer,
            slot,
            also: None,
            rule,
        };
        if !self.bases.contains(&need) {
            self.bases.push(need);
        }
        true
    }

    /// Every piece of build `slots` (slot name → pieces) and a base for each runeword in it,
    /// the `weapon` slot's under `rule`.
    fn slots(&mut self, slots: &BTreeMap<String, Vec<String>>, wearer: Wearer, rule: WeaponRule) {
        for entry in slots.values().flatten() {
            self.piece(entry, 1);
        }
        self.slot_bases(slots, wearer, rule, None);
    }

    /// A base for each runeword in build `slots` (only runeword `only` when set) for `wearer`;
    /// whether there was any. A weapon beside off-hand pieces must be one-handed; `swap` isn't.
    /// The `weapon` slot's bases keep to `rule`.
    fn slot_bases(
        &mut self,
        slots: &BTreeMap<String, Vec<String>>,
        wearer: Wearer,
        rule: WeaponRule,
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
            let rule = if slot == "weapon" {
                rule
            } else {
                WeaponRule::default()
            };
            for entry in pieces {
                if only.is_none_or(|word| same_name(piece_name(entry), word)) {
                    any |= self.base(entry, wearer, slot_body(slot), rule);
                }
            }
        }
        any
    }

    /// Every piece of mercenary `slots` and a base for each runeword in it.
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
    /// `build`; whether there was any.
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
    /// does. The wearer is the mercenary of `build`'s act that can carry it (Act 2 without a
    /// build); a need of an alternative act goes to `spare` for [`Needs::share`].
    fn merc_base(
        &mut self,
        entry: &str,
        build: Option<&Build>,
        slot: Option<u8>,
        spare: &mut Vec<BaseNeed>,
    ) -> bool {
        let act = build.map_or_else(MercAct::default, |b| b.merc_wearer(entry, slot));
        if build.is_none_or(|b| b.merc_act() == act) {
            return self.base(entry, Wearer::Merc(act), slot, WeaponRule::default());
        }
        let Some(word) = runewords_data::lookup(piece_name(entry)) else {
            return false;
        };
        spare.push(BaseNeed {
            word,
            wearer: Wearer::Merc(act),
            slot,
            also: None,
            rule: WeaponRule::default(),
        });
        true
    }

    /// Adds the alternative mercenaries' needs (`spare`). One that a need of the same runeword
    /// already covers adds no second base (one copy is swapped between them); that need then
    /// also takes the bases only the alternative mercenary can use.
    fn share(&mut self, spare: Vec<BaseNeed>) {
        for need in spare {
            match self.same_word_need(&need).and_then(|i| self.bases.get_mut(i)) {
                Some(n) => {
                    if n.wearer != need.wearer && n.also.is_none() {
                        n.also = Some(need.wearer);
                    }
                }
                None => self.bases.push(need),
            }
        }
    }

    /// The need of `need`'s runeword in the same slot, else a slot-less one.
    fn same_word_need(&self, need: &BaseNeed) -> Option<usize> {
        let at = |slot_less: bool| {
            move |n: &BaseNeed| {
                n.word.name == need.word.name && (n.slot == need.slot || (slot_less && n.slot.is_none()))
            }
        };
        self.bases
            .iter()
            .position(at(false))
            .or_else(|| self.bases.iter().position(at(true)))
    }

    /// The player's own needs from the resolved `bis`, every build of the stage `chain`
    /// ([`Settings::stage_chain`]) and the farm list; a player piece is for `class` unless its
    /// build names one. A build's piece whose leveling window ends below the character's
    /// `level` is no need of that build. Pieces `worn` by the character or its mercenary need
    /// no stash copy, and each copy `owns` counts drops one base need of that runeword (and
    /// meets `switch_requires` groups).
    #[must_use]
    pub fn from_config(
        config: &Settings,
        chain: &[String],
        bis: Option<&Bis>,
        class: u32,
        level: Option<u8>,
        worn: &[String],
        owns: &dyn Fn(&str) -> u32,
    ) -> Self {
        let mut needs = Self::default();
        let mut spare = Vec::new();
        if let Some(bis) = bis {
            let def = build_def(config, &bis.build);
            let rule = def.map_or_default(|b| b.weapon_rule);
            needs.slots(&current(&bis.slots, def, level), Wearer::Player(class), rule);
            needs.merc_slots(&current(&bis.merc_slots, def, level), def, &mut spare);
        }
        for build in chain_builds(config, chain) {
            needs.build(build, class, level, &mut spare);
            needs.switch_gear(config, build, class, worn, owns);
        }
        for group in &config.farm {
            needs.farm_group(group, build_def(config, &group.target), class, &mut spare);
        }
        needs.share(spare);
        needs.drop_slotless_placed();
        needs.subtract_worn(worn);
        needs.drop_made_bases(owns);
        needs
    }

    /// The slots (those the character at `level` hasn't outgrown), extras and unique charm
    /// wishes of `build`.
    fn build(&mut self, build: &Build, class: u32, level: Option<u8>, spare: &mut Vec<BaseNeed>) {
        let player = Wearer::Player(build.class.unwrap_or(class));
        self.slots(
            &current(&build.slots, Some(build), level),
            player,
            build.weapon_rule,
        );
        self.merc_slots(
            &current(&build.merc_slots, Some(build), level),
            Some(build),
            spare,
        );
        for entry in build.extras.values().flatten() {
            self.piece(entry, 1);
            self.base(entry, player, None, WeaponRule::default());
        }
        for wish in build.charms.iter().filter(|w| w.unique) {
            self.piece(&wish.name, wish.count);
        }
    }

    /// The `switch_requires` items of `build`, without those marked "not needed" for it or for
    /// the stage it switches to. A one-item group wants its copies and a base. An any-of group
    /// wants each candidate as a piece but no base (that would keep a base for every
    /// alternative runeword); once the held copies (`owns`) meet it, only the group's count of
    /// held candidates: the `worn` ones first, then in the group's order.
    fn switch_gear(
        &mut self,
        config: &Settings,
        build: &Build,
        class: u32,
        worn: &[String],
        owns: &dyn Fn(&str) -> u32,
    ) {
        let player = Wearer::Player(build.class.unwrap_or(class));
        let marks_of = |slug: &str| config.not_needed.get(slug).map_or(&[][..], Vec::as_slice);
        let own = marks_of(&build.name.to_lowercase());
        let next = build.next.as_deref().map_or(&[][..], marks_of);
        let marked =
            |item: &str| crate::not_needed::covers(own, item) || crate::not_needed::covers(next, item);
        let worn_copies = |item: &str| {
            let on = worn.iter().filter(|w| same_name(w, item)).count();
            u32::try_from(on).unwrap_or(u32::MAX)
        };
        for group in &build.switch_requires {
            let candidates: Vec<&String> = group.any.iter().filter(|item| !marked(item)).collect();
            if group.any.len() == 1 {
                for item in candidates {
                    self.piece(item, group.count);
                    self.base(item, player, None, WeaponRule::default());
                }
            } else if group.met(owns) {
                let mut left = group.count;
                let mut taken: Vec<(&str, u32)> = Vec::new();
                let worn_first = candidates
                    .iter()
                    .map(|item| (item.as_str(), worn_copies(item)))
                    .chain(
                        candidates
                            .iter()
                            .map(|item| (item.as_str(), owns(item).saturating_sub(worn_copies(item)))),
                    );
                for (item, copies) in worn_first {
                    let take = copies.min(left);
                    left -= take;
                    match taken.iter_mut().find(|(n, _)| *n == item) {
                        Some((_, t)) => *t += take,
                        None => taken.push((item, take)),
                    }
                }
                for (item, count) in taken.into_iter().filter(|&(_, c)| c > 0) {
                    self.piece(item, count);
                }
            } else {
                for item in candidates {
                    self.piece(item, 1);
                }
            }
        }
    }

    /// The named items of farm `group`. A runeword its `target` build places takes that
    /// build's base needs, else one from the entry's own slot and wearer.
    fn farm_group(
        &mut self,
        group: &FarmGroup,
        target: Option<&Build>,
        class: u32,
        spare: &mut Vec<BaseNeed>,
    ) {
        let player = Wearer::Player(target.and_then(|b| b.class).unwrap_or(class));
        // A rolled entry names no item: [`farm_entry`] keeps its copies by their stats.
        for item in group.items.iter().filter(|i| i.kind != FarmKind::Rolled) {
            self.piece(&item.name, item.count);
            if target.is_some_and(|build| self.placed_by(build, player, &item.name, spare)) {
                continue;
            }
            let slot = item.slot.map(crate::config::FarmSlot::body);
            if item.merc {
                self.merc_base(&item.name, target, slot, spare);
            } else {
                self.base(&item.name, player, slot, WeaponRule::default());
            }
        }
    }

    /// Wants the bases `build` places runeword `name` in, for the player and the mercenary;
    /// whether it places it anywhere.
    fn placed_by(&mut self, build: &Build, player: Wearer, name: &str, spare: &mut Vec<BaseNeed>) -> bool {
        let placed = self.slot_bases(&build.slots, player, build.weapon_rule, Some(name));
        let merc_placed = self.merc_bases(&build.merc_slots, Some(build), Some(name), spare);
        placed || merc_placed
    }

    /// Drops the slot-less needs of a runeword some build places in a slot: a two-hander could
    /// fill them where the slot wants one hand.
    fn drop_slotless_placed(&mut self) {
        let placed: Vec<&str> = self
            .bases
            .iter()
            .filter(|n| n.slot.is_some())
            .map(|n| n.word.name)
            .collect();
        self.bases
            .retain(|n| n.slot.is_some() || !placed.contains(&n.word.name));
    }

    fn subtract_worn(&mut self, worn: &[String]) {
        for (name, count) in &mut self.pieces {
            let on = worn.iter().filter(|w| same_name(w, name)).count();
            *count = count.saturating_sub(u32::try_from(on).unwrap_or(u32::MAX));
        }
    }

    fn drop_made_bases(&mut self, owns: &dyn Fn(&str) -> u32) {
        let mut held: Vec<(&str, u32)> = Vec::new();
        self.bases.retain(|need| {
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
    }
}

/// A white (low, normal or superior) item without a runeword whose code is a socketable
/// weapon or armor base.
pub fn plain_base(item: &ItemDrop) -> bool {
    d2r_core::game::maybe_white_base(item.quality, item.flags)
        && item.code.as_deref().is_some_and(socket_bases::socketable)
}

/// What the valuables rules keep for trade: a certain S or A match, or an uncertain S; the
/// money rules only when `money` (the money farming switch). A white base matching only a
/// twink A rule is own gear, not a trade good.
fn trade_worthy(item: &ItemDrop, money: bool) -> bool {
    let mut tiers = valuables::keep_tiers(money);
    if plain_base(item) {
        tiers.twink.retain(|&t| t == Tier::S);
    }
    valuables::best(item, &tiers).is_some_and(|h| !h.uncertain || h.tier == Tier::S)
}

/// [`base_rank`]'s key: power, grade, requirement (lower is better).
type Rank = (u32, u8, Reverse<u32>);

/// How good a runeword base is, better is greater: base damage (weapons) or maximum defense
/// (armor), then superior or ethereal over normal over low, then the lower str + dex requirement.
fn base_rank(item: &ItemDrop) -> Rank {
    let code = item.code.as_deref().unwrap_or_default();
    let raw = d2r_core::weapons_data::damage(code)
        .or_else(|| socket_bases::max_defense(code).map(u32::from))
        .unwrap_or(0);
    // Ethereal bases have +50% base damage / defense in game.
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
                .any(|w| socket_bases::fits(need.word, code, w, need.slot) && need.rule.allows(code, w))
        })
}

/// The stash tab `h` belongs in when it is a gem, rune or material; `None` for anything else,
/// in the cube (a recipe in progress), and for a rejuvenation potion in the inventory (drunk
/// from there).
fn stow(h: &Held) -> Option<Tab> {
    let code = h.item.code.as_deref()?;
    let potion = matches!(code, "rvs" | "rvl");
    stash::tab_of(code).filter(|_| h.spot != Spot::Cube && !(potion && h.spot == Spot::Inventory))
}

/// An item the plan leaves alone: charms in the inventory (they work there), finished
/// runewords that aren't a build piece, and gems, runes and materials [`stow`] doesn't move.
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
/// the personal stash.
const fn keep_or_move(side: Side, spot: Spot) -> Action {
    match (spot, page_side(spot), side) {
        (Spot::Stash, _, Side::Own)
        | (_, Some(Side::Own), Side::Own)
        | (_, Some(Side::Trade), Side::Trade) => Action::Keep(side),
        _ => Action::Move(side),
    }
}

/// The indices of `items`, best place for `side` first (then in input order), without the
/// ignored page's.
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

/// The item plan of `snap`'s character ([`plan`]) for the resolved `bis`, the stage `chain` and
/// the farm list, and what a ground drop would still add ([`Wants`]).
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
    let needs = Needs::from_config(
        config,
        chain,
        bis,
        snap.player.class,
        snap.player.char_level,
        &worn,
        &owns,
    );
    let (advice, mut wants) = plan(&held(snap), &needs, &Scoring::new(snap, config, chain));
    for piece in &mut wants.pieces {
        piece.farm = farm_missing(config, &holdings, &piece.name);
    }
    (advice, wants)
}

/// Whether `name` is a farm-list unique or set nobody holds yet (the farm panel's "missing").
fn farm_missing(config: &Settings, holdings: &Holdings<'_>, name: &str) -> bool {
    config.farm.iter().flat_map(|g| &g.items).any(|item| {
        matches!(item.kind, FarmKind::Unique | FarmKind::Set) && same_name(&item.name, name) && {
            let (stashed, carried) = holdings.copies(&item.name, item.kind.item_kind());
            farm_status(item.count, stashed, carried) == FarmStatus::Missing
        }
    })
}

/// What a ground drop would add to the item plan: a better base for an open runeword need, or
/// a unique or set build piece still wanted. Magic and rare drops' stats aren't known before
/// pickup.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Wants {
    /// Every runeword base need with the rank of the base kept for it.
    bases: Vec<(BaseNeed, Option<Rank>)>,
    pieces: Vec<WantedPiece>,
}

/// A unique or set piece of [`Wants`].
#[derive(Debug, Clone, PartialEq, Eq)]
struct WantedPiece {
    name: String,
    /// Quality and base code of each row of that name: what an unidentified copy looks like.
    bases: &'static [(u32, &'static str)],
    /// A still-missing farm-list entry.
    farm: bool,
}

impl Wants {
    /// The wants left after the plan kept the bases `kept` (per need) and left `wanted` copies
    /// (per piece); farm flags unset.
    fn left(items: &[Held], needs: &Needs, kept: Vec<Option<usize>>, wanted: &[u32]) -> Self {
        Self {
            bases: needs
                .bases
                .iter()
                .zip(kept)
                .map(|(need, k)| (*need, k.and_then(|i| items.get(i)).map(|h| base_rank(&h.item))))
                .collect(),
            pieces: needs
                .pieces
                .iter()
                .zip(wanted)
                .filter(|&((name, _), &left)| left > 0 && runewords_data::lookup(name).is_none())
                .map(|((name, _), _)| WantedPiece {
                    name: name.clone(),
                    bases: d2r_core::names::named_bases_cached(name),
                    farm: false,
                })
                .collect(),
        }
    }

    /// The wanted piece `drop` is: `(rule, farm entry, uncertain)`. An unidentified one matches
    /// by quality and base code, uncertain: the rule names every wanted piece on that base with
    /// a "?" ("Raven Frost? / Nagelring?"), only the farm entries when any is one.
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

    /// The runeword whose open base need `drop` fills better than the base kept for it; a
    /// trade-worthy base is no own base (`money`: the money farming switch).
    #[must_use]
    pub fn base(&self, drop: &ItemDrop, money: bool) -> Option<&'static str> {
        if !plain_base(drop) {
            return None;
        }
        let rank = base_rank(drop);
        let (need, _) = self
            .bases
            .iter()
            .find(|(need, kept)| fills(need, drop) && kept.as_ref().is_none_or(|k| rank > *k))?;
        // Last: the rules lookup is the costly part.
        (!trade_worthy(drop, money)).then_some(need.word.name)
    }
}

/// The reason a plain base is sold: "two-handed" when it would fill a runeword need but for
/// its hands, else none.
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

/// An item's action with its reason ("not on the farm list", "Teleport").
type Decided = (Action, Option<&'static str>);

/// Keeps the copies of build pieces still wanted, best placed first; they win over trade.
/// Returns the copies left wanted per piece and the keep-one names kept.
fn keep_build_pieces(
    items: &[Held],
    needs: &Needs,
    order: &[usize],
    actions: &mut [Option<Decided>],
) -> (Vec<u32>, Vec<&'static str>) {
    let mut kept_one: Vec<&str> = Vec::new();
    let mut wanted: Vec<u32> = needs.pieces.iter().map(|&(_, c)| c).collect();
    for &i in order {
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
    (wanted, kept_one)
}

/// One base per runeword need, the needs with the fewest fitting bases first (a base that fits
/// two needs must not starve the narrower one), each taking its best free base (on a tie the
/// best placed, `order`). A trade-worthy base is left for the trade side (`money`: the money
/// farming switch).
fn assign_bases(
    items: &[Held],
    needs: &Needs,
    order: &[usize],
    actions: &mut [Option<Decided>],
    money: bool,
) -> Vec<Option<usize>> {
    let candidates: Vec<usize> = order
        .iter()
        .copied()
        .filter(|&i| {
            items
                .get(i)
                .is_some_and(|h| plain_base(&h.item) && !trade_worthy(&h.item, money))
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

/// The action for an item that is no build piece or kept base; none for one to identify first
/// and for a magic, rare or crafted item whose stats are unread. Such an item is kept only as a
/// farm-list copy or by a valuables trade rule, else sold.
fn decide_rest(
    h: &Held,
    needs: &Needs,
    scoring: &Scoring,
    kept_one: &mut Vec<&'static str>,
) -> Option<Decided> {
    if let Some(tab) = stow(h) {
        return Some((Action::Stow(tab), None));
    }
    if valuables::worth_identifying(&h.item, scoring.money) {
        return None;
    }
    if magic_charm(&h.item) {
        return charm_action(h, scoring);
    }
    if let Some(entry) = farm_entry(&h.item, scoring) {
        let reason = entry.needs.first().map(|n| n.name());
        return Some((keep_or_move(Side::Own, h.spot), reason));
    }
    if trade_worthy(&h.item, scoring.money) {
        let action = match valuables::keep_one_name(&h.item) {
            Some(name) if kept_one.contains(&name) => Action::GiveAway,
            one => {
                kept_one.extend(one);
                keep_or_move(Side::Trade, h.spot)
            }
        };
        return Some((action, None));
    }
    if plain_base(&h.item) {
        return Some((Action::Sell, base_sell_reason(&h.item, needs)));
    }
    if d2r_core::game::magic_or_better(h.item.quality) {
        return judgeable(&h.item).then_some((Action::Sell, Some("not on the farm list")));
    }
    let action = match valuables::verdict(&h.item)? {
        Verdict::Sell => Action::Sell,
        Verdict::GiveAway => Action::GiveAway,
    };
    Some((action, None))
}

/// The farm list's rolled entry `item` is a copy of ([`FarmItem::rolled_match`]).
fn farm_entry<'s>(item: &ItemDrop, scoring: &'s Scoring) -> Option<&'s FarmItem> {
    if !d2r_core::game::magic_or_better(item.quality) || !judgeable(item) {
        return None;
    }
    scoring
        .rolled
        .iter()
        .find(|entry| entry.rolled_match(item.code.as_deref(), &item.stats))
}

/// Identified with its stats read (a real magic / rare item always has affix stats).
const fn judgeable(item: &ItemDrop) -> bool {
    item.flags & item_data::FLAG_IDENTIFIED != 0 && !item.stats.is_empty()
}

/// A magic (not unique) charm.
const fn magic_charm(item: &ItemDrop) -> bool {
    matches!(item.kind, DropKind::Charm { unique: false, .. }) && item.quality == item_data::QUALITY_MAGIC
}

/// The action for a stored magic charm (user rule, 2026-11-22): own use when a charm wish of
/// the chain matches it or the builds use it, else a trade keep when a valuables rule wants
/// it, else sold. None while its stats are unread.
fn charm_action(h: &Held, scoring: &Scoring) -> Option<Decided> {
    if !judgeable(&h.item) {
        return None;
    }
    let wished = scoring
        .charms
        .iter()
        .any(|wish| wish.matches(&h.item, Some(scoring.profile.class)));
    Some(if wished || own_charm(&h.item, &scoring.profile) {
        (keep_or_move(Side::Own, h.spot), None)
    } else if trade_worthy(&h.item, scoring.money) {
        (keep_or_move(Side::Trade, h.spot), None)
    } else {
        (Action::Sell, Some("no keeper stats"))
    })
}

/// Whether the builds use `charm`: a skiller of a tab they plan skills on (any other skiller is
/// a trade good); else a valuables keeper whose gear score for `who` reaches
/// [`gear_score::MIN_GAIN`] (life, resistances, FHR, max damage for a fighter, mana for a
/// caster), or any score for an own-use rule's keeper (a 6% MF small charm).
fn own_charm(charm: &ItemDrop, who: &Profile) -> bool {
    let mut tabs = charm.stats.iter().filter(|s| s.id == stat::SKILL_TAB).peekable();
    if tabs.peek().is_some() {
        return tabs.any(|s| who.plans_tab(s.param));
    }
    let piece = Piece {
        code: charm.code.as_deref(),
        stats: &charm.stats,
    };
    let (total, _) = gear_score::scores(piece, who);
    match valuables::stat_keeper(charm) {
        Some(valuables::Keeper::Trade) => total >= gear_score::MIN_GAIN,
        Some(valuables::Keeper::Own) => total > 0.0,
        None => false,
    }
}

fn merge_advice(items: &[Held], actions: Vec<Option<Decided>>) -> Vec<Advice> {
    let mut out: Vec<Advice> = Vec::new();
    for (h, action) in items.iter().zip(actions) {
        let Some((action, reason)) = action else { continue };
        match out
            .iter_mut()
            .find(|a| a.action == action && a.spot == h.spot && a.name == h.name && a.reason == reason)
        {
            Some(a) => {
                a.count = a.count.saturating_add(1);
                a.cells.extend(h.item.cell);
            }
            None => out.push(Advice {
                name: h.name.clone(),
                spot: h.spot,
                action,
                reason,
                count: 1,
                cells: h.item.cell.into_iter().collect(),
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

/// The action for each of `items` as dashboard lines, and the [`Wants`] left after it (farm
/// flags unset: [`for_snapshot`] sets them).
#[must_use]
pub fn plan(items: &[Held], needs: &Needs, scoring: &Scoring) -> (Vec<Advice>, Wants) {
    let mut actions: Vec<Option<Decided>> = vec![None; items.len()];
    let own_order = by_preference(items, Side::Own);
    let (wanted, mut kept_one) = keep_build_pieces(items, needs, &own_order, &mut actions);
    let kept = assign_bases(items, needs, &own_order, &mut actions, scoring.money);
    let wants = Wants::left(items, needs, kept, &wanted);
    for i in by_preference(items, Side::Trade) {
        let (Some(h), Some(slot)) = (items.get(i), actions.get_mut(i)) else {
            continue;
        };
        if slot.is_none() && !left_alone(h) {
            *slot = decide_rest(h, needs, scoring, &mut kept_one);
        }
    }
    (merge_advice(items, actions), wants)
}

#[cfg(test)]
mod tests {
    use super::*;
    use d2r_core::item_stats::ItemStat;

    const DRUID: Wearer = Wearer::Player(5);

    /// A white item of `code` with `sockets`, not ethereal.
    fn base(code: &str, sockets: u8) -> ItemDrop {
        ItemDrop {
            cell: None,
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
        needs.base("Harmony@39", DRUID, Some(4), WeaponRule::default());
        needs
    }

    #[test]
    fn merged_lines_carry_every_items_cells() {
        use d2r_core::game::CellRect;
        let at_cell = |x| ItemDrop {
            cell: CellRect::at(Some("hbw"), x, 0),
            ..base("hbw", 3)
        };
        let items = [
            at(at_cell(0), Spot::Inventory),
            at(at_cell(4), Spot::Inventory),
            at(base("hbw", 3), Spot::Inventory),
        ];
        let (advice, _) = plan(&items, &harmony_needs(), &druid());
        let cells: Vec<_> = advice.iter().map(|a| (a.count, a.cells.clone())).collect();
        assert_eq!(
            cells,
            [(
                3,
                vec![
                    CellRect {
                        x: 0,
                        y: 0,
                        w: 2,
                        h: 3
                    },
                    CellRect {
                        x: 4,
                        y: 0,
                        w: 2,
                        h: 3
                    }
                ]
            )],
            "three 3os bows sold in one line; the one without a cell adds none"
        );
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
        // Eth 4os Colossus Voulge is S twink (trade); Crystal Sword 4os only A twink (own or sold).
        let mut voulge = base("7vo", 4);
        voulge.ethereal = true;
        let mut needs = Needs::default();
        needs.base("Spirit", DRUID, Some(4), WeaponRule::default());
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
        Needs::from_config(&config, &[], None, 5, None, &[], &move |n| {
            u32::from(n == "Insight") * held
        })
    }

    #[test]
    fn a_twink_a_base_is_no_trade_good() {
        // Bill / Poleaxe 4os are A twink: with Insight held they are sold, not traded.
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
    fn a_store_base_the_builds_want_is_marked_keep() {
        use d2r_core::guidance::MarkKind;
        use d2r_core::offsets::stat_list;
        let mut spirit = Needs::default();
        spirit.base("Spirit", DRUID, Some(4), WeaponRule::default());
        let (_, wants) = plan(&[], &spirit, &druid());
        let socketed = |sockets: i32| {
            let mut item = d2r_core::shop::shop_item(Some("crs"), item_data::QUALITY_NORMAL, 1, 0, 0);
            item.stats = vec![ItemStat {
                id: stat_list::NUM_SOCKETS,
                param: 0,
                value: sockets,
                base: false,
            }];
            item
        };
        let shop = d2r_core::shop::Shop {
            npc_txt: 0,
            npc: None,
            items: vec![socketed(4), socketed(3)],
        };
        let marks = crate::marks::shop_marks(&shop, &wants, true);
        let rules: Vec<Option<(MarkKind, String)>> =
            marks.into_iter().map(|m| m.map(|m| (m.kind, m.rule))).collect();
        assert_eq!(
            rules,
            [Some((MarkKind::Keep, "Spirit base".to_owned())), None],
            "the socket count is the store item's stat"
        );
    }

    #[test]
    fn a_ground_base_is_wanted_only_when_better_than_the_kept_one() {
        // Spirit in the weapon slot, a Crystal Sword 4os kept on page 1.
        let mut spirit = Needs::default();
        spirit.base("Spirit", DRUID, Some(4), WeaponRule::default());
        let (_, wants) = plan(&[at(base("crs", 4), Spot::Page(1))], &spirit, &druid());
        assert_eq!(
            wants.base(&base("bsd", 4), true),
            Some("Spirit"),
            "Broad Sword beats it"
        );
        assert_eq!(
            wants.base(&base("crs", 4), true),
            None,
            "the same base: no better"
        );
        assert_eq!(wants.base(&base("bsd", 3), true), None, "wrong socket count");
        assert_eq!(wants.base(&base("crs", 5), true), None, "5os: no Spirit");
        let mut magic = base("bsd", 4);
        magic.quality = item_data::QUALITY_MAGIC;
        assert_eq!(wants.base(&magic, true), None, "a magic item takes no runeword");
        // Nothing kept yet: any fitting base is wanted.
        let (_, open) = plan(&[], &spirit, &druid());
        assert_eq!(open.base(&base("crs", 4), true), Some("Spirit"));
        // A trade-worthy base is never an own base: eth elite polearm 4os (twink S) for Insight.
        let mut insight = Needs::default();
        insight.base(
            "Insight",
            Wearer::Merc(MercAct::Act2),
            Some(4),
            WeaponRule::default(),
        );
        let (_, merc) = plan(&[], &insight, &druid());
        assert_eq!(
            merc.base(&base("9vo", 4), true),
            Some("Insight"),
            "a plain Bill 4os"
        );
        let mut eth = base("7vo", 4);
        eth.ethereal = true;
        assert_eq!(
            merc.base(&eth, true),
            None,
            "an eth Colossus Voulge 4os goes to trade"
        );
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
        // Broad Sword (7–14) beats Crystal Sword (5–15), even from a trade page.
        let mut spirit = Needs::default();
        spirit.base("Spirit", DRUID, Some(4), WeaponRule::default());
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
        fortitude.base("Fortitude", DRUID, Some(3), WeaponRule::default());
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
        // The inventory is the wrong place; the personal stash is as good as pages 1–2.
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
        // Shako is S; Sigon's Visor is keep-one; Magefist is B only.
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

    /// A druid with no builds and no farm list.
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
    fn a_rare_ring_no_rule_wants_is_sold_with_a_reason() {
        let ring = rolled("rin", QUALITY_RARE, &[(LIGHT_RADIUS, 0, 2)]);
        assert_eq!(
            judged(&[at(ring, Spot::Page(4))], &druid()),
            vec![(
                "Rare Ring".to_owned(),
                Spot::Page(4),
                Action::Sell,
                Some(NOT_FARMED)
            )]
        );
    }

    const NOT_FARMED: &str = "not on the farm list";

    #[test]
    fn good_rare_gear_no_rule_or_farm_entry_wants_is_sold() {
        let helm = rolled(
            "uap",
            QUALITY_RARE,
            &[
                (stat::CLASS_SKILLS, 5, 2),
                (stat::FCR, 0, 20),
                (stat::MAX_LIFE, 0, 40),
            ],
        );
        let gloves = rolled(
            "xlg",
            QUALITY_RARE,
            &[
                (stat::IAS, 0, 20),
                (stat::STRENGTH, 0, 15),
                (stat::RES_FIRE, 0, 30),
            ],
        );
        let boots = rolled(
            "xvb",
            item_data::QUALITY_CRAFTED,
            &[(stat::FRW, 0, 30), (stat::FHR, 0, 10), (stat::RES_COLD, 0, 40)],
        );
        let amulet = rolled("amu", QUALITY_MAGIC, &[(stat::RES_FIRE, 0, 15)]);
        let mut scoring = druid();
        scoring.profile.teleport = true;
        let items = [
            at(helm, Spot::Page(1)),
            at(gloves, Spot::Inventory),
            at(boots, Spot::Stash),
            at(amulet, Spot::Inventory),
        ];
        assert_eq!(
            judged(&items, &scoring)
                .into_iter()
                .map(|(name, _, action, reason)| (name, action, reason))
                .collect::<Vec<_>>(),
            [
                "Crafted Sharkskin Boots",
                "Magic Amulet",
                "Rare Demonhide Gloves",
                "Rare Shako"
            ]
            .map(|name| (name.to_owned(), Action::Sell, Some(NOT_FARMED)))
        );
    }

    /// The druid with the shipped farm list's rolled entries (the Teleport amulet) while no
    /// Enigma is held.
    fn druid_without_enigma() -> Scoring {
        let config = shipped();
        let rolled: Vec<FarmItem> = config
            .farm
            .iter()
            .flat_map(|g| &g.items)
            .filter(|i| i.kind == FarmKind::Rolled)
            .cloned()
            .collect();
        let mut scoring = druid();
        scoring.profile.teleport = true;
        scoring.rolled = rolled;
        scoring
    }

    #[test]
    fn the_farm_lists_teleport_amulet_is_kept_own() {
        let scoring = druid_without_enigma();
        assert!(
            scoring.rolled.iter().any(|i| i.name == "Teleport Amulet"),
            "{:?}",
            scoring.rolled
        );
        // Level 1 Teleport charges: param = 54 << 6 | 1.
        let charges = (stat::CHARGED_SKILL, (gear_score::TELEPORT << 6) | 1, 10);
        let amulet = rolled("amu", QUALITY_MAGIC, &[charges]);
        let oskill = rolled(
            "amu",
            QUALITY_RARE,
            &[(stat::NONCLASS_SKILL, gear_score::TELEPORT, 1)],
        );
        let items = [
            at(amulet.clone(), Spot::Inventory),
            at(amulet.clone(), Spot::Page(1)),
            at(oskill, Spot::Page(3)),
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
                    "Rare Amulet".to_owned(),
                    Spot::Page(3),
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
        // The entry's slot is amulet: a Teleport staff is no copy.
        let staff = rolled("sst", QUALITY_MAGIC, &[charges]);
        assert_eq!(
            judged(&[at(staff, Spot::Page(1))], &scoring),
            vec![(
                "Magic Short Staff".to_owned(),
                Spot::Page(1),
                Action::Sell,
                Some(NOT_FARMED)
            )]
        );
        // Enigma held: Scoring::new leaves the Teleport entry out, the amulet is sold.
        let mut enigma = scoring;
        enigma.profile.teleport = false;
        enigma.rolled.retain(|i| !i.wants_teleport());
        assert_eq!(
            judged(&[at(amulet, Spot::Page(1))], &enigma),
            vec![(
                "Magic Amulet".to_owned(),
                Spot::Page(1),
                Action::Sell,
                Some(NOT_FARMED)
            )]
        );
    }

    #[test]
    fn unidentified_magic_class_items_get_no_advice_until_identified() {
        let unid = |code: &str| {
            let mut d = rolled(code, QUALITY_MAGIC, &[]);
            d.flags = 0;
            at(d, Spot::Inventory)
        };
        let junk = rolled("wnd", QUALITY_MAGIC, &[(LIGHT_RADIUS, 0, 1)]);
        let plus3 = rolled("wnd", QUALITY_MAGIC, &[(stat::SKILL_TAB, 2 * 8, 3)]);
        assert_eq!(
            judged(
                &[
                    unid("wnd"),
                    unid("9kr"),
                    at(junk, Spot::Inventory),
                    at(plus3, Spot::Page(1))
                ],
                &druid()
            ),
            vec![
                (
                    "Magic Wand".to_owned(),
                    Spot::Inventory,
                    Action::Sell,
                    Some(NOT_FARMED)
                ),
                (
                    "Magic Wand".to_owned(),
                    Spot::Page(1),
                    Action::Move(Side::Trade),
                    None
                ),
            ],
            "the unidentified wand and dagger: no advice; identified, the +3 rules decide"
        );
    }

    #[test]
    fn money_farming_off_sells_a_plus3_wand() {
        let plus3 = || {
            let wand = rolled("wnd", QUALITY_MAGIC, &[(stat::SKILL_TAB, 2 * 8, 3)]);
            at(wand, Spot::Page(3))
        };
        let row = |action, reason| vec![("Magic Wand".to_owned(), Spot::Page(3), action, reason)];
        assert_eq!(judged(&[plus3()], &druid()), row(Action::Keep(Side::Trade), None));
        let off = Scoring {
            money: false,
            ..druid()
        };
        assert_eq!(judged(&[plus3()], &off), row(Action::Sell, Some(NOT_FARMED)));
    }

    #[test]
    fn a_magic_amazon_spear_is_no_money_item() {
        let mut unid = rolled("am3", QUALITY_MAGIC, &[]);
        unid.flags = 0;
        let plus3 = rolled("am3", QUALITY_MAGIC, &[(stat::SKILL_TAB, 0, 3)]);
        assert_eq!(
            judged(&[at(unid, Spot::Inventory), at(plus3, Spot::Inventory)], &druid()),
            vec![(
                "Magic Maiden Spear".to_owned(),
                Spot::Inventory,
                Action::Sell,
                Some(NOT_FARMED)
            )],
            "+3 to an Amazon tab: no trade keep (vendors pay little); unidentified: no advice"
        );
    }

    #[test]
    fn unidentified_or_unread_rares_get_no_advice() {
        let mut unid = rolled("rin", QUALITY_RARE, &[(LIGHT_RADIUS, 0, 2)]);
        unid.flags = 0;
        let unread = rolled("rin", QUALITY_RARE, &[]);
        let mut boots = rolled("xvb", QUALITY_RARE, &[]);
        boots.flags = 0;
        assert_eq!(
            judged(
                &[
                    at(unid, Spot::Page(4)),
                    at(unread, Spot::Page(1)),
                    at(boots, Spot::Inventory)
                ],
                &druid_without_enigma()
            ),
            vec![],
            "empty stats are a failed read, not a junk item"
        );
    }

    /// An identified magic charm of `code` with `stats` (id, param, value).
    fn charm(code: &str, stats: &[(u16, u16, i32)]) -> ItemDrop {
        let size = match code {
            "cm1" => d2r_core::items::CharmSize::Small,
            "cm2" => d2r_core::items::CharmSize::Large,
            _ => d2r_core::items::CharmSize::Grand,
        };
        ItemDrop {
            kind: DropKind::Charm { size, unique: false },
            ..rolled(code, QUALITY_MAGIC, stats)
        }
    }

    /// The druid of the shipped stage `stage` (its chain's planned skills), a caster or not.
    fn shipped_druid(stage: &str, caster: bool) -> Scoring {
        let config = shipped();
        let chain = config.stage_chain(stage);
        let builds: Vec<&Build> = chain_builds(&config, &chain).collect();
        Scoring {
            profile: Profile {
                class: 5,
                skills: planned_skills(Vec::new(), &builds, 5),
                caster,
                ..Profile::default()
            },
            charms: builds.iter().flat_map(|b| b.charms.iter().cloned()).collect(),
            ..Scoring::default()
        }
    }

    /// The `+1 to <tab>` stat of `class`'s tab `tab` (0..=2).
    const fn skiller(class: u16, tab: u16) -> (u16, u16, i32) {
        (stat::SKILL_TAB, class * 8 + tab, 1)
    }

    const MAX_DMG_3: [(u16, u16, i32); 3] = [
        (stat::MAX_DAMAGE, 0, 3),
        (stat::MAX_DAMAGE_2H, 0, 3),
        (stat::THROW_MAX, 0, 3),
    ];

    #[test]
    fn charms_the_druid_builds_use_are_kept_for_real() {
        let summoner = shipped_druid("summoner druid", true);
        let fury = shipped_druid("fury druid", false);
        let summoning = charm("cm3", &[skiller(5, 0)]);
        let shape_shifting = charm("cm3", &[skiller(5, 1)]);
        let combat = charm("cm3", &[skiller(3, 0)]);
        let one = |c: &ItemDrop, s: &Scoring| judged(&[at(c.clone(), Spot::Page(3))], s);
        let own = |name: &str| vec![(name.to_owned(), Spot::Page(3), Action::Move(Side::Own), None)];
        let trade = |name: &str| vec![(name.to_owned(), Spot::Page(3), Action::Keep(Side::Trade), None)];
        assert_eq!(
            one(&summoning, &summoner),
            own("Magic Grand Charm"),
            "summoner: Summoning skiller"
        );
        assert_eq!(
            one(&shape_shifting, &summoner),
            own("Magic Grand Charm"),
            "the chain's Fury stage plans Shape Shifting"
        );
        assert_eq!(
            one(&shape_shifting, &fury),
            own("Magic Grand Charm"),
            "Fury: Shape Shifting"
        );
        assert_eq!(
            one(&summoning, &fury),
            own("Magic Grand Charm"),
            "Fury: Oak Sage is Summoning"
        );
        let elemental = charm("cm3", &[skiller(5, 2)]);
        assert_eq!(
            one(&elemental, &summoner),
            own("Magic Grand Charm"),
            "summoner: Arctic Blast, Cyclone Armor"
        );
        assert_eq!(
            one(&elemental, &fury),
            trade("Magic Grand Charm"),
            "the Fury stage alone plans no Elemental"
        );
        assert_eq!(
            one(&combat, &summoner),
            trade("Magic Grand Charm"),
            "Paladin Combat skiller: a trade good"
        );

        let mut small = MAX_DMG_3.to_vec();
        small.extend([(stat::ATTACK_RATING, 0, 20), (stat::MAX_LIFE, 0, 20)]);
        assert_eq!(
            one(&charm("cm1", &small), &fury),
            own("Magic Small Charm"),
            "3/20/20: life and damage"
        );
        let res = [
            (stat::RES_FIRE, 0, 5),
            (stat::RES_COLD, 0, 5),
            (stat::RES_LIGHTNING, 0, 5),
            (stat::RES_POISON, 0, 5),
        ];
        assert_eq!(
            one(&charm("cm1", &res), &summoner),
            own("Magic Small Charm"),
            "5 all res"
        );
        let gc_10_60 = charm(
            "cm3",
            &[
                (stat::MAX_DAMAGE, 0, 10),
                (stat::MAX_DAMAGE_2H, 0, 10),
                (stat::THROW_MAX, 0, 10),
                (stat::ATTACK_RATING, 0, 60),
            ],
        );
        assert_eq!(
            one(&gc_10_60, &fury),
            own("Magic Grand Charm"),
            "10/60: the Fury fights"
        );
        assert_eq!(
            one(&gc_10_60, &summoner),
            own("Magic Grand Charm"),
            "10/60: the chain's Fury stage wishes for one"
        );
        let caster = Scoring {
            charms: Vec::new(),
            ..summoner
        };
        assert_eq!(
            one(&gc_10_60, &caster),
            trade("Magic Grand Charm"),
            "10/60: without the wishlist a caster trades it"
        );
        let gold = charm("cm1", &[(stat::GOLD_FIND, 0, 10)]);
        assert_eq!(
            one(&gold, &fury),
            trade("Magic Small Charm"),
            "gold find: no build stat"
        );
    }

    #[test]
    fn a_charm_the_wishlist_wants_is_kept_for_own_use() -> anyhow::Result<()> {
        // Gold find is no build stat: a trade good, unless a wish names it.
        let fury = shipped_druid("fury druid", false);
        let gold = || at(charm("cm1", &[(stat::GOLD_FIND, 0, 10)]), Spot::Page(3));
        let row = |action| [("Magic Small Charm".to_owned(), Spot::Page(3), action, None)];
        assert_eq!(judged(&[gold()], &fury), row(Action::Keep(Side::Trade)));
        let wish: CharmWish = toml::from_str(
            "name = \"Gold small charm\"\nsize = \"small\"\nstats = [{ stat = \"gold_find\", min = 10 }]",
        )?;
        let wishing = Scoring {
            charms: vec![wish],
            ..fury
        };
        assert_eq!(judged(&[gold()], &wishing), row(Action::Move(Side::Own)));
        // The unique wishes are build pieces: one copy each is wanted (Annihilus, Torch, ...).
        let needs = shipped_needs(&shipped(), "summoner druid")?;
        for name in ["Annihilus", "Hellfire Torch", "Gheed's Fortune", "Bone Break"] {
            assert!(
                needs.pieces.iter().any(|(n, c)| n == name && *c == 1),
                "{name}: {:?}",
                needs.pieces
            );
        }
        Ok(())
    }

    #[test]
    fn six_mf_is_kept_for_own_use_and_seven_for_trade() {
        let summoner = shipped_druid("summoner druid", true);
        let mf = |n| at(charm("cm1", &[(stat::MAGIC_FIND, 0, n)]), Spot::Stash);
        assert_eq!(
            judged(&[mf(6), mf(7), mf(5)], &summoner),
            vec![
                (
                    "Magic Small Charm".to_owned(),
                    Spot::Stash,
                    Action::Sell,
                    Some("no keeper stats")
                ),
                (
                    "Magic Small Charm".to_owned(),
                    Spot::Stash,
                    Action::Move(Side::Trade),
                    None
                ),
                (
                    "Magic Small Charm".to_owned(),
                    Spot::Stash,
                    Action::Keep(Side::Own),
                    None
                ),
            ],
            "5% sold, 7% to the trade pages, 6% kept"
        );
    }

    #[test]
    fn other_charms_are_sold_and_unidentified_ones_get_no_advice() {
        let fury = shipped_druid("fury druid", false);
        let large = charm("cm2", &[(stat::MAX_LIFE, 0, 35)]);
        let mut unid = charm("cm3", &[]);
        unid.flags = 0;
        let mut weak = MAX_DMG_3.to_vec();
        weak.push((stat::ATTACK_RATING, 0, 20));
        assert_eq!(
            judged(
                &[
                    at(large, Spot::Page(4)),
                    at(unid, Spot::Stash),
                    at(charm("cm1", &[(stat::RES_FIRE, 0, 8)]), Spot::Stash),
                    at(charm("cm1", &weak), Spot::Page(1)),
                    at(charm("cm1", &[(stat::MAX_LIFE, 0, 20)]), Spot::Inventory),
                ],
                &fury
            ),
            vec![
                (
                    "Magic Large Charm".to_owned(),
                    Spot::Page(4),
                    Action::Sell,
                    Some("no keeper stats")
                ),
                (
                    "Magic Small Charm".to_owned(),
                    Spot::Stash,
                    Action::Sell,
                    Some("no keeper stats")
                ),
                (
                    "Magic Small Charm".to_owned(),
                    Spot::Page(1),
                    Action::Sell,
                    Some("no keeper stats")
                ),
            ],
            "a plain 35 life large charm, 8 fire res and a lifeless 3/20: vendor trash; the \
             unidentified grand charm: identify first; the carried charm stays out"
        );
    }

    #[test]
    fn jewels_no_trade_rule_wants_are_sold() {
        let res = rolled("jew", QUALITY_MAGIC, &[(stat::RES_FIRE, 0, 8)]);
        let rare = rolled(
            "jew",
            QUALITY_RARE,
            &[(stat::IAS, 0, 15), (stat::MAX_LIFE, 0, 20)],
        );
        assert_eq!(
            judged(&[at(res, Spot::Page(2)), at(rare, Spot::Stash)], &druid()),
            vec![
                (
                    "Magic Jewel".to_owned(),
                    Spot::Page(2),
                    Action::Sell,
                    Some(NOT_FARMED)
                ),
                (
                    "Rare Jewel".to_owned(),
                    Spot::Stash,
                    Action::Sell,
                    Some(NOT_FARMED)
                ),
            ]
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
            // A recipe in progress: no advice.
            at(base("xa2", 0), Spot::Cube),
            at(base("rvl", 0), Spot::Stash),
            at(vex.clone(), Spot::Page(1)),
            at(vex, Spot::Inventory),
            at(gem, Spot::Page(4)),
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
            cell: None,
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
            None,
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
            Needs::from_config(&config, &[], None, 5, None, &[], &move |n| {
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
        // Sigon's Visor is keep-one: the own copy counts, so the trade-page copy is a give-away.
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
        // Crescent Moon takes axes or swords, Lawbringer only swords: Lawbringer gets the sword.
        let mut needs = Needs::default();
        needs.base("Crescent Moon", DRUID, None, WeaponRule::default());
        needs.base("Lawbringer", DRUID, None, WeaponRule::default());
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
                cell: None,
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
        let needs = Needs::from_config(&config, &[], None, 5, None, &[], &|n| holdings.count(n));
        assert_eq!(needs.bases.len(), 0, "Harmony on page 5 is held: no base wanted");
        assert_eq!(
            rows(&[at(base("hbw", 4), Spot::Page(1))], &needs),
            vec![row("Hunter's Bow 4os", Spot::Page(1), Action::Sell, 1)]
        );
    }

    #[test]
    fn named_boots_are_build_pieces_not_judged() {
        // Sigon's Sabot is a build piece, never a "not on the farm list" sale.
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

    /// The needs of a druid holding nothing, on the shipped chain at stage `bis_build`.
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
        Ok(Needs::from_config(
            config,
            &chain,
            Some(&bis),
            5,
            None,
            &[],
            &|_| 0,
        ))
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
        assert_eq!(
            fury.merc_acts,
            vec![MercAct::Act5, MercAct::Act2],
            "the Lawbringer merc first, the Act 2 Pride merc after Bone Break"
        );
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
            // Both are the dual-wielding Act 5 merc's; "2x Lawbringer" adds no slot-less third.
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
        // The Fury has no off-hand: its weapon-slot runewords keep both hands.
        let bis = Bis {
            build: fury.name.clone(),
            slots: fury.slots.clone(),
            merc_slots: fury.merc_slots.clone(),
        };
        let own = Needs::from_config(
            &config,
            &["fury druid".to_owned()],
            Some(&bis),
            5,
            None,
            &[],
            &|_| 0,
        );
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
    fn fury_rings_of_either_slot_are_kept_one_copy_each() -> anyhow::Result<()> {
        // The shipped Fury lists the guide's rings in ring1 (after Raven Frost) and ring2: a
        // ring named in both slots is still one wanted copy, and each held one is kept.
        let needs = shipped_needs(&shipped(), "fury druid")?;
        let rings = [
            ("Raven Frost", item_data::QUALITY_UNIQUE, 275),
            ("Nature's Peace", item_data::QUALITY_UNIQUE, 300),
            ("Dwarf Star", item_data::QUALITY_UNIQUE, 274),
            ("Wisp Projector", item_data::QUALITY_UNIQUE, 319),
            ("Nagelring", item_data::QUALITY_UNIQUE, 120),
            ("Manald Heal", item_data::QUALITY_UNIQUE, 121),
            ("Angelic Halo", item_data::QUALITY_SET, 52),
        ];
        for (name, quality, index) in rings {
            let wanted: Vec<u32> = needs
                .pieces
                .iter()
                .filter(|(n, _)| same_name(n, name))
                .map(|&(_, c)| c)
                .collect();
            assert_eq!(wanted, vec![1], "{name}");
            assert_eq!(
                rows(&[at(named("rin", quality, index), Spot::Page(2))], &needs),
                vec![row(name, Spot::Page(2), Action::Keep(Side::Own), 1)]
            );
        }
        // One copy wanted: a second Dwarf Star is not kept for own use.
        let star = || named("rin", item_data::QUALITY_UNIQUE, 274);
        let plan = rows(&[at(star(), Spot::Page(1)), at(star(), Spot::Page(2))], &needs);
        let own = plan
            .iter()
            .filter(|r| matches!(r.2, Action::Keep(Side::Own)))
            .count();
        assert_eq!(own, 1, "{plan:?}");
        Ok(())
    }

    #[test]
    fn every_form_of_bone_break_is_the_wanted_piece() -> anyhow::Result<()> {
        // Row 405 is the old Bone Break, 431 the Latent one that drops, 436 the Renewed one.
        let needs = shipped_needs(&shipped(), "fury druid")?;
        let bone_break = |index| at(named("cm3", item_data::QUALITY_UNIQUE, index), Spot::Page(3));
        for (index, name) in [
            (405, "Bone Break"),
            (431, "Latent Bone Break"),
            (436, "Renewed Bone Break"),
        ] {
            assert_eq!(
                rows(&[bone_break(index)], &needs),
                vec![row(name, Spot::Page(3), Action::Move(Side::Own), 1)]
            );
        }
        // One copy wanted, whatever its form.
        let plan = rows(&[bone_break(431), bone_break(436)], &needs);
        let own = plan.iter().filter(|r| r.2 == Action::Move(Side::Own)).count();
        assert_eq!(own, 1, "{plan:?}");
        Ok(())
    }

    #[test]
    fn fury_guide_alternatives_are_kept_one_copy_each() -> anyhow::Result<()> {
        // The shipped Fury lists the Maxroll guide's pieces of every slot after the user's
        // options (and its unique charms in the extras): each held one is kept for own use.
        let config = shipped();
        let fury = config
            .build_defs
            .get("fury druid")
            .ok_or_else(|| anyhow::anyhow!("no fury druid"))?;
        let first = |slot: &str| fury.slots.get(slot).and_then(|s| s.first()).cloned();
        assert_eq!(first("weapon").as_deref(), Some("Eternity@63"), "core first");
        assert_eq!(first("helm").as_deref(), Some("Jalal's Mane@42"));
        assert_eq!(first("gloves").as_deref(), Some("Laying of Hands@63"));
        let needs = shipped_needs(&config, "fury druid")?;
        let guide = [
            ("7pa", "Tomb Reaver", item_data::QUALITY_UNIQUE, 298),
            ("uap", "Harlequin Crest", item_data::QUALITY_UNIQUE, 248),
            ("zhb", "Thundergod's Vigor", item_data::QUALITY_UNIQUE, 246),
            ("xhn", "Shaftstop", item_data::QUALITY_UNIQUE, 215),
            ("cm1", "Annihilus", item_data::QUALITY_UNIQUE, 381),
            ("xhm", "Guillaume's Face", item_data::QUALITY_SET, 104),
        ];
        for (code, name, quality, index) in guide {
            let wanted: Vec<u32> = needs
                .pieces
                .iter()
                .filter(|(n, _)| same_name(n, name))
                .map(|&(_, c)| c)
                .collect();
            assert_eq!(wanted, vec![1], "{name}");
            assert_eq!(
                rows(&[at(named(code, quality, index), Spot::Page(1))], &needs),
                vec![row(name, Spot::Page(1), Action::Keep(Side::Own), 1)]
            );
        }
        Ok(())
    }

    /// Asserts each `(code, name, quality, index)` piece is wanted once and kept for own use.
    fn kept_one_copy_each(needs: &Needs, pieces: &[(&str, &str, u32, u32)]) {
        for &(code, name, quality, index) in pieces {
            let wanted: Vec<u32> = needs
                .pieces
                .iter()
                .filter(|(n, _)| same_name(n, name))
                .map(|&(_, c)| c)
                .collect();
            assert_eq!(wanted, vec![1], "{name}");
            assert_eq!(
                rows(&[at(named(code, quality, index), Spot::Page(1))], needs),
                vec![row(name, Spot::Page(1), Action::Keep(Side::Own), 1)]
            );
        }
    }

    #[test]
    fn summoner_guide_alternatives_are_kept_one_copy_each() -> anyhow::Result<()> {
        // The shipped Summoner lists the Maxroll Summon Druid guide's pieces of every slot
        // (shields, one-handers, both rings, the merc's) after its own stage pieces.
        let config = shipped();
        let summoner = config
            .build_defs
            .get("summoner druid")
            .ok_or_else(|| anyhow::anyhow!("no summoner druid"))?;
        let first = |slots: &BTreeMap<String, Vec<String>>, slot: &str| {
            slots.get(slot).and_then(|s| s.first()).cloned()
        };
        let own = &summoner.slots;
        assert_eq!(
            first(own, "weapon").as_deref(),
            Some("Heart of the Oak@55"),
            "core first"
        );
        assert_eq!(first(own, "offhand").as_deref(), Some("Spirit@54"));
        assert_eq!(first(own, "helm").as_deref(), Some("Jalal's Mane@42"));
        assert_eq!(first(own, "ring1").as_deref(), Some("Raven Frost@45"));
        let merc = &summoner.merc_slots;
        assert_eq!(first(merc, "weapon").as_deref(), Some("Insight@27"));
        assert_eq!(first(merc, "body").as_deref(), Some("Smoke@37"));
        assert_eq!(first(merc, "helm").as_deref(), Some("Bulwark@35"));
        assert_eq!(
            merc.get("weapon").and_then(|w| w.last()).map(String::as_str),
            Some("Harmony@39"),
            "the Act 1 rogue's piece stays last"
        );
        let needs = shipped_needs(&config, "summoner druid")?;
        kept_one_copy_each(
            &needs,
            &[
                ("uit", "Stormshield", item_data::QUALITY_UNIQUE, 253),
                ("dre", "Ravenlore", item_data::QUALITY_UNIQUE, 350),
                ("rin", "The Stone of Jordan", item_data::QUALITY_UNIQUE, 122),
                ("rin", "Bul-Kathos' Wedding Band", item_data::QUALITY_UNIQUE, 268),
                ("cm1", "Annihilus", item_data::QUALITY_UNIQUE, 381),
                ("7s8", "The Reaper's Toll", item_data::QUALITY_UNIQUE, 326),
                ("xhm", "Guillaume's Face", item_data::QUALITY_SET, 104),
            ],
        );
        // The Act 2 merc's Hustle and Fortitude want polearm / body armor bases for Act 2.
        for word in ["Hustle (weapon)", "Fortitude"] {
            assert!(
                needs
                    .bases
                    .iter()
                    .any(|n| n.word.name == word && n.wearer == Wearer::Merc(MercAct::Act2)),
                "{word}"
            );
        }
        Ok(())
    }

    #[test]
    fn fury_merc_guide_alternatives_follow_the_lawbringers() -> anyhow::Result<()> {
        // The Act 5 Lawbringer merc stays first; Pride and The Reaper's Toll are the later
        // Act 2 Might merc's (after Bone Break), the armor and helms any merc wears.
        let config = shipped();
        let fury = config
            .build_defs
            .get("fury druid")
            .ok_or_else(|| anyhow::anyhow!("no fury druid"))?;
        let slot = |s: &str| fury.merc_slots.get(s).cloned().unwrap_or_default();
        assert_eq!(
            slot("weapon"),
            ["Lawbringer@43", "Pride@67", "The Reaper's Toll@75"]
        );
        assert_eq!(
            slot("helm"),
            ["Bulwark@35", "Vampire Gaze@41", "Guillaume's Face@34"]
        );
        assert_eq!(slot("body"), ["Smoke@37", "Shaftstop@38", "Treachery@43"]);
        assert!(fury.charms.iter().any(|c| c.unique && c.name == "Bone Break"));
        assert!(fury.extras.is_empty(), "the unique charms left the extras");
        assert_eq!(fury.merc_wearer("Pride@67", Some(4)), MercAct::Act2);
        assert_eq!(fury.merc_wearer("Treachery@43", Some(3)), MercAct::Act5);
        let needs = shipped_needs(&config, "fury druid")?;
        assert!(
            needs
                .bases
                .iter()
                .any(|n| n.word.name == "Pride" && n.wearer == Wearer::Merc(MercAct::Act2)),
            "{:?}",
            needs.bases
        );
        kept_one_copy_each(
            &needs,
            &[
                ("xh9", "Vampire Gaze", item_data::QUALITY_UNIQUE, 208),
                ("xhn", "Shaftstop", item_data::QUALITY_UNIQUE, 215),
            ],
        );
        Ok(())
    }

    #[test]
    fn a_piece_marked_not_needed_is_neither_kept_nor_given_a_base() -> anyhow::Result<()> {
        // User decision 2026-11-22: the player removes pieces they no longer want by hand.
        let fury_needs = |config: &Settings| -> anyhow::Result<Needs> {
            let def = config
                .build_defs
                .get("fury druid")
                .ok_or_else(|| anyhow::anyhow!("no fury druid"))?;
            let bis = Bis {
                build: def.name.clone(),
                slots: def.slots.clone(),
                merc_slots: def.merc_slots.clone(),
            };
            let chain = ["fury druid".to_owned()];
            Ok(Needs::from_config(
                config,
                &chain,
                Some(&bis),
                5,
                None,
                &[],
                &|_| 0,
            ))
        };
        let wants = |needs: &Needs, name: &str| {
            (
                needs.pieces.iter().any(|(n, c)| same_name(n, name) && *c > 0),
                needs.bases.iter().any(|b| b.word.name == name),
            )
        };
        let tomb_reaver = [at(named("7pa", item_data::QUALITY_UNIQUE, 298), Spot::Page(1))];
        let mut config = shipped();
        let needs = fury_needs(&config)?;
        assert_eq!(wants(&needs, "Grief"), (true, true));
        assert_eq!(wants(&needs, "Tomb Reaver"), (true, false));
        let kept = rows(&tomb_reaver, &needs);
        assert_eq!(
            kept,
            vec![row("Tomb Reaver", Spot::Page(1), Action::Keep(Side::Own), 1)]
        );
        let marked = crate::not_needed::NotNeeded::from([(
            "fury druid".to_owned(),
            vec!["Grief".to_owned(), "Tomb Reaver".to_owned()],
        )]);
        let mut marks = crate::not_needed::marks(&marked, &config.build_defs);
        crate::not_needed::apply(&mut marks, &mut config.build_defs, &mut config.farm);
        let needs = fury_needs(&config)?;
        assert_eq!(wants(&needs, "Grief"), (false, false), "no Grief base kept");
        assert_eq!(wants(&needs, "Tomb Reaver"), (false, false));
        assert!(
            rows(&tomb_reaver, &needs)
                .iter()
                .all(|(_, _, action, _)| *action != Action::Keep(Side::Own)),
            "falls through to the trade / sell rules: {:?}",
            rows(&tomb_reaver, &needs)
        );
        Ok(())
    }

    #[test]
    fn a_mark_made_on_the_summoner_stage_holds_for_the_fury_stage_after_it() -> anyhow::Result<()> {
        // Manald Heal is in both builds' ring slots and the Summoner's farm group; the
        // Summoner's stash plan keeps for the whole chain.
        let wanted = |needs: &Needs| {
            needs
                .pieces
                .iter()
                .any(|(n, c)| same_name(n, "Manald Heal") && *c > 0)
        };
        let mut config = shipped();
        let ring = [at(named("rin", item_data::QUALITY_UNIQUE, 121), Spot::Page(1))];
        let needs = shipped_needs(&config, "summoner druid")?;
        assert!(wanted(&needs));
        assert_eq!(
            rows(&ring, &needs),
            vec![row("Manald Heal", Spot::Page(1), Action::Keep(Side::Own), 1)]
        );
        let marked = crate::not_needed::NotNeeded::from([(
            "summoner druid".to_owned(),
            vec!["Manald Heal".to_owned()],
        )]);
        let mut marks = crate::not_needed::marks(&marked, &config.build_defs);
        crate::not_needed::apply(&mut marks, &mut config.build_defs, &mut config.farm);
        let needs = shipped_needs(&config, "summoner druid")?;
        assert!(!wanted(&needs), "{:?}", needs.pieces);
        assert!(
            rows(&ring, &needs)
                .iter()
                .all(|(_, _, action, _)| *action != Action::Keep(Side::Own)),
            "{:?}",
            rows(&ring, &needs)
        );
        Ok(())
    }

    #[test]
    fn one_harmony_base_serves_the_druid_swap_and_the_act1_rogue() -> anyhow::Result<()> {
        let config = shipped();
        let needs = shipped_needs(&config, "summoner druid")?;
        // The Fury's swap Harmony and the rogue's Harmony are one need.
        let harmony: Vec<_> = needs
            .bases
            .iter()
            .filter(|n| n.word.name == "Harmony")
            .map(|n| (n.wearer, n.slot, n.also))
            .collect();
        assert_eq!(harmony, vec![(DRUID, Some(4), Some(Wearer::Merc(MercAct::Act1)))]);
        let own = Action::Keep(Side::Own);
        let mine = |n: &str, action| (n.to_owned(), Spot::Page(1), action, 1);
        // A Stag Bow is an amazon bow: only the rogue can use it.
        assert_eq!(
            rows(&[at(base("hbw", 4), Spot::Page(1))], &needs),
            vec![mine("Hunter's Bow 4os", own)]
        );
        assert_eq!(
            rows(&[at(base("am1", 4), Spot::Page(1))], &needs),
            vec![mine("Stag Bow 4os", own)]
        );
        // Both held: one Harmony, one base (the stronger one); the other 4os bow is the
        // druid's base for Brand (the Fury's guide swap alternative), and a third is sold.
        assert_eq!(
            rows(
                &[
                    at(base("hbw", 4), Spot::Page(1)),
                    at(base("hbw", 4), Spot::Page(1)),
                    at(base("am1", 4), Spot::Page(1))
                ],
                &needs
            ),
            vec![
                mine("Hunter's Bow 4os", Action::Sell),
                mine("Hunter's Bow 4os", own),
                mine("Stag Bow 4os", own)
            ]
        );
        // The Fury stage alone has no rogue: the amazon bow is no base.
        let fury = config
            .build_defs
            .get("fury druid")
            .ok_or_else(|| anyhow::anyhow!("fury"))?;
        let bis = Bis {
            build: fury.name.clone(),
            slots: fury.slots.clone(),
            merc_slots: fury.merc_slots.clone(),
        };
        let fury_needs = Needs::from_config(
            &config,
            &["fury druid".to_owned()],
            Some(&bis),
            5,
            None,
            &[],
            &|_| 0,
        );
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
        let needs = Needs::from_config(&config, &["b".to_owned()], None, 5, None, &[], &|_| 0);
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
    fn live_two_broad_swords_kept_and_the_zweihander_is_the_hustle_base() -> anyhow::Result<()> {
        // The shipped build files must load, or every build drops and both swords are sold.
        let config = shipped();
        assert_eq!(
            config.build_defs.get("fury druid").map(|b| b.merc_acts.clone()),
            Some(vec![MercAct::Act5, MercAct::Act2])
        );
        let needs = shipped_needs(&config, "summoner druid")?;
        let items = [
            at(base("bsd", 3), Spot::Page(2)),
            at(base("bsd", 3), Spot::Page(2)),
            at(base("9fb", 3), Spot::Inventory),
        ];
        let own = Action::Keep(Side::Own);
        let mine = |n: &str, spot, action, reason, count| (n.to_owned(), spot, action, reason, count);
        // The Act 5 merc's two Lawbringer bases (one-handed swords); the Zweihander is the
        // Fury's two-handed Hustle (weapon) base, never a Lawbringer one.
        let zweihander = |action| mine("Zweihander 3os", Spot::Inventory, action, None, 1);
        assert_eq!(
            reasoned(&items, &needs),
            vec![
                zweihander(Action::Move(Side::Own)),
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
                mine("War Sword 3os", Spot::Inventory, Action::Move(Side::Own), None, 1),
                zweihander(Action::Move(Side::Own)),
                mine("Broad Sword 3os", Spot::Page(2), own, None, 1),
            ]
        );
        // On the map: a second Zweihander 3os is not needed (the Hustle base is kept), a War
        // Sword 3os is a Lawbringer base.
        let (_, wants) = plan(&items, &needs, &druid());
        assert_eq!(wants.base(&base("9fb", 3), true), None);
        assert_eq!(wants.base(&base("wsd", 3), true), Some("Lawbringer"));
        assert_eq!(
            wants.base(&base("bsd", 3), true),
            None,
            "no better than the kept ones"
        );
        Ok(())
    }

    #[test]
    fn a_two_handed_base_without_any_offhand_stays_a_candidate() {
        // No off-hand in the stage: the weapon slot takes both hands, the Zweihander wins.
        let mut needs = Needs::default();
        let slots: BTreeMap<String, Vec<String>> =
            [("weapon".to_owned(), vec!["Lawbringer@43".to_owned()])].into();
        needs.slots(&slots, DRUID, WeaponRule::default());
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
        needs.slots(&slots, DRUID, WeaponRule::default());
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

    /// The needs of a level `level` druid on the shipped `chain`, Summoner `BiS`, holding the
    /// pieces `held` (worn by its merc).
    fn druid_needs(config: &Settings, chain: &[&str], level: u8, held: &[&str]) -> anyhow::Result<Needs> {
        let def = config
            .build_defs
            .get("summoner druid")
            .ok_or_else(|| anyhow::anyhow!("no summoner druid"))?;
        let bis = Bis {
            build: def.name.clone(),
            slots: def.slots.clone(),
            merc_slots: def.merc_slots.clone(),
        };
        let chain: Vec<String> = chain.iter().map(|&c| c.to_owned()).collect();
        let worn: Vec<String> = held.iter().map(|&h| h.to_owned()).collect();
        let owns = |name: &str| u32::from(held.iter().any(|h| same_name(h, name)));
        Ok(Needs::from_config(
            config,
            &chain,
            Some(&bis),
            5,
            Some(level),
            &worn,
            &owns,
        ))
    }

    const CHAIN: [&str; 2] = ["summoner druid", "fury druid"];

    fn wants_piece(needs: &Needs, name: &str) -> bool {
        needs.pieces.iter().any(|(n, c)| same_name(n, name) && *c > 0)
    }

    #[test]
    fn a_not_needed_mark_reaches_the_switch_groups() -> anyhow::Result<()> {
        // The user's not-needed.toml (2026-11-22): Harmony and Arkaine's Valor on the Summoner.
        // Harmony is also a one-item switch group of the Summoner; marked, it wants neither the
        // piece nor a base (the switch condition itself stays).
        let mut config = shipped();
        let marked = crate::not_needed::NotNeeded::from([(
            "summoner druid".to_owned(),
            vec!["Arkaine's Valor".to_owned(), "Harmony".to_owned()],
        )]);
        let mut marks = crate::not_needed::marks(&marked, &config.build_defs);
        crate::not_needed::apply(&mut marks, &mut config.build_defs, &mut config.farm);
        config.not_needed = marks;
        let needs = druid_needs(&config, &CHAIN, 77, &[])?;
        assert!(!wants_piece(&needs, "Harmony"), "{:?}", needs.pieces);
        assert!(
            needs.bases.iter().all(|n| n.word.name != "Harmony"),
            "{:?}",
            needs.bases
        );
        let summoner = config
            .build_defs
            .get("summoner druid")
            .ok_or_else(|| anyhow::anyhow!("summoner"))?;
        assert!(summoner.switch_requires.iter().any(|g| g.any == ["Harmony"]));
        // Two 4os missile bases: one is the Fury's swap Brand base (a guide alternative, kept
        // by user decision), the other was the switch group's Harmony base and goes.
        let items = [
            at(base("6hx", 4), Spot::Page(2)),
            at(base("hbw", 4), Spot::Page(2)),
        ];
        assert_eq!(
            rows(&items, &needs),
            vec![
                row("Hunter's Bow 4os", Spot::Page(2), Action::Sell, 1),
                row("Colossus Crossbow 4os", Spot::Page(2), Action::Keep(Side::Own), 1),
            ]
        );
        Ok(())
    }

    #[test]
    fn a_met_any_of_switch_group_wants_only_its_held_candidates() -> anyhow::Result<()> {
        let config = shipped();
        let horns = [at(named("fhl", item_data::QUALITY_SET, 16), Spot::Page(2))];
        // Nothing held: every life-leech merc helm is a candidate.
        let open = druid_needs(&config, &CHAIN, 77, &[])?;
        assert!(wants_piece(&open, "Isenhart's Horns"));
        assert_eq!(
            rows(&horns, &open),
            vec![row("Isenhart's Horns", Spot::Page(2), Action::Keep(Side::Own), 1)]
        );
        // The merc wears Bulwark: the group is met, the other candidates are no pieces.
        let met = druid_needs(&config, &CHAIN, 77, &["Bulwark"])?;
        assert!(!wants_piece(&met, "Isenhart's Horns"), "{:?}", met.pieces);
        assert!(
            rows(&horns, &met)
                .iter()
                .all(|(_, _, action, _)| *action != Action::Keep(Side::Own)),
            "{:?}",
            rows(&horns, &met)
        );
        // A held candidate stays wanted: Wormskull in the stash meets the group itself.
        let def = config
            .build_defs
            .get("summoner druid")
            .ok_or_else(|| anyhow::anyhow!("summoner"))?;
        let mut held = Needs::default();
        held.switch_gear(&config, def, 5, &[], &|n| u32::from(n == "Wormskull"));
        assert!(wants_piece(&held, "Wormskull") && !wants_piece(&held, "Isenhart's Horns"));
        // Live 2026-11-22: the merc wears Bulwark and Isenhart's Horns is on page 2. The group
        // needs one helm: the worn one; Isenhart's isn't wanted by it.
        let mut both = Needs::default();
        let worn = ["Bulwark".to_owned()];
        both.switch_gear(&config, def, 5, &worn, &|n| {
            u32::from(n == "Bulwark" || n == "Isenhart's Horns")
        });
        both.subtract_worn(&worn);
        assert!(!wants_piece(&both, "Isenhart's Horns"), "{:?}", both.pieces);
        assert!(!wants_piece(&both, "Bulwark"), "worn: no stash copy");
        let owns = |n: &str| u32::from(n == "Bulwark" || n == "Isenhart's Horns");
        let chain: Vec<String> = CHAIN.iter().map(|&c| c.to_owned()).collect();
        let live = Needs::from_config(&config, &chain, None, 5, Some(77), &worn, &owns);
        assert!(!wants_piece(&live, "Isenhart's Horns"), "{:?}", live.pieces);
        assert!(
            rows(&horns, &live)
                .iter()
                .all(|(_, _, action, _)| *action != Action::Keep(Side::Own)),
            "{:?}",
            rows(&horns, &live)
        );
        // Neither worn: the first held in the group's order (Isenhart's before Wormskull).
        let mut stash = Needs::default();
        stash.switch_gear(&config, def, 5, &[], &|n| {
            u32::from(n == "Wormskull" || n == "Isenhart's Horns")
        });
        assert!(wants_piece(&stash, "Isenhart's Horns") && !wants_piece(&stash, "Wormskull"));
        Ok(())
    }

    #[test]
    fn the_werewolf_weapon_takes_no_bow_and_no_one_hander() -> anyhow::Result<()> {
        let config = shipped();
        let needs = druid_needs(&config, &CHAIN, 77, &[])?;
        let fits_word = |word: &str, code: &str, sockets: u8| {
            needs
                .bases
                .iter()
                .any(|n| n.word.name == word && fills(n, &base(code, sockets)))
        };
        // Breath of the Dying in the Fury's weapon slot: no Gothic Bow.
        assert!(!fits_word("Breath of the Dying", "8lw", 6));
        assert!(fits_word("Breath of the Dying", "7gd", 6), "Colossus Blade");
        // Hustle: the druid holds a Legend Sword or Zweihander in both hands; a Broad Sword
        // takes one hand.
        assert!(fits_word("Hustle (weapon)", "72h", 3) && fits_word("Hustle (weapon)", "9fb", 3));
        assert!(!fits_word("Hustle (weapon)", "bsd", 3));
        // The swap keeps its bows (Brand), the Act 5 merc its one-handed Lawbringer swords.
        assert!(fits_word("Brand", "6hx", 4));
        assert!(fits_word("Lawbringer", "bsd", 3));
        assert_eq!(
            rows(&[at(base("8lw", 6), Spot::Page(1))], &needs),
            vec![row("Gothic Bow 6os", Spot::Page(1), Action::Sell, 1)]
        );
        Ok(())
    }

    #[test]
    fn an_outgrown_leveling_piece_is_wanted_only_by_another_list() -> anyhow::Result<()> {
        // Sigon's Sabot: the Summoner's lvl 40-60 boots, a Fury guide alternative.
        let config = shipped();
        let sabot = [at(named("hbt", item_data::QUALITY_SET, 38), Spot::Page(2))];
        let summoner = ["summoner druid"];
        assert!(wants_piece(
            &druid_needs(&config, &summoner, 50, &[])?,
            "Sigon's Sabot"
        ));
        let past = druid_needs(&config, &summoner, 77, &[])?;
        assert!(!wants_piece(&past, "Sigon's Sabot"), "{:?}", past.pieces);
        assert!(
            rows(&sabot, &past)
                .iter()
                .all(|(_, _, action, _)| *action != Action::Keep(Side::Own)),
            "{:?}",
            rows(&sabot, &past)
        );
        let both = druid_needs(&config, &CHAIN, 77, &[])?;
        assert_eq!(
            rows(&sabot, &both),
            vec![row("Sigon's Sabot", Spot::Page(2), Action::Keep(Side::Own), 1)]
        );
        Ok(())
    }
}

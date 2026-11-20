//! The playstyle plugin's interface: what the player's leveling and farming strategy makes of a
//! snapshot.
//!
//! Live mode and `--render` / `--show` / `--dump` use a playstyle only through [`Playstyle`].
//! The playstyles are crates of their own, picked at build time by the binary's Cargo features
//! (static dispatch, as for the reader); today `d2r-playstyle-config`, the builds, stages, farm
//! list, route and valuables tiers of a playstyle folder (`playstyles/<name>/`, shipped: the
//! Summoner → Fury Druid chain). The folder is picked at run time (`config.toml`'s
//! `playstyle`) and located by [`Files`].
//!
//! What the core gets back: the [`Guidance`] (area rating, `BiS` list, sell / move / keep
//! advice, ground-drop marks, [`crate::guidance`]), the opaque state it carries between calls
//! (the wants for the next tick's marks, the reached stage it persists in `played.json` as one
//! string per character), and the dashboard panels and `/config` JSON.

mod files;

pub use files::{DIR_VAR, Files};

use crate::game::{ItemDrop, Snapshot};
use crate::guidance::{Guidance, Marks};
use crate::played::Played;
use crate::quest_log::QuestsDone;
use crate::web::{GearInfo, LiveInfo, Panels};

/// One snapshot's evaluation ([`Playstyle::evaluate`]): `W` is the playstyle's
/// [`Playstyle::Wants`], `X` its [`Playstyle::Extra`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evaluation<'a, W, X> {
    /// What the map and the dashboard show (the marks stay empty: see
    /// [`Playstyle::mark_drops`]).
    pub guidance: Guidance,
    /// What a ground drop would still add to the playstyle's plan: the input of the next
    /// [`Playstyle::mark_drops`] calls (owned, so the caller can keep it across ticks).
    pub wants: W,
    /// The playstyle state to persist for this character (the stage it reached), when it moved
    /// on from the state passed in; the caller stores it and passes it back next time.
    pub reached: Option<&'a str>,
    /// The playstyle's own results, read only by its [`Playstyle::panels`].
    pub extra: X,
}

/// The evaluation type of playstyle `P`.
pub type EvaluationOf<'a, P> = Evaluation<'a, <P as Playstyle>::Wants, <P as Playstyle>::Extra<'a>>;

/// What the core needs from a playstyle.
pub trait Playstyle {
    /// What an evaluation leaves for the next ground-drop marks ([`Evaluation::wants`]); the
    /// default is "nothing wanted yet" (before the first evaluation, or for another player).
    type Wants: Default;

    /// The playstyle's own part of an evaluation ([`Evaluation::extra`]): the core passes it
    /// back to [`Playstyle::panels`] untouched.
    type Extra<'a>
    where
        Self: 'a;

    /// Loads the playstyle from its files.
    fn load(files: &Files) -> Self
    where
        Self: Sized;

    /// Reloads from the files after a change on disk (the core compares
    /// [`Files::fingerprint`] each tick); a broken file keeps what it decided before.
    fn reload(&mut self, files: &Files);

    /// The marks of the ground drops worth a ring, by unit id; removes the plain bases no mark
    /// wants (the map draws every drop left). Runs before the session merge (the journal
    /// records the marks with the drops), so the keep marks use the `wants` of an earlier
    /// [`Playstyle::evaluate`] (live: the previous tick's; [`Playstyle::Wants`]'s default
    /// before the first: trade marks only).
    fn mark_drops(&self, drops: &mut Vec<ItemDrop>, wants: &Self::Wants) -> Marks;

    /// The guidance for `snap` (after the session merge: cached stash pages are in).
    /// `reached` is the state stored for the character ([`Evaluation::reached`] of an earlier
    /// call), `None` when there is none; `quests` the quests it finished per difficulty
    /// (remembered across games, [`crate::quest_log::Memory`]), `None` while unknown (no
    /// record read yet): then no advice that depends on them.
    fn evaluate(
        &self,
        snap: &Snapshot,
        reached: Option<&str>,
        quests: Option<&QuestsDone>,
    ) -> EvaluationOf<'_, Self>;

    /// The playstyle's part of one tick's dashboard ([`crate::web::live_json`]) for the
    /// evaluation of the snapshot `info` was gathered from.
    fn panels<'a>(&'a self, evaluation: &'a EvaluationOf<'_, Self>, info: &LiveInfo<'a>) -> Panels<'a>;

    /// The `/config` page's JSON (`GET /api/config`): every known character with its build
    /// selection and resolved gear view, annotated from the live `gear` state; `played` gives
    /// the order and the classes seen.
    fn config_json(&self, gear: &GearInfo, played: &Played) -> serde_json::Value;

    /// The build definitions the `/config` page offers (`GET /api/builds`).
    fn builds_json(&self) -> serde_json::Value;

    /// Saves character `name`'s build selection (`None` clears it) in the playstyle's files.
    /// The core serialises the calls.
    ///
    /// # Errors
    /// The save failed (the browser gets the message).
    fn select_build(files: &Files, name: &str, build: Option<&str>) -> anyhow::Result<()>
    where
        Self: Sized;

    /// The playstyle's D2R in-game loot filter profile (`GET /api/loot-filter`, the `/config`
    /// page's copy button): the exact bytes the player imports into the game, `None` when the
    /// playstyle has none.
    fn loot_filter(files: &Files) -> Option<Vec<u8>>
    where
        Self: Sized;

    /// Forgets character `name` in the playstyle's files.
    ///
    /// # Errors
    /// As [`Playstyle::select_build`].
    fn remove_character(files: &Files, name: &str) -> anyhow::Result<()>
    where
        Self: Sized;
}

//! The playstyle plugin: what the player's leveling and farming strategy makes of a snapshot.
//!
//! Live mode and `--render` / `--show` / `--dump` use it only through [`Playstyle`]; this
//! build's playstyle is [`ConfigDriven`] (the builds, stages, farm list and valuables tiers of
//! a playstyle folder, `playstyles/<name>/`, with the user's overrides: [`files`]; shipped:
//! the Summoner → Fury Druid chain). Static dispatch, as for the reader: the playstyle is
//! picked at build time (`main.rs`), the folder at run time (`config.toml`'s `playstyle`).
//!
//! What sits behind the trait:
//! - [`valuables`]: the ground-drop marks (rules from the folder's `valuables.json`);
//! - the builds, their stage chain and its latch (`config::Settings::stage`; the reached stage
//!   is persisted by the core in `played.json` as one opaque string per character);
//! - [`stash_plan`]: sell / move / keep of the player's items, with [`gear_score`] for magic and
//!   rare items;
//! - [`build`]: the area rating of the info line from the build's damage types;
//! - [`route`]: the leveling and farming route (`route.toml`) and its current step, from the
//!   character's quests per difficulty (remembered by the core: [`crate::quest_log`]);
//! - [`web`]: the dashboard's playstyle panels (gear grid, upgrades, recipes, tips, stage, farm
//!   list, sell / keep entries, the vendor notes and the skill plan for the core panels) and the
//!   `/config` page's JSON and build selection.

pub mod build;
pub mod config;
mod config_driven;
pub mod files;
pub mod gear_score;
pub mod guidance;
pub mod route;
pub mod stash_plan;
pub mod valuables;
pub mod web;
pub mod worth;

pub use config_driven::ConfigDriven;
pub use files::Files;

use crate::config::Stage;
use crate::game::{ItemDrop, Snapshot};
use crate::played::Played;
use crate::quest_log::QuestsDone;
use crate::web::{GearInfo, LiveInfo, Panels};
use guidance::{Guidance, Marks};

/// One snapshot's evaluation ([`Playstyle::evaluate`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evaluation<'a> {
    /// What the map and the dashboard show (the marks stay empty: see
    /// [`Playstyle::mark_drops`]).
    pub guidance: Guidance,
    /// The playstyle state to persist for this character (the stage it reached), when it moved
    /// on from the state passed in; the caller stores it and passes it back next time.
    pub reached: Option<&'a str>,
    /// The character's current stage, for the dashboard's build panels.
    pub stage: Option<Stage<'a>>,
    /// The names of the current stage and its later ones, for the dashboard's farm panel.
    pub chain: Vec<String>,
    /// The route's current step, for the dashboard's "Next" panel (`None` without a route,
    /// or with the character level, difficulty or quests unknown).
    pub route: Option<route::Advice>,
}

/// What the core needs from a playstyle.
pub trait Playstyle {
    /// Loads the playstyle from its files.
    fn load(files: &Files) -> Self
    where
        Self: Sized;

    /// Reloads from the files after a change on disk (the core compares
    /// [`Files::fingerprint`] each tick); a broken file keeps what it decided before.
    fn reload(&mut self, files: &Files);

    /// The marks of the ground drops worth a ring, by unit id; removes the plain bases no mark
    /// wants (the map draws every drop left). Runs before the session merge: the journal
    /// records the marks with the drops.
    fn mark_drops(&self, drops: &mut Vec<ItemDrop>) -> Marks;

    /// The guidance for `snap` (after the session merge: cached stash pages are in).
    /// `reached` is the state stored for the character ([`Evaluation::reached`] of an earlier
    /// call), `None` when there is none; `quests` the quests it finished per difficulty
    /// (remembered across games, [`crate::quest_log::Memory`]), `None` while unknown (no
    /// record read yet): then no route advice.
    fn evaluate(&self, snap: &Snapshot, reached: Option<&str>, quests: Option<&QuestsDone>)
    -> Evaluation<'_>;

    /// The playstyle's part of one tick's dashboard ([`crate::web::live_json`]) for the
    /// evaluation of the snapshot `info` was gathered from.
    fn panels<'a>(&'a self, evaluation: &'a Evaluation<'a>, info: &LiveInfo<'a>) -> Panels<'a>;

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

    /// Forgets character `name` in the playstyle's files.
    ///
    /// # Errors
    /// As [`Playstyle::select_build`].
    fn remove_character(files: &Files, name: &str) -> anyhow::Result<()>
    where
        Self: Sized;
}

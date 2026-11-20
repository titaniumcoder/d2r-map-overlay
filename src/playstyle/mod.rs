//! The playstyle plugin: what the player's leveling and farming strategy makes of a snapshot.
//!
//! Live mode and `--render` / `--show` / `--dump` use it only through [`Playstyle`]; this
//! build's playstyle is [`ConfigDriven`] (the builds, stages, farm list and valuables tiers of
//! `config.toml`, `builds/*.toml` and `farm.toml`; today the Summoner → Fury Druid chain).
//! Static dispatch, as for the reader: the playstyle is picked at build time.
//!
//! What sits behind the trait:
//! - [`valuables`]: the ground-drop marks (tiers from `research/valuables.json`);
//! - the builds, their stage chain and its latch (`config::Config::stage`; the reached stage is
//!   persisted by the core in `played.json` as one opaque string per character);
//! - [`stash_plan`]: sell / move / keep of the player's items, with [`gear_score`] for magic and
//!   rare items;
//! - [`build`]: the area rating of the info line from the build's damage types;
//! - [`web`]: the dashboard's playstyle panels (gear grid, upgrades, recipes, tips, stage, farm
//!   list, sell / keep entries, the vendor notes and the skill plan for the core panels) and the
//!   `/config` page's JSON and build selection.

pub mod build;
pub mod config;
mod config_driven;
pub mod gear_score;
pub mod guidance;
pub mod stash_plan;
pub mod valuables;
pub mod web;

pub use config_driven::ConfigDriven;

use crate::config::{Config, Stage};
use crate::game::{ItemDrop, Snapshot};
use crate::played::Played;
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
}

/// What the core needs from a playstyle.
pub trait Playstyle {
    /// Takes the reloaded settings (`config.toml` with the `builds/` and `farm.toml` next to
    /// it, hot-reloaded by the core).
    fn reload(&mut self, config: &Config);

    /// The marks of the ground drops worth a ring, by unit id; removes the plain bases no mark
    /// wants (the map draws every drop left). Runs before the session merge: the journal
    /// records the marks with the drops.
    fn mark_drops(&self, drops: &mut Vec<ItemDrop>) -> Marks;

    /// The guidance for `snap` (after the session merge: cached stash pages are in).
    /// `reached` is the state stored for the character ([`Evaluation::reached`] of an earlier
    /// call), `None` when there is none.
    fn evaluate(&self, snap: &Snapshot, reached: Option<&str>) -> Evaluation<'_>;

    /// The playstyle's part of one tick's dashboard ([`crate::web::live_json`]) for the
    /// evaluation of the snapshot `info` was gathered from.
    fn panels<'a>(&'a self, evaluation: &'a Evaluation<'a>, info: &LiveInfo<'a>) -> Panels<'a>;

    /// The `/config` page's JSON (`GET /api/config`): every known character with its build
    /// selection and resolved gear view, annotated from the live `gear` state; `played` gives
    /// the order and the classes seen.
    fn config_json(&self, gear: &GearInfo, played: &Played) -> serde_json::Value;

    /// The build definitions the `/config` page offers (`GET /api/builds`).
    fn builds_json(&self) -> serde_json::Value;

    /// `config.toml`'s `text` with character `name`'s build selection set to `build` (`None`
    /// clears it), comments preserved; `None` when the text can't be edited (the browser gets
    /// an error).
    fn select_build(text: &str, name: &str, build: Option<&str>) -> Option<String>
    where
        Self: Sized;

    /// `config.toml`'s `text` without character `name`'s entries; `None` when the text can't
    /// be edited.
    fn remove_character(text: &str, name: &str) -> Option<String>
    where
        Self: Sized;
}

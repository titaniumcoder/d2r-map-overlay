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
//! - recipes, tips, skill plans and the farm list (read by the dashboard's playstyle panels;
//!   they move behind the trait with the dashboard split, step 6).

pub mod build;
pub mod config;
mod config_driven;
pub mod gear_score;
pub mod guidance;
pub mod stash_plan;
pub mod valuables;

pub use config_driven::ConfigDriven;

use crate::config::{Config, FarmGroup, Stage};
use crate::game::{ItemDrop, Snapshot};
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

    /// The cross-character farm list, for the dashboard.
    fn farm(&self) -> &[FarmGroup];
}

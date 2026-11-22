//! The playstyle plugin's interface: what the player's leveling and farming strategy makes of a
//! snapshot, plus its dashboard panels and `/config` JSON.

mod files;

pub use files::{DIR_VAR, Files};

use crate::game::{ItemDrop, Snapshot};
use crate::guidance::{Guidance, Marks};
use crate::played::Played;
use crate::web::{GearInfo, LiveInfo, Panels};

/// One snapshot's evaluation ([`Playstyle::evaluate`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Evaluation<'a, W, X> {
    /// The marks stay empty: see [`Playstyle::mark_drops`].
    pub guidance: Guidance,
    /// The input of the next [`Playstyle::mark_drops`] calls.
    pub wants: W,
    /// The stage reached, when it moved on; the caller persists it and passes it back.
    pub reached: Option<&'a str>,
    /// Read only by [`Playstyle::panels`].
    pub extra: X,
}

pub type EvaluationOf<'a, P> = Evaluation<'a, <P as Playstyle>::Wants, <P as Playstyle>::Extra<'a>>;

pub trait Playstyle {
    /// The default is "nothing wanted yet".
    type Wants: Default;

    type Extra<'a>
    where
        Self: 'a;

    fn load(files: &Files) -> Self
    where
        Self: Sized;

    /// After a change on disk; a broken file keeps what it decided before.
    fn reload(&mut self, files: &Files);

    /// The marks of the ground drops worth a ring; removes the plain bases no mark wants.
    /// Runs before [`Playstyle::evaluate`], so `wants` are an earlier evaluation's.
    fn mark_drops(&self, drops: &mut Vec<ItemDrop>, wants: &Self::Wants) -> Marks;

    /// `reached` is an earlier [`Evaluation::reached`].
    fn evaluate(&self, snap: &Snapshot, reached: Option<&str>) -> EvaluationOf<'_, Self>;

    /// The playstyle's part of one tick's dashboard.
    fn panels<'a>(&'a self, evaluation: &'a EvaluationOf<'_, Self>, info: &LiveInfo<'a>) -> Panels<'a>;

    /// `GET /api/config`: every known character with its build selection and gear view.
    fn config_json(&self, gear: &GearInfo, played: &Played) -> serde_json::Value;

    /// `GET /api/builds`.
    fn builds_json(&self) -> serde_json::Value;

    /// `None` clears the selection. The core serialises the calls.
    ///
    /// # Errors
    /// The save failed (the browser gets the message).
    fn select_build(files: &Files, name: &str, build: Option<&str>) -> anyhow::Result<()>
    where
        Self: Sized;

    /// Marks `piece` of build `build` (its name) not needed (`needed = false`) or needed again.
    /// The core serialises the calls.
    ///
    /// # Errors
    /// As [`Playstyle::select_build`].
    fn set_not_needed(files: &Files, build: &str, piece: &str, needed: bool) -> anyhow::Result<()>
    where
        Self: Sized;

    /// The exact bytes of the in-game loot filter profile the player imports.
    fn loot_filter(files: &Files) -> Option<Vec<u8>>
    where
        Self: Sized;

    ///
    /// # Errors
    /// As [`Playstyle::select_build`].
    fn remove_character(files: &Files, name: &str) -> anyhow::Result<()>
    where
        Self: Sized;
}

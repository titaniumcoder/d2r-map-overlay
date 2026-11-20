//! This build's playstyle: everything read from the playstyle folder's files (see
//! [`super::files`]).

use crate::game::{ItemDrop, Snapshot};
use crate::played::Played;
use crate::web::{GearInfo, LiveInfo, Panels};

use super::config::Settings;
use super::files::{self, Files};
use super::guidance::{Guidance, Marks};
use super::{Evaluation, Playstyle, build, stash_plan, valuables, web};

/// The playstyle of the folder's files: the character's selected build (else its class's
/// default) followed along its stage chain, the farm list and the valuables tiers.
#[derive(Debug, Clone, Default)]
pub struct ConfigDriven {
    config: Settings,
}

impl Playstyle for ConfigDriven {
    fn load(files: &Files) -> Self {
        Self {
            config: files::load(files),
        }
    }

    fn reload(&mut self, files: &Files) {
        files::reload(&mut self.config, files);
    }

    fn mark_drops(&self, drops: &mut Vec<ItemDrop>) -> Marks {
        valuables::apply(drops, &self.config.valuables)
    }

    fn evaluate(&self, snap: &Snapshot, reached: Option<&str>) -> Evaluation<'_> {
        let config = &self.config;
        // Damages: the character's own `[builds]` entry, else the selected build's, else its
        // class's. `BiS`: inline slots when present, else the selected build's (user decision,
        // 2026-11-17). A `switch_requires` group is met by the copies held in the shared stash
        // (session-cached), by this character or worn by its mercenary, or when the character
        // reached the stage before (played.json): the shared stash is unread each new game
        // until opened, and a switch is one-way.
        let (class, level) = (Some(snap.player.class), snap.player.char_level);
        let holdings = snap.holdings();
        let owns = |item: &str| holdings.count(item);
        let name = snap.player.name.as_str();
        let damages = config.damages_for_player(name, class, level, &owns, reached);
        let stage = config.stage(name, class, level, &owns, reached);
        let bis = config.resolved_bis(name, class, level, &owns, reached);
        // Remember a stage further along (or off the remembered one's chain, e.g. a new
        // selection); never one behind it (a level not yet read, an earlier stage).
        let moved_on = stage
            .map(|s| s.slug)
            .filter(|&slug| !reached.is_some_and(|reached| config.leads_to(slug, reached)));
        // The current stage and its later ones: the farm list drops what this character wears
        // of them; the item plan keeps them.
        let chain = stage.map_or_else(Vec::new, |s| config.stage_chain(s.slug));
        let items = stash_plan::for_snapshot(snap, config, &chain, bis.as_ref());
        Evaluation {
            guidance: Guidance {
                rating: build::rate(snap.level.id, snap.difficulty, damages),
                bis,
                items,
                marks: Marks::new(),
            },
            reached: moved_on,
            stage,
            chain,
        }
    }

    fn panels<'a>(&'a self, evaluation: &'a Evaluation<'a>, info: &LiveInfo<'a>) -> Panels<'a> {
        web::panels(&web::PanelInfo {
            live: *info,
            bis: evaluation.guidance.bis.as_ref(),
            build: evaluation.stage.map(|s| s.build),
            stage: evaluation.stage,
            farm: &self.config.farm,
            stage_chain: &evaluation.chain,
        })
    }

    fn config_json(&self, gear: &GearInfo, played: &Played) -> serde_json::Value {
        web::config_page::config_json(&self.config, gear, played)
    }

    fn builds_json(&self) -> serde_json::Value {
        web::config_page::builds_json(&self.config)
    }

    fn select_build(files: &Files, name: &str, build: Option<&str>) -> anyhow::Result<()> {
        files::select_build(files, name, build)
    }

    fn remove_character(files: &Files, name: &str) -> anyhow::Result<()> {
        files::remove_character(files, name)
    }
}

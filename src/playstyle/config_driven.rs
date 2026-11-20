//! This build's playstyle: everything read from the playstyle folder's files (see
//! [`super::files`]).

use crate::game::{ItemDrop, Snapshot};
use crate::played::Played;
use crate::quest_log::QuestsDone;
use crate::stash::Holdings;
use crate::web::{GearInfo, LiveInfo, Panels};

use super::build::Damage;
use super::config::Settings;
use super::files::{self, Files};
use super::guidance::{Guidance, Marks};
use super::stash_plan::Wants;
use super::{Evaluation, Playstyle, build, marks, route, stash_plan, web};

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

    fn mark_drops(&self, drops: &mut Vec<ItemDrop>, wants: &Wants) -> Marks {
        marks::apply(drops, &self.config.valuables, wants)
    }

    fn evaluate(
        &self,
        snap: &Snapshot,
        reached: Option<&str>,
        quests: Option<&QuestsDone>,
    ) -> Evaluation<'_> {
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
        let (items, wants) = stash_plan::for_snapshot(snap, config, &chain, bis.as_ref());
        let route = advise(config, snap, &holdings, damages, quests);
        Evaluation {
            guidance: Guidance {
                rating: build::rate(snap.level.id, snap.difficulty, damages),
                bis,
                items,
                marks: Marks::new(),
            },
            wants,
            reached: moved_on,
            stage,
            chain,
            route,
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
            next: evaluation.route.as_ref(),
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

/// The route's current step for `snap`'s character ([`route::current`]); `None` without
/// steps or with the level, difficulty or quests unknown (no false advice before the quest
/// record is read).
fn advise(
    config: &Settings,
    snap: &Snapshot,
    holdings: &Holdings,
    damages: &[Damage],
    quests: Option<&QuestsDone>,
) -> Option<route::Advice> {
    let (Some(level), Some(difficulty), Some(quests)) = (snap.player.char_level, snap.difficulty, quests)
    else {
        return None;
    };
    if config.route.step.is_empty() {
        return None;
    }
    let materials = web::recipes::materials_of(&snap.held_materials, &snap.stash);
    let worth = route::worth_for(&config.route, difficulty, &config.farm, holdings, &materials);
    let max_areas = route::max_areas(damages);
    route::current(
        &config.route,
        &route::Facts {
            level,
            difficulty,
            quests,
            worth: &worth,
            max_areas: &max_areas,
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::{Context, Result};

    #[test]
    fn no_route_advice_while_the_quests_are_unknown() -> Result<()> {
        let mut snap: Snapshot =
            serde_json::from_str(include_str!("../../tests/fixtures/jail1-snapshot.json"))
                .context("parsing fixture")?;
        snap.player.char_level = Some(10);
        snap.difficulty = Some(0);
        let playstyle = ConfigDriven::load(&Files {
            shipped: Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("playstyles/summoner-fury")),
            quiet: true,
            ..Files::default()
        });
        assert_eq!(playstyle.evaluate(&snap, None, None).route, None, "unread record");
        let advice = playstyle
            .evaluate(&snap, None, Some(&QuestsDone::default()))
            .route;
        assert_eq!(advice.map(|a| a.id).as_deref(), Some("normal-tristram"));
        Ok(())
    }

    #[test]
    fn ground_drops_get_keep_farm_and_trade_marks() -> Result<()> {
        use crate::guidance::MarkKind;
        // The Flayer Jungle dump (a level-50 Summoner holding no Harmony) with five drops:
        // a white 4os Hunter's Bow, a Monarch 4os, a Long Sword 0os, an unidentified unique ring
        // and unidentified unique Heavy Gloves.
        let snap: Snapshot =
            serde_json::from_str(include_str!("../../tests/fixtures/flayer-marks-snapshot.json"))
                .context("parsing fixture")?;
        let playstyle = ConfigDriven::load(&Files {
            shipped: Some(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("playstyles/summoner-fury")),
            quiet: true,
            ..Files::default()
        });
        let wants = playstyle.evaluate(&snap, None, None).wants;
        let mut drops = snap.drops.clone();
        let marks = playstyle.mark_drops(&mut drops, &wants);
        let got: Vec<_> = drops
            .iter()
            .map(|d| {
                let m = marks.get(&d.id);
                (
                    d.code.as_deref(),
                    m.map(|m| m.kind),
                    m.map(|m| m.rule.as_str()),
                    m.is_some_and(|m| m.uncertain),
                )
            })
            .collect();
        assert_eq!(
            got,
            vec![
                (Some("hbw"), Some(MarkKind::Keep), Some("Harmony base"), false),
                (Some("uit"), Some(MarkKind::Trade), Some("Monarch 4os"), false),
                // Raven Frost or Manald Heal by its base, but the S unique-ring trade rule wins
                // over a guess.
                (Some("rin"), Some(MarkKind::Trade), Some("Stone of Jordan"), true),
                (Some("vgl"), Some(MarkKind::Farm), Some("Bloodfist?"), true),
            ],
            "the Long Sword 0os is gone"
        );
        // Without wants (before the first evaluation, another player unit): trade marks only.
        let mut drops = snap.drops;
        let marks = playstyle.mark_drops(&mut drops, &Wants::default());
        let kinds: Vec<_> = drops
            .iter()
            .map(|d| (d.code.as_deref(), marks.get(&d.id).map(|m| m.kind)))
            .collect();
        assert_eq!(
            kinds,
            vec![
                (Some("uit"), Some(MarkKind::Trade)),
                (Some("rin"), Some(MarkKind::Trade)),
                (Some("vgl"), None),
            ],
            "the gloves stay shown (a unique), without a ring"
        );
        Ok(())
    }
}

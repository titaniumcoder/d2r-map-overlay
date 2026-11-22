//! The playstyle read from the playstyle folder's files ([`crate::files`]).

use d2r_core::build::{self, Damage};
use d2r_core::game::{ItemDrop, Snapshot};
use d2r_core::guidance::{Guidance, Marks};
use d2r_core::played::Played;
use d2r_core::playstyle::{Evaluation, EvaluationOf, Files, Playstyle};
use d2r_core::quest_log::QuestsDone;
use d2r_core::stash::Holdings;
use d2r_core::web::{GearInfo, LiveInfo, Panels};

use crate::config::{Settings, Stage};
use crate::stash_plan::Wants;
use crate::{files, marks, route, stash_plan, web};

/// The playstyle of the folder's files: the character's selected build (else its class's
/// default) followed along its stage chain, the farm list and the valuables tiers.
#[derive(Debug, Clone, Default)]
pub struct ConfigDriven {
    config: Settings,
}

/// The evaluation's part only the panels read ([`Playstyle::Extra`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extra<'a> {
    /// The character's current stage, for the build panels.
    pub stage: Option<Stage<'a>>,
    /// The names of the current stage and its later ones, for the farm panel.
    pub chain: Vec<String>,
    /// The route's current step, for the "Next" panel ([`advise`]).
    pub route: Option<route::Advice>,
}

impl Playstyle for ConfigDriven {
    type Wants = Wants;
    type Extra<'a> = Extra<'a>;

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
    ) -> EvaluationOf<'_, Self> {
        let config = &self.config;
        let (class, level) = (Some(snap.player.class), snap.player.char_level);
        let holdings = snap.holdings();
        let owns = |item: &str| holdings.count(item);
        let name = snap.player.name.as_str();
        let damages = config.damages_for_player(name, class, level, &owns, reached);
        let stage = config.stage(name, class, level, &owns, reached);
        let bis = config.resolved_bis(name, class, level, &owns, reached);
        let moved_on = stage_to_remember(config, stage, reached);
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
            extra: Extra { stage, chain, route },
        }
    }

    fn panels<'a>(&'a self, evaluation: &'a EvaluationOf<'_, Self>, info: &LiveInfo<'a>) -> Panels<'a> {
        let extra = &evaluation.extra;
        web::panels(&web::PanelInfo {
            live: *info,
            bis: evaluation.guidance.bis.as_ref(),
            build: extra.stage.map(|s| s.build),
            stage: extra.stage,
            farm: &self.config.farm,
            route: &self.config.route,
            stage_chain: &extra.chain,
            next: extra.route.as_ref(),
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

    fn loot_filter(files: &Files) -> Option<Vec<u8>> {
        files::loot_filter(files)
    }

    fn remove_character(files: &Files, name: &str) -> anyhow::Result<()> {
        files::remove_character(files, name)
    }
}

/// `stage` when it is further along than `reached` or off its chain (a new selection); never
/// one behind it (a level not yet read must not undo the latch).
fn stage_to_remember<'a>(
    config: &Settings,
    stage: Option<Stage<'a>>,
    reached: Option<&str>,
) -> Option<&'a str> {
    stage
        .map(|s| s.slug)
        .filter(|&slug| !reached.is_some_and(|reached| config.leads_to(slug, reached)))
}

/// The route's current step for `snap`'s character ([`route::current`]); `None` without
/// steps, with the level or difficulty unknown, or with the quests unknown (record not read
/// this game) and the current step depending on them (no false advice).
fn advise(
    config: &Settings,
    snap: &Snapshot,
    holdings: &Holdings,
    damages: &[Damage],
    quests: Option<&QuestsDone>,
) -> Option<route::Advice> {
    let (Some(level), Some(difficulty)) = (snap.player.char_level, snap.difficulty) else {
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
    fn quest_steps_hide_while_the_quests_are_unknown() -> Result<()> {
        let mut snap: Snapshot =
            serde_json::from_str(include_str!("../../../tests/fixtures/jail1-snapshot.json"))
                .context("parsing fixture")?;
        snap.player.char_level = Some(10);
        snap.difficulty = Some(0);
        let playstyle = ConfigDriven::load(&Files {
            shipped: Some(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../playstyles/summoner-fury"),
            ),
            quiet: true,
            ..Files::default()
        });
        for quests in [None, Some(&QuestsDone::default())] {
            let advice = playstyle.evaluate(&snap, None, quests).extra.route;
            assert_eq!(advice.map(|a| a.id).as_deref(), Some("normal-tristram"));
        }
        snap.player.char_level = Some(16);
        assert_eq!(
            playstyle.evaluate(&snap, None, None).extra.route,
            None,
            "unread record"
        );
        let advice = playstyle
            .evaluate(&snap, None, Some(&QuestsDone::default()))
            .extra
            .route;
        assert_eq!(advice.map(|a| a.id).as_deref(), Some("normal-canyon"));
        Ok(())
    }

    #[test]
    fn ground_drops_get_keep_farm_and_trade_marks() -> Result<()> {
        use d2r_core::guidance::MarkKind;
        // Level-50 Summoner without Harmony; drops: Hunter's Bow 4os, Monarch 4os, Long Sword
        // 0os, unidentified unique ring and Heavy Gloves.
        let snap: Snapshot =
            serde_json::from_str(include_str!("../../../tests/fixtures/flayer-marks-snapshot.json"))
                .context("parsing fixture")?;
        let playstyle = ConfigDriven::load(&Files {
            shipped: Some(
                std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../playstyles/summoner-fury"),
            ),
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
                (Some("rin"), Some(MarkKind::Trade), Some("Stone of Jordan"), true),
                (Some("vgl"), Some(MarkKind::Farm), Some("Bloodfist?"), true),
            ],
            "the Long Sword 0os is gone"
        );
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

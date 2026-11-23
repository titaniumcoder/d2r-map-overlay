//! The playstyles read from the playstyle folders' files ([`crate::files`]): the selected one
//! and every other folder beside it, each character following one of them.

use d2r_core::build;
use d2r_core::game::{ItemDrop, Snapshot};
use d2r_core::guidance::{Guidance, Marks};
use d2r_core::played::Played;
use d2r_core::playstyle::{Evaluation, EvaluationOf, Files, Playstyle};
use d2r_core::web::{LiveInfo, Panels};

use crate::config::{FarmGroup, Settings, Stage, owns_nothing, playstyle_of, split_selection};
use crate::stash_plan::{Other, Others, Wants};
use crate::web::farm::OtherFarm;
use crate::{files, marks, stash_plan, web};

/// The playstyles of the folders' files: each character's playstyle (its selection, else the
/// one with a default build for its class, else the selected one), its selected build (else
/// its class's default) followed along its stage chain, the farm list and the valuables tiers.
/// The shared stash keeps for the other characters and playstyles too.
#[derive(Debug, Clone, Default)]
pub struct ConfigDriven {
    /// The playstyle `config.toml` selects, with the user's and old-place overrides.
    selected: Folder,
    /// The other playstyle folders beside it ([`Files::others`]), by name.
    others: Vec<Folder>,
}

/// One playstyle folder's settings.
#[derive(Debug, Clone, Default)]
struct Folder {
    /// The folder's name: how a selection (`"<key>/<slug>"`) and the `/config` page name it.
    key: String,
    config: Settings,
}

/// The wants of one evaluation and the playstyle (folder name) whose mark tiers apply to them
/// ([`Playstyle::Wants`]); the default is the selected playstyle's.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PlaystyleWants {
    playstyle: String,
    wants: Wants,
}

/// The evaluation's part only the panels read ([`Playstyle::Extra`]).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extra<'a> {
    /// The character's playstyle.
    pub config: &'a Settings,
    /// The character's current stage, for the build panels.
    pub stage: Option<Stage<'a>>,
    /// The names of the current stage and its later ones, for the farm panel.
    pub chain: Vec<String>,
    /// The farm groups some own character following the playstyle still needs
    /// ([`stash_plan::farm_needed`]), for the farm panel.
    pub farm: Vec<FarmGroup>,
    /// The other own characters following another playstyle, by name, each with the farm
    /// groups it still needs, for the farm panel after the character's own groups.
    pub other_farm: Vec<OtherFarm<'a>>,
}

/// Each folder's files: the selected one's as given, every other one's own files only.
fn folder_files(files: &Files) -> Vec<(String, Files)> {
    let key = files
        .shipped
        .as_deref()
        .and_then(|dir| dir.file_name()?.to_str())
        .unwrap_or("default")
        .to_owned();
    let selected = Files {
        others: Vec::new(),
        ..files.clone()
    };
    std::iter::once((key, selected))
        .chain(
            files
                .others
                .iter()
                .map(|(key, dir)| (key.clone(), files.only(dir))),
        )
        .collect()
}

impl ConfigDriven {
    /// The selected playstyle first, then the others.
    fn folders(&self) -> Vec<(&str, &Settings)> {
        std::iter::once(&self.selected)
            .chain(&self.others)
            .map(|f| (f.key.as_str(), &f.config))
            .collect()
    }

    /// The playstyle (folder name and settings) character `name` of class `class` follows.
    fn playstyle_of(&self, name: &str, class: Option<u32>) -> (&str, &Settings) {
        playstyle_of(&self.folders(), name, class).unwrap_or((&self.selected.key, &self.selected.config))
    }

    /// Says each selection that names no build of any playstyle; its character follows its
    /// class default.
    fn say_stale_selections(&self, files: &Files) {
        for (name, build) in stale_selections(&self.folders()) {
            files.say(format_args!(
                "playstyle: [bis.{name:?}] build = {build:?} names no build any more; the class default is followed"
            ));
        }
    }

    /// The selections live in the selected playstyle's files (an old `config.toml` may hold
    /// some): every playstyle reads the same ones.
    fn share_selections(&mut self) {
        for other in &mut self.others {
            other.config.bis.clone_from(&self.selected.config.bis);
        }
    }

    /// The other own characters in `played` (not `me`; class known), by the playstyle they
    /// follow, for the playing character's playstyle `mine`; one of the playstyle's class
    /// without a stage has an empty chain (no farm group is passed while it is there). Every
    /// other playstyle is listed, with or without characters (its farm list counts).
    fn others(&self, me: &str, mine: &str, played: &Played) -> Others<'_> {
        let folders = self.folders();
        let mut others = Others {
            alongside: Vec::new(),
            playstyles: folders
                .iter()
                .filter(|(key, _)| *key != mine)
                .map(|(_, config)| (*config, Vec::new()))
                .collect(),
        };
        for (key, entry) in played {
            let Some(class) = entry.class.filter(|_| *key != me.to_lowercase()) else {
                continue;
            };
            let (folder, config) = self.playstyle_of(key, Some(class));
            let reached = entry.stage.as_deref();
            let stage = config.stage(key, Some(class), entry.level, &owns_nothing, reached);
            let of_class = config.build_defs.values().any(|b| b.class == Some(class));
            let chain = match stage {
                Some(stage) => config.stage_chain(stage.slug),
                None if of_class => Vec::new(),
                None => continue,
            };
            let other = Other {
                name: entry.name.clone(),
                chain,
                class,
                level: entry.level,
                worn: entry.worn.clone(),
            };
            if folder == mine {
                others.alongside.push(other);
            } else if let Some((_, members)) = others
                .playstyles
                .iter_mut()
                .find(|(c, _)| std::ptr::eq(*c, config))
            {
                members.push(other);
            }
        }
        others
    }
}

impl Playstyle for ConfigDriven {
    type Wants = PlaystyleWants;
    type Extra<'a> = Extra<'a>;

    fn load(files: &Files) -> Self {
        let mut folders = folder_files(files).into_iter().map(|(key, files)| Folder {
            config: files::load(&files),
            key,
        });
        let mut playstyle = Self {
            selected: folders.next().unwrap_or_default(),
            others: folders.collect(),
        };
        playstyle.share_selections();
        playstyle.say_stale_selections(files);
        playstyle
    }

    fn reload(&mut self, files: &Files) {
        let folders = folder_files(files);
        let mine = std::iter::once(&mut self.selected).chain(&mut self.others);
        for (folder, (_, files)) in mine.zip(&folders) {
            files::reload(&mut folder.config, files);
        }
        self.share_selections();
        self.say_stale_selections(files);
    }

    fn mark_drops(&self, drops: &mut Vec<ItemDrop>, wants: &PlaystyleWants) -> Marks {
        let config = self
            .others
            .iter()
            .find(|f| f.key == wants.playstyle)
            .map_or(&self.selected.config, |f| &f.config);
        marks::apply(drops, &config.valuables, &wants.wants)
    }

    fn evaluate(&self, snap: &Snapshot, reached: Option<&str>, played: &Played) -> EvaluationOf<'_, Self> {
        let (class, level) = (Some(snap.player.class), snap.player.char_level);
        let name = snap.player.name.as_str();
        let (folder, config) = self.playstyle_of(name, class);
        let holdings = snap.holdings();
        let owns = |item: &str| holdings.count(item);
        let damages = config.damages_for_player(name, class, level, &owns, reached);
        let stage = config.stage(name, class, level, &owns, reached);
        let bis = config.resolved_bis(name, class, level, &owns, reached);
        let moved_on = stage_to_remember(config, stage, reached);
        let chain = stage.map_or_else(Vec::new, |s| config.stage_chain(s.slug));
        let others = self.others(name, folder, played);
        let (items, wants) = stash_plan::for_snapshot(snap, config, &chain, bis.as_ref(), &others);
        let farm = stash_plan::farm_needed(config, &others.own_stages(&chain))
            .into_iter()
            .cloned()
            .collect();
        let other_farm = other_farm(&others);
        Evaluation {
            guidance: Guidance {
                rating: build::rate(snap.level.id, snap.difficulty, damages),
                bis,
                items,
                marks: Marks::new(),
            },
            wants: PlaystyleWants {
                playstyle: folder.to_owned(),
                wants,
            },
            reached: moved_on,
            extra: Extra {
                config,
                stage,
                chain,
                farm,
                other_farm,
            },
        }
    }

    fn panels<'a>(&'a self, evaluation: &'a EvaluationOf<'_, Self>, info: &LiveInfo<'a>) -> Panels<'a> {
        let extra = &evaluation.extra;
        let config = extra.config;
        let mut panels = web::panels(&web::PanelInfo {
            live: *info,
            bis: evaluation.guidance.bis.as_ref(),
            build: extra.stage.map(|s| s.build),
            stage: extra.stage,
            farm: &extra.farm,
            other_farm: &extra.other_farm,
            route: &config.route,
            stage_chain: &extra.chain,
            not_needed: &config.not_needed,
        });
        if let Some(shop) = info.shop {
            let marks = marks::shop_marks(shop, &evaluation.wants.wants, config.valuables.money);
            for (note, mark) in panels.shop.iter_mut().zip(marks) {
                note.mark = mark;
            }
        }
        panels
    }

    fn config_json(&self, played: &Played) -> serde_json::Value {
        web::config_page::config_json(&self.folders(), played)
    }

    fn builds_json(&self) -> serde_json::Value {
        web::config_page::builds_json(&self.folders())
    }

    fn select_build(files: &Files, name: &str, build: Option<&str>) -> anyhow::Result<()> {
        files::select_build(files, name, build)
    }

    fn set_not_needed(files: &Files, build: &str, piece: &str, needed: bool) -> anyhow::Result<()> {
        files::set_not_needed(files, build, piece, needed)
    }

    fn set_money_farming(files: &Files, on: bool) -> anyhow::Result<()> {
        files::set_money_farming(files, on)
    }

    fn loot_filter(files: &Files, playstyle: Option<&str>) -> Option<Vec<u8>> {
        let folders = folder_files(files);
        let (_, files) = match playstyle {
            Some(key) => folders.iter().find(|(k, _)| k == key)?,
            None => folders.first()?,
        };
        files::loot_filter(files)
    }

    fn remove_character(files: &Files, name: &str) -> anyhow::Result<()> {
        files::remove_character(files, name)
    }
}

/// The farm panel's part for each own character following another playstyle than the played
/// one, by name: the groups of its playstyle's farm list it hasn't passed (its current stage,
/// [`Settings::farm_passed`]; all of them without a stage). A character following the played
/// one's playstyle shares its groups instead ([`Others::own_stages`]).
fn other_farm<'a>(others: &Others<'a>) -> Vec<OtherFarm<'a>> {
    let mut farms: Vec<OtherFarm> = others
        .playstyles
        .iter()
        .flat_map(|(config, members)| members.iter().map(move |m| (*config, m)))
        .map(|(config, member)| {
            let stage: Vec<&str> = member.chain.first().map(String::as_str).into_iter().collect();
            OtherFarm {
                character: member.name.clone(),
                config,
                class: member.class,
                chain: member.chain.clone(),
                worn: member.worn.clone(),
                groups: stash_plan::farm_needed(config, &stage)
                    .into_iter()
                    .cloned()
                    .collect(),
            }
        })
        .collect();
    farms.sort_by_key(|f| f.character.to_lowercase());
    farms
}

/// The `(character, selection)` of each build selection (shared by every folder, read from the
/// first) whose build slug no folder holds; one without a build (inline slots only) is none.
fn stale_selections(folders: &[(&str, &Settings)]) -> Vec<(String, String)> {
    let Some((_, first)) = folders.first() else {
        return Vec::new();
    };
    first
        .bis
        .iter()
        .filter(|(_, bis)| !bis.build.trim().is_empty())
        .filter(|(_, bis)| {
            let (_, slug) = split_selection(&bis.build);
            !folders.iter().any(|(_, c)| c.build_defs.contains_key(&slug))
        })
        .map(|(name, bis)| (name.clone(), bis.build.clone()))
        .collect()
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

#[cfg(test)]
mod tests {
    use super::*;
    use anyhow::{Context, Result};

    fn shipped(name: &str) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../playstyles")
            .join(name)
    }

    #[test]
    fn the_assassin_follows_the_trapsin_and_the_druid_keeps_for_her() -> Result<()> {
        let files = Files {
            shipped: Some(shipped("fury-druid")),
            others: vec![("trapsin".to_owned(), shipped("trapsin"))],
            quiet: true,
            ..Files::default()
        };
        let playstyle = ConfigDriven::load(&files);
        let fixture = include_str!("../../../tests/fixtures/flayer-marks-snapshot.json");
        let druid: Snapshot = serde_json::from_str(fixture).context("parsing fixture")?;
        let none = Played::new();
        let at = |level: u8| -> Result<_> {
            let mut snap: Snapshot = serde_json::from_str(fixture)?;
            snap.player.class = 6;
            "Trapper".clone_into(&mut snap.player.name);
            snap.player.char_level = Some(level);
            let evaluation = playstyle.evaluate(&snap, None, &none);
            let stage = evaluation.extra.stage.map(|s| s.slug.to_owned());
            let name = evaluation.extra.config.name.clone();
            Ok((stage, evaluation.wants.playstyle, name))
        };
        let trapsin = "Fire Wake Assassin → Trapsin".to_owned();
        assert_eq!(
            at(20)?,
            (
                Some("fire wake assassin".to_owned()),
                "trapsin".to_owned(),
                trapsin.clone()
            )
        );
        assert_eq!(
            at(38)?,
            (Some("trapsin".to_owned()), "trapsin".to_owned(), trapsin)
        );
        // The farm panel hides the Fire Wake group once she is past that stage.
        let farm_at = |level: u8, played: &Played| -> Result<Vec<String>> {
            let mut snap: Snapshot = serde_json::from_str(fixture)?;
            snap.player.class = 6;
            "Trapper".clone_into(&mut snap.player.name);
            snap.player.char_level = Some(level);
            let evaluation = playstyle.evaluate(&snap, None, played);
            Ok(evaluation.extra.farm.iter().map(|g| g.target.clone()).collect())
        };
        assert_eq!(farm_at(20, &none)?, ["Fire Wake Assassin", "Trapsin"]);
        assert_eq!(farm_at(55, &none)?, ["Trapsin"]);
        let leveling_alt = Played::from([(
            "starter".to_owned(),
            d2r_core::played::Entry {
                name: "Starter".to_owned(),
                t: 1,
                class: Some(6),
                stage: None,
                level: Some(12),
                worn: Vec::new(),
            },
        )]);
        assert_eq!(
            farm_at(55, &leveling_alt)?,
            ["Fire Wake Assassin", "Trapsin"],
            "another own assassin still needs it"
        );
        let fury = playstyle.evaluate(&druid, None, &none);
        assert_eq!(
            (fury.extra.stage.map(|s| s.slug), fury.wants.playstyle.as_str()),
            (Some("fury druid"), "fury-druid")
        );
        // The druid's stash plan wants the trapsin's farm list, and more once she was played.
        let alone = ConfigDriven::load(&Files {
            others: Vec::new(),
            ..files
        });
        let without = alone.evaluate(&druid, None, &none).wants.wants;
        assert_ne!(fury.wants.wants, without, "the trapsin farm list");
        let played = Played::from([(
            "trapper".to_owned(),
            d2r_core::played::Entry {
                name: "Trapper".to_owned(),
                t: 1,
                class: Some(6),
                stage: Some("trapsin".to_owned()),
                level: Some(50),
                worn: Vec::new(),
            },
        )]);
        let with_her = playstyle.evaluate(&druid, None, &played).wants.wants;
        assert_ne!(with_her, fury.wants.wants, "her build's own pieces");
        Ok(())
    }

    fn entry(
        name: &str,
        class: u32,
        stage: &str,
        level: u8,
        worn: &[&str],
    ) -> (String, d2r_core::played::Entry) {
        let entry = d2r_core::played::Entry {
            name: name.to_owned(),
            t: 1,
            class: Some(class),
            stage: Some(stage.to_owned()),
            level: Some(level),
            worn: worn.iter().map(|&w| w.to_owned()).collect(),
        };
        (name.to_lowercase(), entry)
    }

    /// The fixture's snapshot played by `name` of class `class` at `level`.
    fn played_by(name: &str, class: u32, level: u8) -> Result<Snapshot> {
        let fixture = include_str!("../../../tests/fixtures/flayer-marks-snapshot.json");
        let mut snap: Snapshot = serde_json::from_str(fixture).context("parsing fixture")?;
        snap.player.class = class;
        name.clone_into(&mut snap.player.name);
        snap.player.char_level = Some(level);
        Ok(snap)
    }

    /// The farm panel's groups as `(character, build)`, and the entry names of each.
    fn farm_groups(
        playstyle: &ConfigDriven,
        snap: &Snapshot,
        played: &Played,
    ) -> Vec<(Option<String>, String, Vec<String>)> {
        let evaluation = playstyle.evaluate(snap, None, played);
        let blocks = d2r_core::render::TextBlocks::default();
        let info = d2r_core::web::fixtures::live_info(snap.player.class, &blocks, &[], &[], &[]);
        let panels = playstyle.panels(&evaluation, &info);
        let groups = panels
            .sections
            .get("farm")
            .and_then(|f| f.get("groups"))
            .and_then(serde_json::Value::as_array)
            .cloned()
            .unwrap_or_default();
        groups
            .iter()
            .map(|g| {
                let text =
                    |v: Option<&serde_json::Value>| v.and_then(serde_json::Value::as_str).map(str::to_owned);
                let items = g
                    .get("items")
                    .and_then(serde_json::Value::as_array)
                    .map_or_else(Vec::new, |items| {
                        items.iter().filter_map(|i| text(i.get("name"))).collect()
                    });
                (
                    text(g.get("character")),
                    text(g.get("for")).unwrap_or_default(),
                    items,
                )
            })
            .collect()
    }

    fn two_playstyles() -> ConfigDriven {
        ConfigDriven::load(&Files {
            shipped: Some(shipped("trapsin")),
            others: vec![("fury-druid".to_owned(), shipped("fury-druid"))],
            quiet: true,
            ..Files::default()
        })
    }

    #[test]
    fn the_farm_panel_lists_the_other_characters_groups_after_the_own() -> Result<()> {
        let playstyle = two_playstyles();
        let vex = played_by("Vex", 6, 55)?;
        let wolf = Played::from([entry("BigBadWolf", 5, "fury druid", 10, &[])]);
        let shown: Vec<(Option<String>, String)> = farm_groups(&playstyle, &vex, &wolf)
            .into_iter()
            .map(|(who, build, _)| (who, build))
            .collect();
        assert_eq!(
            shown,
            [
                (None, "Trapsin".to_owned()),
                (Some("BigBadWolf".to_owned()), "Fury Druid".to_owned())
            ],
            "Vex is past the Fire Wake group"
        );
        // His recorded worn gear holds the entry for him; an assassin alongside her shares her groups.
        let wearing = Played::from([
            entry("BigBadWolf", 5, "fury druid", 10, &["Dimoak's Hew"]),
            entry("Starter", 6, "fire wake assassin", 12, &[]),
        ]);
        let groups = farm_groups(&playstyle, &vex, &wearing);
        let names = |groups: &[(Option<String>, String, Vec<String>)], who: Option<&str>| {
            groups
                .iter()
                .filter(|(w, _, _)| w.as_deref() == who)
                .map(|(_, build, items)| (build.clone(), items.clone()))
                .collect::<Vec<_>>()
        };
        let his = names(&groups, Some("BigBadWolf"));
        assert!(
            his.iter()
                .all(|(_, items)| !items.iter().any(|i| i == "Dimoak's Hew")),
            "{his:?}"
        );
        assert!(
            names(&farm_groups(&playstyle, &vex, &wolf), Some("BigBadWolf"))
                .iter()
                .any(|(_, items)| items.iter().any(|i| i == "Dimoak's Hew")),
            "missing while he doesn't wear it"
        );
        let own: Vec<String> = names(&groups, None).into_iter().map(|(b, _)| b).collect();
        assert_eq!(
            own,
            ["Fire Wake Assassin", "Trapsin"],
            "Starter still needs Fire Wake"
        );
        assert!(names(&groups, Some("Starter")).is_empty(), "no duplicate group");
        // Each other character's passed groups are hidden by its own stage.
        let wolf_plays = played_by("BigBadWolf", 5, 30)?;
        let farm_of_vex = |stage: &str, level: u8| {
            let vex = Played::from([entry("Vex", 6, stage, level, &[])]);
            let groups = farm_groups(&playstyle, &wolf_plays, &vex);
            names(&groups, Some("Vex"))
                .into_iter()
                .map(|(b, _)| b)
                .collect::<Vec<_>>()
        };
        assert_eq!(farm_of_vex("trapsin", 55), ["Trapsin"]);
        assert_eq!(
            farm_of_vex("fire wake assassin", 20),
            ["Fire Wake Assassin", "Trapsin"]
        );
        Ok(())
    }

    #[test]
    fn another_characters_missing_farm_entry_gets_the_farm_mark_on_the_ground() -> Result<()> {
        use d2r_core::guidance::MarkKind;
        use d2r_core::offsets::item_data;
        let mut vex = played_by("Vex", 6, 55)?;
        let mut hew = vex.drops.first().cloned().context("a fixture drop")?;
        hew.code = Some("bar".to_owned());
        hew.quality = item_data::QUALITY_UNIQUE;
        hew.flags = item_data::FLAG_IDENTIFIED;
        hew.sockets = 0;
        hew.unique_index = Some(48);
        vex.drops = vec![hew];
        let mark = |playstyle: &ConfigDriven, played: &Played| {
            let wants = playstyle.evaluate(&vex, None, played).wants;
            let mut drops = vex.drops.clone();
            let marks = playstyle.mark_drops(&mut drops, &wants);
            drops
                .first()
                .and_then(|d| marks.get(&d.id))
                .map(|m| (m.kind, m.rule.clone()))
        };
        let playstyle = two_playstyles();
        let wolf = Played::from([entry("BigBadWolf", 5, "fury druid", 10, &[])]);
        assert_eq!(
            mark(&playstyle, &wolf),
            Some((MarkKind::Farm, "Dimoak's Hew".to_owned()))
        );
        let wearing = Played::from([entry("BigBadWolf", 5, "fury druid", 10, &["Dimoak's Hew"])]);
        assert_eq!(mark(&playstyle, &wearing), None, "he wears it");
        let trapsin_only = ConfigDriven::load(&Files {
            shipped: Some(shipped("trapsin")),
            quiet: true,
            ..Files::default()
        });
        assert_eq!(mark(&trapsin_only, &wolf), None, "no fury farm list");
        Ok(())
    }

    #[test]
    fn a_selection_or_stage_of_a_removed_build_falls_back_to_the_class_default() -> Result<()> {
        let files = Files {
            shipped: Some(shipped("fury-druid")),
            others: vec![("trapsin".to_owned(), shipped("trapsin"))],
            quiet: true,
            ..Files::default()
        };
        let mut playstyle = ConfigDriven::load(&files);
        let select = |build: &str| d2r_core::guidance::Bis {
            build: build.to_owned(),
            slots: std::collections::BTreeMap::new(),
            merc_slots: std::collections::BTreeMap::new(),
        };
        playstyle.selected.config.bis = [
            ("bigbadwolf".to_owned(), select("summoner druid")),
            ("renamed".to_owned(), select("summoner-fury/fury druid")),
            ("vex".to_owned(), select("trapsin/trapsin")),
        ]
        .into();
        playstyle.share_selections();
        assert_eq!(
            stale_selections(&playstyle.folders()),
            [("bigbadwolf".to_owned(), "summoner druid".to_owned())],
            "a renamed folder still finds its build"
        );
        let fixture = include_str!("../../../tests/fixtures/flayer-marks-snapshot.json");
        let mut snap: Snapshot = serde_json::from_str(fixture).context("parsing fixture")?;
        "BigBadWolf".clone_into(&mut snap.player.name);
        let evaluation = playstyle.evaluate(&snap, Some("summoner druid"), &Played::new());
        assert_eq!(
            (
                evaluation.extra.stage.map(|s| (s.slug, s.class_default)),
                evaluation.wants.playstyle.as_str(),
                evaluation.reached
            ),
            (Some(("fury druid", true)), "fury-druid", Some("fury druid")),
            "the class default, and its stage replaces the old latch"
        );
        Ok(())
    }

    #[test]
    fn ground_drops_get_keep_farm_and_trade_marks() -> Result<()> {
        use d2r_core::guidance::MarkKind;
        // Level-50 druid without Harmony; drops: Hunter's Bow 4os, Monarch 4os, Long Sword
        // 0os, unidentified unique ring and Heavy Gloves.
        let snap: Snapshot =
            serde_json::from_str(include_str!("../../../tests/fixtures/flayer-marks-snapshot.json"))
                .context("parsing fixture")?;
        let playstyle = ConfigDriven::load(&Files {
            shipped: Some(shipped("fury-druid")),
            quiet: true,
            ..Files::default()
        });
        let wants = playstyle.evaluate(&snap, None, &Played::new()).wants;
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
                (Some("hbw"), Some(MarkKind::Keep), Some("Brand base"), false),
                (Some("uit"), Some(MarkKind::Trade), Some("Monarch 4os"), false),
                (Some("rin"), Some(MarkKind::Trade), Some("Stone of Jordan"), true),
                (Some("vgl"), Some(MarkKind::Keep), Some("Bloodfist?"), true),
            ],
            "the Long Sword 0os is gone"
        );
        let mut drops = snap.drops;
        let marks = playstyle.mark_drops(&mut drops, &PlaystyleWants::default());
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

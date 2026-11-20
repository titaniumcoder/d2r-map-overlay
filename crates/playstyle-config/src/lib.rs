//! The config-driven playstyle plugin ([`ConfigDriven`]): the builds, stages, farm list, route
//! and valuables tiers of a playstyle folder (`playstyles/<name>/`, with the user's overrides:
//! [`files`]; shipped: the Summoner → Fury Druid chain), behind the core's
//! [`d2r_core::playstyle::Playstyle`].
//!
//! What sits behind the trait:
//! - [`marks`]: the ground-drop marks: keep and farm from the item plan's wants
//!   ([`stash_plan::Wants`]), trade from [`valuables`] (rules from the folder's `valuables.json`);
//! - the builds, their stage chain and its latch (`config::Settings::stage`; the reached stage
//!   is persisted by the core in `played.json` as one opaque string per character);
//! - [`stash_plan`]: sell / move / keep of the player's items, with [`gear_score`] for magic and
//!   rare items;
//! - the area rating of the info line from the build's damage types ([`d2r_core::build`]);
//! - [`route`]: the leveling and farming route (`route.toml`) and its current step, from the
//!   character's quests per difficulty (remembered by the core: [`d2r_core::quest_log`]), with
//!   [`worth`] (the bosses still worth farming, from the drop tables);
//! - [`web`]: the dashboard's playstyle panels (gear grid, upgrades, recipes, tips, stage, farm
//!   list, sell / keep entries, the vendor notes and the skill plan for the core panels) and the
//!   `/config` page's JSON and build selection.

pub mod config;
mod config_driven;
pub mod files;
pub mod gear_score;
pub mod marks;
pub mod route;
pub mod stash_plan;
pub mod valuables;
pub mod web;
pub mod worth;

pub use config_driven::ConfigDriven;

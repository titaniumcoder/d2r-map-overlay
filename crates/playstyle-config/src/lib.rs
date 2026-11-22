//! The config-driven playstyle plugin ([`ConfigDriven`]): builds, stages, farm list, route and
//! valuables of a playstyle folder (`playstyles/<name>/`), behind [`d2r_core::playstyle::Playstyle`].

pub mod charms;
pub mod config;
mod config_driven;
pub mod files;
pub mod gear_score;
pub mod labels;
pub mod marks;
pub mod not_needed;
pub mod route;
pub mod stash_plan;
pub mod valuables;
pub mod web;
pub mod worth;

pub use config_driven::ConfigDriven;

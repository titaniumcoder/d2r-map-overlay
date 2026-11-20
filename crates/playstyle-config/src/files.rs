//! A playstyle's files: where they are ([`Files`]), loading them into [`Settings`] ([`load`],
//! [`reload`]) and the `/config` page's writes ([`select_build`], [`remove_character`]).
//!
//! A playstyle is a folder, `playstyles/<name>/` in the repo (shipped with the overlay; this
//! build's is `summoner-fury`):
//! - `playstyle.toml`: name, description, the `[valuables]` tiers, the `[damages]` defaults;
//! - `builds/*.toml`: the build definitions (next to the `*.source.json` they are generated
//!   from by `research/gen_build.py`);
//! - `farm.toml`: the farm list;
//! - `route.toml`: the leveling and farming route ([`crate::route`]);
//! - `valuables.json`: the drop mark rules, compiled in ([`crate::valuables`]; no override).
//!
//! Each file is looked up in three places; the first that has it wins, per file (per build
//! file in `builds/`):
//! 1. the user's folder `~/.config/d2r-map-overlay/playstyle/` (same relative paths);
//! 2. the old places, from before the folder layout (refactor step 7): `builds/` and
//!    `farm.toml` in `~/.config/d2r-map-overlay/`, and `config.toml`'s `[valuables]` and
//!    `[builds]` tables, each one present standing in for `playstyle.toml`'s `[valuables]` /
//!    `[damages]`;
//! 3. the shipped folder.
//!
//! The per-character build selection (`[bis.<name>]`) is state, not playstyle: `bis.toml` in
//! the data dir (`~/.local/share/d2r-map-overlay/`, next to `played.json`), written by the
//! `/config` page. An old `config.toml`'s `[bis]` entries are still read (`bis.toml` wins per
//! character) and a selection that sits there is edited there.
//!
//! Where the files are is the core's [`Files`] (the shipped folder is read at run time from the
//! workspace's `playstyles/`: a regenerated build file hot-reloads without a rebuild).

use std::collections::BTreeMap;
use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use d2r_core::playstyle::{DIR_VAR, Files};

use crate::config::{
    Bis, Build, Builds, FarmGroup, Settings, Valuables, parse_build, parse_farm, parse_playstyle,
    parse_tables,
};
use crate::route::{Route, parse_route};
use crate::web::config_page;

/// A new `bis.toml` (the comment heads the `[bis]` table, so it stays on top).
const BIS_HEADER: &str = "# Each character's build selection: [bis.<name>] build = \"<build slug>\".\n\
                          # Written by the overlay's /config page.\n\
                          [bis]\n";

/// Index of the old places in [`Files::layers`].
const OLD_PLACES: usize = 1;

/// One file's say on a setting.
enum Say<T> {
    /// No file, or nothing about this setting in it.
    Silent,
    Value(T),
    /// Unreadable or invalid (reported when read).
    Broken,
}

impl<T> Say<T> {
    /// The part `pick` takes from the file (`None`: the file says nothing about it).
    fn part<U>(&self, pick: impl FnOnce(&T) -> Option<U>) -> Say<U> {
        match self {
            Self::Silent => Say::Silent,
            Self::Broken => Say::Broken,
            Self::Value(value) => pick(value).map_or(Say::Silent, Say::Value),
        }
    }
}

/// The first say, in precedence order, that isn't silent, and its index: its value, or
/// `None` when that file is broken. The default (and no index) when no file speaks.
fn decide<T: Default>(says: impl IntoIterator<Item = Say<T>>) -> (Option<T>, Option<usize>) {
    for (i, say) in says.into_iter().enumerate() {
        match say {
            Say::Silent => {}
            Say::Value(value) => return (Some(value), Some(i)),
            Say::Broken => return (None, Some(i)),
        }
    }
    (Some(T::default()), None)
}

/// Reads `path` and parses it with `parse` (warnings reported): silent when there is no
/// such place or file.
fn read<T>(
    files: &Files,
    path: Option<&Path>,
    parse: impl FnOnce(&str) -> Result<(T, Vec<String>), toml::de::Error>,
) -> Say<T> {
    let Some(path) = path else {
        return Say::Silent;
    };
    let text = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Say::Silent,
        Err(e) => {
            files.say(format_args!("playstyle {}: {e}", path.display()));
            return Say::Broken;
        }
    };
    match parse(&text) {
        Ok((value, warnings)) => {
            for w in warnings {
                files.say(format_args!("playstyle {}: {w}", path.display()));
            }
            Say::Value(value)
        }
        Err(e) => {
            files.say(format_args!("playstyle {}: invalid:\n{e}", path.display()));
            Say::Broken
        }
    }
}

/// One read of every file: `None` for a setting whose deciding file is broken.
struct Loaded {
    /// `playstyle.toml`'s `name` (the shipped or the user's file).
    name: Option<String>,
    valuables: Option<Valuables>,
    damages: Option<Builds>,
    bis: Option<BTreeMap<String, Bis>>,
    build_defs: BTreeMap<String, Build>,
    farm: Option<Vec<FarmGroup>>,
    route: Option<Route>,
    /// The old-place files in use (for the notice).
    old: Vec<String>,
}

fn read_all(files: &Files) -> Loaded {
    let [user, _, shipped] = files.layers("playstyle.toml");
    let user_toml = read(files, user.as_deref(), parse_playstyle);
    let shipped_toml = read(files, shipped.as_deref(), parse_playstyle);
    let legacy = read(files, files.legacy_config().as_deref(), parse_tables);
    let mut old = Vec::new();
    // A user `playstyle.toml` decides whole (a table it leaves out is the default); an old
    // `config.toml` only with the tables it has.
    let (valuables, from) = decide([
        user_toml.part(|(_, t)| Some(t.valuables.clone().unwrap_or_default())),
        legacy.part(|t| t.valuables.clone()),
        shipped_toml.part(|(_, t)| Some(t.valuables.clone().unwrap_or_default())),
    ]);
    if from == Some(OLD_PLACES) {
        old.push("config.toml [valuables]".to_owned());
    }
    let (damages, from) = decide([
        user_toml.part(|(_, t)| Some(t.damages.clone().unwrap_or_default())),
        legacy.part(|t| t.damages.clone()),
        shipped_toml.part(|(_, t)| Some(t.damages.clone().unwrap_or_default())),
    ]);
    if from == Some(OLD_PLACES) && damages.as_ref().is_some_and(|d| *d != Builds::default()) {
        old.push("config.toml [builds]".to_owned());
    }
    let name = match (&user_toml, &shipped_toml) {
        (Say::Value((name, _)), _) | (Say::Silent, Say::Value((name, _))) => Some(name.clone()),
        _ => None,
    };
    // The selections: an old config.toml's, then bis.toml's over them per character.
    let state = read(files, files.bis_path().as_deref(), parse_tables);
    let bis = match (legacy.part(|t| t.bis.clone()), state.part(|t| t.bis.clone())) {
        (Say::Broken, _) | (_, Say::Broken) => None,
        (from_old, from_state) => {
            let mut bis = match from_old {
                Say::Value(bis) => bis,
                Say::Silent | Say::Broken => BTreeMap::new(),
            };
            if let Say::Value(newer) = from_state {
                bis.extend(newer);
            }
            Some(bis)
        }
    };
    let (farm, from) = decide(
        files
            .layers("farm.toml")
            .iter()
            .map(|path| read(files, path.as_deref(), parse_farm)),
    );
    if from == Some(OLD_PLACES) {
        old.push("farm.toml".to_owned());
    }
    // No old place for the route: it is newer than the folder layout.
    let [user_route, _, shipped_route] = files.layers("route.toml");
    let (route, _) =
        decide([user_route, shipped_route].map(|path| read(files, path.as_deref(), parse_route)));
    let build_defs = load_builds(files, &mut old);
    Loaded {
        name,
        valuables,
        damages,
        bis,
        build_defs,
        farm,
        route,
        old,
    }
}

/// Every build definition, keyed by lowercased build name: per file name, the user's file
/// wins over the old place's over the shipped one. Only `*.toml` files count; an unreadable,
/// invalid or nameless one is reported and skipped. Notes the old-place files used in `old`.
fn load_builds(files: &Files, old: &mut Vec<String>) -> BTreeMap<String, Build> {
    // File name → (place index, path); the shipped place first, so the better ones overwrite.
    let mut chosen: BTreeMap<OsString, (usize, PathBuf)> = BTreeMap::new();
    for (i, dir) in files.layers("builds").into_iter().enumerate().rev() {
        let Some(entries) = dir.and_then(|d| std::fs::read_dir(d).ok()) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().is_some_and(|e| e == "toml") {
                chosen.insert(entry.file_name(), (i, path));
            }
        }
    }
    let mut builds = BTreeMap::new();
    for (file, (place, path)) in chosen {
        if place == OLD_PLACES {
            old.push(format!("builds/{}", file.to_string_lossy()));
        }
        let text = match std::fs::read_to_string(&path) {
            Ok(text) => text,
            Err(e) => {
                files.say(format_args!("build {}: {e}; skipped", path.display()));
                continue;
            }
        };
        match parse_build(&text) {
            Ok((slug, build, warnings)) => {
                for w in warnings {
                    files.say(format_args!("build {}: {w}", path.display()));
                }
                if slug.is_empty() {
                    files.say(format_args!("build {}: no name; skipped", path.display()));
                } else {
                    builds.insert(slug, build);
                }
            }
            Err(e) => files.say(format_args!("build {}: invalid, skipped:\n{e}", path.display())),
        }
    }
    builds
}

/// Tells which old-place files are in use: they hide the shipped ones of the same name.
fn note_old_places(files: &Files, old: &[String]) {
    if let (false, Some(dir)) = (old.is_empty(), &files.legacy) {
        files.say(format_args!(
            "playstyle: old places in {} hide the shipped files: {}; delete those files and \
             tables, or move the files to playstyle/ there to keep them as overrides",
            dir.display(),
            old.join(", ")
        ));
    }
}

/// Loads the playstyle's settings for the start. A broken file's settings are the defaults
/// (reported).
#[must_use]
pub fn load(files: &Files) -> Settings {
    let loaded = read_all(files);
    if loaded.name.is_none() && loaded.build_defs.is_empty() {
        // No playstyle.toml and no build file anywhere: no folder found and no old files.
        files.say(format_args!(
            "error: playstyle: no playstyle files found (no playstyle folder, no user or old \
             files): no builds, stages or farm list; see README \"Playstyles\" ({DIR_VAR})"
        ));
    } else {
        files.say(format_args!(
            "playstyle: {} ({} builds)",
            loaded.name.as_deref().unwrap_or("unnamed"),
            loaded.build_defs.len()
        ));
    }
    note_old_places(files, &loaded.old);
    Settings {
        valuables: loaded.valuables.unwrap_or_default(),
        builds: loaded.damages.unwrap_or_default(),
        bis: loaded.bis.unwrap_or_default(),
        build_defs: loaded.build_defs,
        farm: loaded.farm.unwrap_or_default(),
        route: loaded.route.unwrap_or_default(),
    }
}

/// Reloads `settings` after a change on disk. A setting whose deciding file is broken (a
/// half-written file, a typo) keeps its current value, so a broken file never blanks the
/// overlay; a removed file falls back to the next place (none left: the default). The build
/// files always reload (a broken one is skipped).
pub fn reload(settings: &mut Settings, files: &Files) {
    let loaded = read_all(files);
    note_old_places(files, &loaded.old);
    replace(
        files,
        &mut settings.valuables,
        loaded.valuables,
        "valuables tiers",
    );
    replace(files, &mut settings.builds, loaded.damages, "damages");
    replace(files, &mut settings.bis, loaded.bis, "build selections");
    let farm_groups = loaded.farm.as_ref().map(Vec::len);
    replace(files, &mut settings.farm, loaded.farm, "farm list");
    replace(files, &mut settings.route, loaded.route, "route");
    settings.build_defs = loaded.build_defs;
    files.say(format_args!(
        "playstyle reloaded ({} builds{})",
        settings.build_defs.len(),
        farm_groups.map_or_else(String::new, |n| format!(", {n} farm groups"))
    ));
}

fn replace<T>(files: &Files, current: &mut T, new: Option<T>, what: &str) {
    match new {
        Some(value) => *current = value,
        None => files.say(format_args!("playstyle: keeping the current {what}")),
    }
}

/// Sets (`Some` slug) or clears (`None`) character `name`'s build selection, comments kept.
/// It is written where it lives: an old `config.toml`'s `[bis.<name>]` when there is one,
/// else `bis.toml` (created by the first selection).
///
/// # Errors
/// A file that can't be read or written, isn't valid TOML or whose `bis` entry isn't a table;
/// no data dir.
pub fn select_build(files: &Files, name: &str, build: Option<&str>) -> Result<()> {
    if let Some(path) = files.legacy_config()
        && let Some(text) = read_existing(&path)?
        && has_bis_entry(&text, name)
    {
        return edit(&path, &text, |t| config_page::set_bis_selection(t, name, build));
    }
    let path = files
        .bis_path()
        .context("no data directory ($XDG_DATA_HOME and $HOME unset)")?;
    let text = read_existing(&path)?.unwrap_or_else(|| BIS_HEADER.to_owned());
    edit(&path, &text, |t| config_page::set_bis_selection(t, name, build))
}

/// Forgets character `name`'s selection and damage entries in `bis.toml` and an old
/// `config.toml` (every spelling, comments kept).
///
/// # Errors
/// As [`select_build`]; an old `config.toml` is edited first, so a broken one changes
/// nothing.
pub fn remove_character(files: &Files, name: &str) -> Result<()> {
    for path in [files.legacy_config(), files.bis_path()].into_iter().flatten() {
        if let Some(text) = read_existing(&path)? {
            edit(&path, &text, |t| config_page::remove_character(t, name))?;
        }
    }
    Ok(())
}

/// The file's text; `None` when it doesn't exist.
fn read_existing(path: &Path) -> Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e).with_context(|| format!("reading {}", path.display())),
    }
}

/// Whether `text` has a `[bis.<name>]` entry (any spelling).
fn has_bis_entry(text: &str, name: &str) -> bool {
    let Ok(doc) = text.parse::<toml_edit::DocumentMut>() else {
        return false;
    };
    doc.get("bis")
        .and_then(toml_edit::Item::as_table_like)
        .is_some_and(|bis| !config_page::matching_keys(bis.iter().map(|(k, _)| k), name).is_empty())
}

/// Writes `change(text)` over `path` (atomically, creating its folder), unless nothing
/// changed.
fn edit(path: &Path, text: &str, change: impl FnOnce(&str) -> Option<String>) -> Result<()> {
    let changed = change(text).with_context(|| format!("{} is not valid TOML", path.display()))?;
    if changed == text {
        return Ok(());
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    d2r_core::web::write_atomic(path, &changed).with_context(|| format!("writing {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::owns_nothing;
    use crate::valuables::Tier;
    use anyhow::{Result, anyhow};
    use d2r_core::testutil::temp_dir;

    /// The repo's shipped playstyle.
    fn shipped() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../playstyles/summoner-fury")
    }

    fn write(path: &Path, text: &str) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, text)?;
        Ok(())
    }

    /// A one-group farm list for `target`.
    fn farm(target: &str) -> String {
        format!("[[farm]]\nfor = \"{target}\"\nitems = [{{ name = \"Smoke\", kind = \"runeword\" }}]\n")
    }

    fn farm_targets(settings: &Settings) -> Vec<&str> {
        settings.farm.iter().map(|g| g.target.as_str()).collect()
    }

    fn notes_of<'a>(settings: &'a Settings, slug: &str) -> Option<&'a str> {
        settings.build_defs.get(slug).and_then(|b| b.notes.as_deref())
    }

    #[test]
    fn the_shipped_playstyle_parses_and_chains() -> Result<()> {
        let dir = shipped();
        // Every file parses without a warning.
        let ((name, tables), warnings) =
            parse_playstyle(&std::fs::read_to_string(dir.join("playstyle.toml"))?)?;
        assert_eq!((name.as_str(), warnings), ("Summoner Druid → Fury Druid", vec![]));
        assert_eq!(tables.valuables, Some(Valuables::default()));
        assert_eq!(tables.damages, Some(Builds::default()));
        let (groups, warnings) = parse_farm(&std::fs::read_to_string(dir.join("farm.toml"))?)?;
        assert_eq!(warnings, Vec::<String>::new());
        assert_eq!(
            groups.iter().map(|g| g.target.as_str()).collect::<Vec<_>>(),
            ["Summoner Druid", "Fury Druid"]
        );
        for file in ["summoner-druid", "fury-druid"] {
            let text = std::fs::read_to_string(dir.join(format!("builds/{file}.toml")))?;
            let (_, _, warnings) = parse_build(&text)?;
            assert_eq!(warnings, Vec::<String>::new(), "{file}");
            assert!(
                dir.join(format!("builds/{file}.source.json")).is_file(),
                "{file} source"
            );
        }
        assert!(dir.join("valuables.json").is_file(), "the compiled-in mark rules");
        let (_, warnings) = parse_route(&std::fs::read_to_string(dir.join("route.toml"))?)?;
        assert_eq!(warnings, Vec::<String>::new(), "route.toml");
        // Loaded alone, the summoner is the druid's default and chains into Fury.
        let files = Files {
            shipped: Some(dir),
            ..Files::default()
        };
        let settings = load(&files);
        assert_eq!(
            settings.build_defs.keys().collect::<Vec<_>>(),
            ["fury druid", "summoner druid"]
        );
        assert_eq!(settings.farm.len(), 2);
        assert_eq!(settings.route.step.len(), 16);
        assert_eq!(
            settings.class_default(5).map(|(slug, _)| slug),
            Some("summoner druid")
        );
        assert_eq!(
            settings.stage_chain("summoner druid"),
            ["Summoner Druid", "Fury Druid"]
        );
        assert!(settings.leads_to("summoner druid", "fury druid"));
        let stage = |level, owns: &dyn Fn(&str) -> u32| {
            settings
                .stage("anyone", Some(5), Some(level), owns, None)
                .map(|s| s.slug)
        };
        assert_eq!(stage(80, &owns_nothing), Some("summoner druid"), "no switch gear");
        assert_eq!(
            stage(80, &|_| 2),
            Some("fury druid"),
            "the gear owned: Fury from 75"
        );
        assert_eq!(stage(70, &|_| 2), Some("summoner druid"), "not before 75");
        Ok(())
    }

    #[test]
    fn user_files_win_over_old_places_over_shipped() -> Result<()> {
        let dir = temp_dir("playstyle-layers")?;
        let files = Files {
            shipped: Some(dir.join("shipped")),
            user: Some(dir.join("config/playstyle")),
            legacy: Some(dir.join("config")),
            state: Some(dir.join("data")),
            quiet: false,
        };
        let build = |name: &str, notes: &str| format!("name = \"{name}\"\nnotes = \"{notes}\"\n");
        write(
            &dir.join("shipped/playstyle.toml"),
            "name = \"S\"\n[valuables]\ntrade = [\"B\"]\n[damages]\nVex = [\"cold\"]\n",
        )?;
        write(&dir.join("shipped/farm.toml"), &farm("shipped"))?;
        write(&dir.join("shipped/builds/a.toml"), &build("A", "shipped"))?;
        write(&dir.join("shipped/builds/b.toml"), &build("B", "shipped"))?;
        write(&dir.join("shipped/builds/c.toml"), &build("C", "shipped"))?;
        write(&dir.join("shipped/builds/c.source.json"), "{}")?;
        let route = |id: &str| format!("[[step]]\nid = \"{id}\"\n");
        let route_ids = |settings: &Settings| -> Vec<String> {
            settings.route.step.iter().map(|s| s.id.clone()).collect()
        };
        write(&dir.join("shipped/route.toml"), &route("shipped"))?;
        // Shipped only.
        let settings = load(&files);
        assert_eq!(route_ids(&settings), ["shipped"]);
        assert_eq!(settings.valuables.trade, [Tier::B]);
        assert_eq!(farm_targets(&settings), ["shipped"]);
        assert_eq!(settings.build_defs.len(), 3, "only the .toml files");
        // The old places win over the shipped files: per file, per table.
        write(
            &dir.join("config/config.toml"),
            "[shrines]\nshow = []\n[valuables]\ntrade = [\"A\"]\n",
        )?;
        write(&dir.join("config/farm.toml"), &farm("old"))?;
        write(&dir.join("config/builds/b.toml"), &build("B", "old"))?;
        write(&dir.join("config/route.toml"), &route("old"))?;
        let settings = load(&files);
        assert_eq!(route_ids(&settings), ["shipped"], "no old place for the route");
        assert_eq!(settings.valuables.trade, [Tier::A], "config.toml [valuables]");
        assert_eq!(
            settings.builds.names.get("vex").map(Vec::as_slice),
            Some(&[d2r_core::build::Damage::Cold][..]),
            "no [builds] in config.toml: the shipped [damages]"
        );
        assert_eq!(farm_targets(&settings), ["old"]);
        assert_eq!(notes_of(&settings, "b"), Some("old"));
        assert_eq!(notes_of(&settings, "a"), Some("shipped"));
        // The user's folder wins over both, a whole file at a time.
        write(&dir.join("config/playstyle/playstyle.toml"), "name = \"U\"\n")?;
        write(&dir.join("config/playstyle/farm.toml"), &farm("user"))?;
        write(&dir.join("config/playstyle/builds/a.toml"), &build("A", "user"))?;
        write(&dir.join("config/playstyle/builds/b.toml"), &build("B", "user"))?;
        write(&dir.join("config/playstyle/route.toml"), &route("user"))?;
        let settings = load(&files);
        assert_eq!(route_ids(&settings), ["user"]);
        assert_eq!(
            settings.valuables,
            Valuables::default(),
            "the user's playstyle.toml decides whole: no [valuables] = the defaults"
        );
        assert_eq!(settings.builds, Builds::default());
        assert_eq!(farm_targets(&settings), ["user"]);
        assert_eq!(notes_of(&settings, "a"), Some("user"));
        assert_eq!(notes_of(&settings, "b"), Some("user"));
        assert_eq!(notes_of(&settings, "c"), Some("shipped"));
        std::fs::remove_dir_all(&dir)?;
        Ok(())
    }

    #[test]
    fn the_old_places_still_load() -> Result<()> {
        let dir = temp_dir("playstyle-old")?;
        // The setup from before the folder layout: everything next to config.toml.
        write(
            &dir.join("config.toml"),
            "# mine\n[valuables]\ntwink = []\n[shrines]\nshow = [\"skill\"]\n\
             [builds]\nVex = [\"fire\"]\n[builds.class]\ndruid = [\"physical\"]\n\
             [bis.Vex]\nbuild = \"x build\"\n",
        )?;
        write(
            &dir.join("builds/x.toml"),
            "name = \"X Build\"\nclass = \"druid\"\n",
        )?;
        write(&dir.join("farm.toml"), &farm("X Build"))?;
        let files = Files {
            shipped: Some(dir.join("no-such-playstyle")),
            legacy: Some(dir.clone()),
            ..Files::default()
        };
        let settings = load(&files);
        assert_eq!(settings.valuables.twink, []);
        assert_eq!(settings.valuables.trade, [Tier::S, Tier::A]);
        assert_eq!(
            settings.builds.names.get("vex").map(Vec::as_slice),
            Some(&[d2r_core::build::Damage::Fire][..])
        );
        assert_eq!(settings.builds.classes.len(), 1);
        assert_eq!(settings.bis.get("vex").map(|b| b.build.as_str()), Some("x build"));
        assert_eq!(settings.build_defs.keys().collect::<Vec<_>>(), ["x build"]);
        assert_eq!(farm_targets(&settings), ["X Build"]);
        // The core reads the same file without a complaint.
        let config = d2r_core::config::load_from(&dir.join("config.toml"));
        assert_eq!(config.shrines.show, [12]);
        std::fs::remove_dir_all(&dir)?;
        Ok(())
    }

    #[test]
    fn selections_live_in_bis_toml_unless_config_toml_has_them() -> Result<()> {
        let dir = temp_dir("playstyle-bis")?;
        let files = Files {
            legacy: Some(dir.join("config")),
            state: Some(dir.join("data")),
            ..Files::default()
        };
        let old = dir.join("config/config.toml");
        let bis = dir.join("data/bis.toml");
        let old_text = "# keep me\n[shrines]\nshow = []\n\n[bis.Old]\nbuild = \"a\"\n";
        write(&old, old_text)?;
        let selected = |name: &str| load(&files).bis.get(name).map(|b| b.build.clone());
        // A new character: bis.toml (created, with its header); config.toml untouched.
        select_build(&files, "New One", Some("b"))?;
        assert_eq!(std::fs::read_to_string(&old)?, old_text);
        let text = std::fs::read_to_string(&bis)?;
        assert!(text.starts_with(BIS_HEADER), "{text}");
        assert_eq!(selected("new one").as_deref(), Some("b"));
        // A character selected in config.toml: edited there, comments kept.
        select_build(&files, "OLD", Some("c"))?;
        let text = std::fs::read_to_string(&old)?;
        assert!(
            text.contains("# keep me") && text.contains("build = \"c\""),
            "{text}"
        );
        assert!(!std::fs::read_to_string(&bis)?.contains("Old"));
        assert_eq!(selected("old").as_deref(), Some("c"));
        // bis.toml wins over config.toml per character.
        write(&bis, &format!("{BIS_HEADER}[bis.old]\nbuild = \"z\"\n"))?;
        assert_eq!(selected("old").as_deref(), Some("z"));
        // Clearing removes the entry; removing a character empties both files of it.
        select_build(&files, "new one", Some("b"))?;
        select_build(&files, "New One", None)?;
        assert_eq!(selected("new one"), None);
        remove_character(&files, "old")?;
        assert_eq!(load(&files).bis, BTreeMap::new());
        assert!(std::fs::read_to_string(&old)?.contains("# keep me"));
        // Nothing to do: no write, no error.
        std::fs::remove_file(&bis)?;
        select_build(&files, "Nobody", None)?;
        remove_character(&files, "Nobody")?;
        assert!(!bis.exists(), "clearing nothing creates no file");
        // A broken config.toml: the removal fails before anything changes.
        write(&old, "[bis\n")?;
        assert!(remove_character(&files, "x").is_err());
        // No data dir: a selection can't be saved.
        let nowhere = Files::default();
        assert!(select_build(&nowhere, "X", Some("b")).is_err());
        std::fs::remove_dir_all(&dir)?;
        Ok(())
    }

    #[test]
    fn a_broken_file_keeps_what_it_decided() -> Result<()> {
        let dir = temp_dir("playstyle-reload")?;
        let files = Files {
            shipped: Some(dir.join("shipped")),
            legacy: Some(dir.join("config")),
            ..Files::default()
        };
        write(&dir.join("shipped/playstyle.toml"), "name = \"S\"\n")?;
        write(&dir.join("shipped/farm.toml"), &farm("shipped"))?;
        write(&dir.join("config/config.toml"), "[valuables]\ntrade = [\"S\"]\n")?;
        write(&dir.join("config/farm.toml"), &farm("old"))?;
        let mut settings = load(&files);
        assert_eq!(
            (settings.valuables.trade.as_slice(), farm_targets(&settings)),
            (&[Tier::S][..], vec!["old"])
        );
        // Half-written farm list: the old list stays; the valuables reload.
        write(
            &dir.join("config/farm.toml"),
            "[[farm]]\nfor = \"B\"\nitems = [{ name = \"Sm",
        )?;
        write(&dir.join("config/config.toml"), "[valuables]\ntrade = [\"A\"]\n")?;
        reload(&mut settings, &files);
        assert_eq!(farm_targets(&settings), ["old"], "kept");
        assert_eq!(settings.valuables.trade, [Tier::A]);
        // A broken config.toml keeps the valuables; the fixed farm list reloads.
        write(&dir.join("config/config.toml"), "[valuables\n")?;
        write(&dir.join("config/farm.toml"), &farm("fixed"))?;
        reload(&mut settings, &files);
        assert_eq!(settings.valuables.trade, [Tier::A], "kept");
        assert_eq!(farm_targets(&settings), ["fixed"]);
        // Removed: the next place's (the shipped list), then none.
        std::fs::remove_file(dir.join("config/farm.toml"))?;
        reload(&mut settings, &files);
        assert_eq!(farm_targets(&settings), ["shipped"]);
        std::fs::remove_file(dir.join("shipped/farm.toml"))?;
        reload(&mut settings, &files);
        assert_eq!(settings.farm, []);
        // The first load of a broken file: the defaults.
        write(&dir.join("config/farm.toml"), "[[farm]]\nfor = 3\n")?;
        assert_eq!(load(&files).farm, []);
        assert_eq!(load(&files).valuables, Valuables::default());
        std::fs::remove_dir_all(&dir)?;
        Ok(())
    }

    #[test]
    fn build_files_load_and_skip_the_broken_ones() -> Result<()> {
        let dir = temp_dir("playstyle-builds")?;
        let builds = dir.join("builds");
        write(
            &builds.join("blood-boil.toml"),
            "name = \"Blood Boil\"\ndamages = [\"physical\", \"fire\"]\nnotes = \"guide\"\n\
             [slots]\nweapon = [\"Heart of the Oak@55\", \" \"]\n\
             [merc_slots]\nweapon = [\"Insight@27\"]",
        )?;
        write(&builds.join("broken.toml"), "name = [\n")?;
        write(&builds.join("nameless.toml"), "damages = [\"fire\"]\n")?;
        write(&builds.join("ignored.txt"), "name = \"Nope\"\n")?;
        let files = Files {
            shipped: Some(dir.clone()),
            ..Files::default()
        };
        let mut old = Vec::new();
        let defs = load_builds(&files, &mut old);
        assert_eq!(defs.keys().collect::<Vec<_>>(), ["blood boil"], "{defs:?}");
        let def = defs.get("blood boil").ok_or_else(|| anyhow!("no blood boil"))?;
        assert_eq!(def.name, "Blood Boil");
        assert_eq!(def.notes.as_deref(), Some("guide"));
        assert_eq!(
            def.slots.get("weapon").map(Vec::as_slice),
            Some(&["Heart of the Oak@55".to_owned()][..]),
            "blank pieces dropped"
        );
        assert_eq!(old, Vec::<String>::new(), "no old place used");
        // No folder anywhere: no builds, no error.
        assert!(load_builds(&Files::default(), &mut old).is_empty());
        std::fs::remove_dir_all(&dir)?;
        Ok(())
    }
}

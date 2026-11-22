//! Where a playstyle's files are; the core locates and watches them, the playstyle loads them.
//! The shipped folder is read at run time from the source tree, so edits need no rebuild.

use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// `None`: no such place on this machine.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Files {
    /// `playstyles/<name>/`.
    pub shipped: Option<PathBuf>,
    /// `~/.config/d2r-map-overlay/playstyle/`.
    pub user: Option<PathBuf>,
    /// The config dir holding the pre-folder-layout files (`~/.config/d2r-map-overlay/`).
    pub legacy: Option<PathBuf>,
    /// Holds `bis.toml` (`~/.local/share/d2r-map-overlay/`).
    pub state: Option<PathBuf>,
    /// No load messages: the web server loads per request, the start already reported them.
    pub quiet: bool,
}

impl Files {
    #[must_use]
    pub fn new(name: &str) -> Self {
        let config_dir = crate::config::default_path().and_then(|p| p.parent().map(Path::to_path_buf));
        Self {
            shipped: shipped_dir(name),
            user: config_dir.as_ref().map(|d| d.join("playstyle")),
            legacy: config_dir,
            state: crate::journal::data_dir(),
            quiet: false,
        }
    }

    #[must_use]
    pub fn quiet(&self) -> Self {
        Self {
            quiet: true,
            ..self.clone()
        }
    }

    pub fn say(&self, message: std::fmt::Arguments) {
        if !self.quiet {
            eprintln!("{message}");
        }
    }

    /// `rel` in each place, the winning one first.
    #[must_use]
    pub fn layers(&self, rel: &str) -> [Option<PathBuf>; 3] {
        [&self.user, &self.legacy, &self.shipped].map(|dir| dir.as_ref().map(|d| d.join(rel)))
    }

    #[must_use]
    pub fn bis_path(&self) -> Option<PathBuf> {
        self.state.as_ref().map(|d| d.join("bis.toml"))
    }

    #[must_use]
    pub fn legacy_config(&self) -> Option<PathBuf> {
        self.legacy.as_ref().map(|d| d.join("config.toml"))
    }

    /// Every file a load may read with its mtime, sorted, for the hot reload to compare.
    #[must_use]
    pub fn fingerprint(&self) -> Vec<(PathBuf, Option<SystemTime>)> {
        let mut out: Vec<_> = self
            .watched_paths()
            .into_iter()
            .map(|p| {
                let modified = std::fs::metadata(&p).and_then(|m| m.modified()).ok();
                (p, modified)
            })
            .collect();
        out.sort();
        out
    }

    /// The fixed files of each place plus its `builds/*.toml`, so added and removed builds show.
    fn watched_paths(&self) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        for dir in [&self.user, &self.legacy, &self.shipped].into_iter().flatten() {
            paths.extend(["playstyle.toml", "farm.toml", "route.toml", "config.toml"].map(|f| dir.join(f)));
            if let Ok(entries) = std::fs::read_dir(dir.join("builds")) {
                paths.extend(
                    entries
                        .flatten()
                        .map(|e| e.path())
                        .filter(|p| p.extension().is_some_and(|e| e == "toml")),
                );
            }
        }
        paths.extend(self.bis_path());
        paths
    }
}

/// `playstyles/` at the workspace root of the source tree the binary was built in.
fn source_playstyles() -> PathBuf {
    let crate_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    crate_dir
        .ancestors()
        .nth(2)
        .unwrap_or(crate_dir)
        .join("playstyles")
}

/// Names the folder holding the playstyle folders, instead of the source tree's.
pub const DIR_VAR: &str = "D2R_PLAYSTYLES_DIR";

/// `None` (reported) when no root has it or `name` isn't a plain folder name.
fn shipped_dir(name: &str) -> Option<PathBuf> {
    let roots = playstyle_roots();
    let found = find_shipped(name, &roots);
    if found.is_none() {
        let looked: Vec<String> = roots.iter().map(|r| r.join(name).display().to_string()).collect();
        eprintln!(
            "error: playstyle {name:?}: no playstyle folder (looked in {}); set {DIR_VAR} to the \
             folder holding the playstyle folders. Only the user's and the old files count.",
            looked.join(", ")
        );
    }
    found
}

/// `$D2R_PLAYSTYLES_DIR` alone when set (an explicit choice), else the source tree's
/// `playstyles/`, then the one next to the binary.
fn playstyle_roots() -> Vec<PathBuf> {
    std::env::var_os(DIR_VAR).map_or_else(
        || {
            let mut roots = vec![source_playstyles()];
            if let Some(dir) = std::env::current_exe()
                .ok()
                .and_then(|exe| exe.parent().map(Path::to_path_buf))
            {
                roots.push(dir.join("playstyles"));
            }
            roots
        },
        |dir| vec![PathBuf::from(dir)],
    )
}

/// `<root>/<name>/` in the first of `roots` that has it; `None` for a name that isn't plain
/// (letters, digits, `-`, `_`).
fn find_shipped(name: &str, roots: &[PathBuf]) -> Option<PathBuf> {
    let plain = !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !plain {
        return None;
    }
    roots.iter().map(|root| root.join(name)).find(|dir| dir.is_dir())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::temp_dir;
    use anyhow::Result;

    fn write(path: &Path, text: &str) -> Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        std::fs::write(path, text)?;
        Ok(())
    }

    #[test]
    fn the_shipped_folder_is_the_first_root_that_has_it() -> Result<()> {
        let dir = temp_dir("playstyle-roots")?;
        let (a, b) = (dir.join("a"), dir.join("b"));
        std::fs::create_dir_all(b.join("mine"))?;
        let roots = [a.clone(), b.clone()];
        assert_eq!(find_shipped("mine", &roots), Some(b.join("mine")), "a lacks it");
        std::fs::create_dir_all(a.join("mine"))?;
        assert_eq!(
            find_shipped("mine", &roots),
            Some(a.join("mine")),
            "the first wins"
        );
        assert_eq!(
            find_shipped("other", &roots),
            None,
            "missing: none, not a phantom path"
        );
        write(&a.join("file"), "")?;
        assert_eq!(find_shipped("file", &[a]), None, "a file is no folder");
        for bad in ["", "../b/mine", "a/b", "x y"] {
            assert_eq!(find_shipped(bad, &roots), None, "{bad:?}");
        }
        // The source tree's own folder is found.
        let repo = [source_playstyles()];
        assert_eq!(
            find_shipped("summoner-fury", &repo),
            Some(source_playstyles().join("summoner-fury"))
        );
        std::fs::remove_dir_all(&dir)?;
        Ok(())
    }

    #[test]
    fn the_fingerprint_sees_added_and_edited_files() -> Result<()> {
        let dir = temp_dir("playstyle-fingerprint")?;
        let files = Files {
            shipped: Some(dir.join("shipped")),
            state: Some(dir.join("data")),
            ..Files::default()
        };
        let before = files.fingerprint();
        write(&dir.join("shipped/builds/a.toml"), "name = \"A\"\n")?;
        let added = files.fingerprint();
        assert_ne!(before, added);
        write(&dir.join("data/bis.toml"), "")?;
        let selected = files.fingerprint();
        assert_ne!(added, selected);
        write(&dir.join("shipped/builds/a.source.json"), "{}")?;
        assert_eq!(files.fingerprint(), selected, "only the build files count");
        std::fs::File::options()
            .write(true)
            .open(dir.join("shipped/builds/a.toml"))?
            .set_modified(SystemTime::UNIX_EPOCH)?;
        assert_ne!(selected, files.fingerprint(), "an in-place edit");
        std::fs::remove_dir_all(&dir)?;
        Ok(())
    }
}

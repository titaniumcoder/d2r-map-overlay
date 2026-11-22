//! A build's charm wishlist (`[[charms]]` in `builds/<slug>.toml`): unique charms by name and
//! magic charms by size and the valuables' stat conditions, each wanted `count` times. The
//! gear grid lists every wish with how many owned charms fill it ([`found`]); the stash plan
//! keeps a matching charm for own use ([`CharmWish::matches`]).

use serde::Deserialize;

use crate::valuables::{self, StatNeed};
use d2r_core::game::ItemDrop;
use d2r_core::item_names_data::{DISPLAY_NAMES, UNIQUE_NAMES};
use d2r_core::items::{CharmSize, DropKind};
use d2r_core::names::same_name;
use d2r_core::offsets::item_data;

/// One `[[charms]]` entry.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CharmWish {
    /// A unique charm's name, or the label of a magic one ("Shape Shifting skiller").
    pub name: String,
    /// A unique charm, matched by `name`; else a magic charm of `size`.
    #[serde(default)]
    pub unique: bool,
    /// Magic charms: the size to match; a unique's is its own.
    #[serde(default)]
    pub size: Option<CharmSize>,
    /// Copies wanted.
    #[serde(default = "one")]
    pub count: u32,
    /// A unique's required level (shown).
    #[serde(default)]
    pub level: Option<u32>,
    /// Conditions on the charm's read stats, all needed (the valuables' vocabulary); a skill
    /// tree or class skills count only for the build's class. A magic wish needs some.
    #[serde(default)]
    pub stats: Vec<StatNeed>,
    #[serde(default)]
    pub why: String,
}

const fn one() -> u32 {
    1
}

impl CharmWish {
    /// Whether `item` fills this wish for a character of `class`: an identified unique of
    /// this name (also "Latent <name>", the Sunder as it drops, and "Renewed <name>", its
    /// crafted form), or a magic charm of this size;
    /// either way with every `stats` condition met.
    #[must_use]
    pub fn matches(&self, item: &ItemDrop, class: Option<u32>) -> bool {
        let DropKind::Charm { size, unique } = item.kind else {
            return false;
        };
        let kind_fits = if self.unique {
            unique
                && valuables::unique_set_name(item).is_some_and(|n| {
                    same_name(n, &self.name)
                        || ["Latent ", "Renewed "].iter().any(|form| {
                            n.strip_prefix(form)
                                .is_some_and(|bare| same_name(bare, &self.name))
                        })
                })
        } else {
            !unique
                && item.quality == item_data::QUALITY_MAGIC
                && self.size == Some(size)
                && !item.stats.is_empty()
        };
        kind_fits && self.stats.iter().all(|need| need.holds(&item.stats, class))
    }

    /// The `stats` conditions as dashboard lines.
    #[must_use]
    pub fn describe(&self) -> Vec<String> {
        self.stats.iter().map(StatNeed::describe).collect()
    }
}

/// The valid wishes of `raw`; the others are warned about and dropped: a blank name, an
/// unknown unique name, a magic wish without a size or stats, a bad stat condition. A
/// `count` of 0 is warned and taken as 1; a repeated name keeps the first.
#[must_use]
pub fn clean(raw: Vec<CharmWish>, label: &str, warnings: &mut Vec<String>) -> Vec<CharmWish> {
    let mut out: Vec<CharmWish> = Vec::new();
    for mut wish in raw {
        let name = wish.name.trim().to_owned();
        wish.name = name;
        let problem = if wish.name.is_empty() {
            Some("has no name".to_owned())
        } else if wish.unique && !known_unique(&wish.name) {
            Some("is no unique item".to_owned())
        } else if !wish.unique && (wish.size.is_none() || wish.stats.is_empty()) {
            Some("is a magic charm without size or stats".to_owned())
        } else if out.iter().any(|w| same_name(&w.name, &wish.name)) {
            Some("is listed twice".to_owned())
        } else {
            wish.stats.iter().find_map(|need| need.check().err())
        };
        if let Some(problem) = problem {
            warnings.push(format!("{label}: charm {:?} {problem}; ignored", wish.name));
            continue;
        }
        if wish.count == 0 {
            warnings.push(format!("{label}: charm {:?} count 0 taken as 1", wish.name));
            wish.count = 1;
        }
        out.push(wish);
    }
    out
}

fn known_unique(name: &str) -> bool {
    UNIQUE_NAMES.iter().any(|&(_, n)| same_name(n, name))
        || DISPLAY_NAMES.iter().any(|&(_, n)| same_name(n, name))
}

/// How many of `items` fill each of `wishes` for `class`, at most its `count`: each charm
/// counts once, for the first wish in order it matches that still wants one (a 5 all res, 20
/// life small charm fills a life wish before a resistance one listed after it).
#[must_use]
pub fn found<'i>(
    wishes: &[&CharmWish],
    items: impl IntoIterator<Item = &'i ItemDrop>,
    class: Option<u32>,
) -> Vec<u32> {
    let mut have = vec![0; wishes.len()];
    for item in items {
        let slot = wishes
            .iter()
            .zip(have.iter_mut())
            .find(|(wish, n)| **n < wish.count && wish.matches(item, class));
        if let Some((_, n)) = slot {
            *n += 1;
        }
    }
    have
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use d2r_core::item_stats::{ItemStat, stat};

    /// An identified magic charm of `code` with `stats` as `(id, param, value)`.
    pub fn magic(code: &str, stats: &[(u16, u16, i32)]) -> ItemDrop {
        let size = match code {
            "cm1" => CharmSize::Small,
            "cm2" => CharmSize::Large,
            _ => CharmSize::Grand,
        };
        ItemDrop {
            cell: None,
            id: 0,
            code: Some(code.into()),
            kind: DropKind::Charm { size, unique: false },
            quality: item_data::QUALITY_MAGIC,
            flags: item_data::FLAG_IDENTIFIED,
            ethereal: false,
            sockets: 0,
            unique_index: None,
            stats: stats
                .iter()
                .map(|&(id, param, value)| ItemStat {
                    id,
                    param,
                    value,
                    base: false,
                })
                .collect(),
            item_data_head: String::new(),
            x: 0,
            y: 0,
        }
    }

    /// An identified unique charm of `code`, uniqueitems row `index`, with `stats`.
    pub fn unique(code: &str, index: u32, stats: &[(u16, u16, i32)]) -> ItemDrop {
        let mut item = magic(code, stats);
        item.quality = item_data::QUALITY_UNIQUE;
        item.unique_index = Some(index);
        if let DropKind::Charm { size, .. } = item.kind {
            item.kind = DropKind::Charm { size, unique: true };
        }
        item
    }

    type Parsed = Result<(Vec<CharmWish>, Vec<String>), toml::de::Error>;

    fn wishes(toml_text: &str) -> Parsed {
        #[derive(Deserialize)]
        struct File {
            charms: Vec<CharmWish>,
        }
        let mut warnings = Vec::new();
        let raw = toml::from_str::<File>(toml_text)?.charms;
        Ok((clean(raw, "build t", &mut warnings), warnings))
    }

    const DRUID: Option<u32> = Some(5);

    #[test]
    fn wishes_are_validated() -> Result<(), toml::de::Error> {
        let (ok, warnings) = wishes(
            r#"
            [[charms]]
            name = "Annihilus"
            unique = true
            level = 70
            [[charms]]
            name = "Hellfire Torch"
            unique = true
            stats = [{ stat = "class_skills", min = 3 }]
            [[charms]]
            name = "Shape Shifting skiller"
            size = "grand"
            count = 6
            stats = [{ stat = "skill_tab", tab = "Shape Shifting", min = 1 }]
            [[charms]]
            name = "No count"
            size = "small"
            count = 0
            stats = [{ stat = "life", min = 20 }]
            [[charms]]
            name = "Annihlus"
            unique = true
            [[charms]]
            name = "Sizeless"
            stats = [{ stat = "life", min = 20 }]
            [[charms]]
            name = "Statless"
            size = "small"
            [[charms]]
            name = "Bad tree"
            size = "grand"
            stats = [{ stat = "skill_tab", tab = "Shapeshifting", min = 1 }]
            [[charms]]
            name = "Tab on life"
            size = "small"
            stats = [{ stat = "life", tab = "Summoning", min = 1 }]
            [[charms]]
            name = "annihilus"
            unique = true
            "#,
        )?;
        let names: Vec<_> = ok.iter().map(|w| (w.name.as_str(), w.count)).collect();
        assert_eq!(
            names,
            [
                ("Annihilus", 1),
                ("Hellfire Torch", 1),
                ("Shape Shifting skiller", 6),
                ("No count", 1)
            ]
        );
        assert_eq!(warnings.len(), 7, "{warnings:#?}");
        for bad in [
            "Annihlus",
            "Sizeless",
            "Statless",
            "Bad tree",
            "Tab on life",
            "annihilus",
            "No count",
        ] {
            assert!(
                warnings.iter().any(|w| w.contains(&format!("{bad:?}"))),
                "{bad}: {warnings:#?}"
            );
        }
        assert_eq!(
            ok.get(2).map(CharmWish::describe),
            Some(vec!["+1 Shape Shifting skills".to_owned()])
        );
        Ok(())
    }

    #[test]
    fn a_charm_fills_the_first_wish_it_matches_up_to_its_count() -> Result<(), toml::de::Error> {
        let (list, warnings) = wishes(
            r#"
            [[charms]]
            name = "Hellfire Torch"
            unique = true
            stats = [{ stat = "class_skills", min = 3 }]
            [[charms]]
            name = "Shape Shifting skiller"
            size = "grand"
            count = 2
            stats = [{ stat = "skill_tab", tab = "Shape Shifting", min = 1 }]
            [[charms]]
            name = "Life small charm"
            size = "small"
            stats = [{ stat = "life", min = 20 }]
            [[charms]]
            name = "Resistance small charm"
            size = "small"
            stats = [{ stat = "all_res", min = 5 }]
            "#,
        )?;
        assert!(warnings.is_empty(), "{warnings:?}");
        let wanted: Vec<&CharmWish> = list.iter().collect();
        let res = [
            (stat::RES_FIRE, 0, 5),
            (stat::RES_COLD, 0, 5),
            (stat::RES_LIGHTNING, 0, 5),
            (stat::RES_POISON, 0, 5),
        ];
        let mut with_life = res.to_vec();
        with_life.push((stat::MAX_LIFE, 0, 20));
        let items = [
            // A sorceress torch (class param 1) is not the druid's.
            unique("cm2", 400, &[(stat::CLASS_SKILLS, 1, 3)]),
            magic("cm3", &[(stat::SKILL_TAB, 5 * 8 + 1, 1)]),
            magic("cm3", &[(stat::SKILL_TAB, 5 * 8 + 1, 1), (stat::MAX_LIFE, 0, 40)]),
            magic("cm3", &[(stat::SKILL_TAB, 5 * 8 + 1, 1)]),
            // A druid Summoning skiller is no Shape Shifting one.
            magic("cm3", &[(stat::SKILL_TAB, 5 * 8, 1)]),
            // Fills the life wish first, so the all res one stays open.
            magic("cm1", &with_life),
            // Large, not small.
            magic("cm2", &[(stat::MAX_LIFE, 0, 20)]),
        ];
        assert_eq!(found(&wanted, &items, DRUID), [0, 2, 1, 0]);
        let druid_torch = unique("cm2", 400, &[(stat::CLASS_SKILLS, 5, 3)]);
        assert_eq!(found(&wanted, [&druid_torch], DRUID), [1, 0, 0, 0]);
        let res_only = magic("cm1", &res);
        assert_eq!(found(&wanted, [&res_only], DRUID), [0, 0, 0, 1]);
        let unread = magic("cm3", &[]);
        assert_eq!(found(&wanted, [&unread], DRUID), [0, 0, 0, 0], "stats unread");
        Ok(())
    }

    #[test]
    fn latent_and_renewed_sunder_charms_count_as_their_unique() -> Result<(), toml::de::Error> {
        let (list, _) = wishes("[[charms]]\nname = \"Bone Break\"\nunique = true\n")?;
        let wish: Vec<&CharmWish> = list.iter().take(1).collect();
        assert_eq!(found(&wish, [&unique("cm3", 405, &[])], DRUID), [1]);
        assert_eq!(found(&wish, [&unique("cm3", 431, &[])], DRUID), [1], "Latent");
        assert_eq!(found(&wish, [&unique("cm3", 436, &[])], DRUID), [1], "Renewed");
        assert_eq!(found(&wish, [&unique("cm3", 359, &[])], DRUID), [0], "Gheed's");
        Ok(())
    }
}

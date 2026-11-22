//! Dashboard test fixtures, shared by the core's web tests and the playstyle crates' panel
//! tests (Cargo feature `testutil`).

use super::LiveInfo;

/// No skills read: the `skills` section is `null`.
pub static EMPTY_SKILLS: crate::game::Skills = crate::game::Skills {
    points: Vec::new(),
    left: None,
    right: None,
};

/// No marked ground drops: the `drops` section is empty.
pub static EMPTY_MARKS: std::sync::LazyLock<crate::guidance::Marks> =
    std::sync::LazyLock::new(crate::guidance::Marks::new);

/// A stash item for the panel tests (identified, not a runeword).
#[must_use]
pub fn stash_item(name: &str, code: &str, quality: u32, quantity: Option<u32>) -> crate::stash::StashItem {
    crate::stash::StashItem {
        name: name.to_owned(),
        code: Some(code.to_owned()),
        quality: u8::try_from(quality).unwrap_or_default(),
        identified: true,
        ethereal: name.starts_with("eth "),
        runeword: false,
        quantity,
        sockets: 0,
        unique_index: None,
        stats: Vec::new(),
    }
}

/// Page 1 with an identified unique and a plain ethereal base, an empty page 2, and the
/// materials tab with gems, runes and potions in memory order (one emptied stack).
#[must_use]
pub fn test_stash() -> Vec<crate::stash::StashTab> {
    use crate::offsets::item_data::{QUALITY_NORMAL, QUALITY_UNIQUE};
    use crate::stash::{StashTab, TabKind};
    let n = QUALITY_NORMAL;
    vec![
        StashTab {
            kind: TabKind::Page(1),
            items: vec![
                stash_item("Harlequin Crest", "uap", QUALITY_UNIQUE, None),
                stash_item("eth Crystal Sword 4os", "crs", n, None),
            ],
        },
        StashTab {
            kind: TabKind::Page(2),
            items: vec![],
        },
        StashTab {
            kind: TabKind::Materials,
            items: vec![
                stash_item("Rejuvenation Potion", "rvs", n, Some(50)),
                stash_item("Io Rune", "r16", n, Some(1)),
                stash_item("Perfect Topaz", "gpy", n, Some(2)),
                stash_item("El Rune", "r01", n, Some(8)),
                stash_item("Chipped Topaz", "gcy", n, Some(19)),
                stash_item("Flawless Amethyst", "gzv", n, Some(0)),
                stash_item("Chipped Amethyst", "gcv", n, Some(3)),
                stash_item("Full Rejuvenation Potion", "rvl", n, Some(43)),
            ],
        },
    ]
}

/// A `LiveInfo` over borrowed test data: difficulty 2 is hell.
#[must_use]
pub fn live_info<'a>(
    class: u32,
    blocks: &'a crate::render::TextBlocks,
    equipped: &'a [(u8, u32)],
    stash: &'a [crate::stash::StashTab],
    stash_owned: &'a [String],
) -> LiveInfo<'a> {
    LiveInfo {
        player_name: "Hork",
        class,
        char_level: Some(42),
        difficulty: Some(2),
        area: "Durance of Hate Level 3",
        blocks,
        equipped,
        equipped_items: &[],
        owned_items: &[],
        owned: &[],
        runeword_copies: &[],
        merc_worn: &[],
        stash,
        stash_owned,
        skills: &EMPTY_SKILLS,
        held_materials: &[],
        white_bases: &[],
        quests: &[],
        shop: None,
        strength: None,
        dexterity: None,
        marks: &EMPTY_MARKS,
        states: &[],
        updated_at: 1_700_000_000,
    }
}

/// The `{ "text", "colour" }` pairs of a block's lines in the live JSON.
pub fn lines_of(json: &serde_json::Value, key: &str) -> Vec<(String, String)> {
    json.get(key)
        .and_then(|v| v.as_array())
        .map_or_else(Vec::new, |lines| {
            lines
                .iter()
                .map(|l| {
                    (
                        l.get("text")
                            .and_then(|t| t.as_str())
                            .unwrap_or_default()
                            .to_owned(),
                        l.get("colour")
                            .and_then(|c| c.as_str())
                            .unwrap_or_default()
                            .to_owned(),
                    )
                })
                .collect()
        })
}

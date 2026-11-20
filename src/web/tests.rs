//! The core dashboard JSON's tests, and the fixtures the playstyle's panel tests share.

use super::*;
use anyhow::{Context, Result, anyhow};

use super::skills::{next_point, plan_mark, plan_target, skills_json};

/// The core panels alone (no playstyle sections, no plan, no vendor notes).
fn live_json(info: &LiveInfo) -> String {
    super::live_json(info, Panels::default())
}

/// No skills read: the `skills` section is `null`.
/// No skills read: the `skills` section is `null`.
pub static EMPTY_SKILLS: crate::game::Skills = crate::game::Skills {
    points: Vec::new(),
    left: None,
    right: None,
};

/// A stash item for the panel tests (identified, not a runeword).
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

#[test]
fn live_json_has_the_skill_trees() -> Result<()> {
    let blocks = crate::render::TextBlocks::default();
    // A level 20 Warlock (class 7): Summon Goatman 16, Blood Boil 1, Miasma Bolt 2; Attack
    // on the left button, Blood Boil on the right.
    let skills = crate::game::Skills {
        points: vec![(373, 16), (380, 1), (395, 2)],
        left: Some(0),
        right: Some(380),
    };
    let mut info = live_info(7, &blocks, &[], &[], &[]);
    info.char_level = Some(20);
    info.skills = &skills;
    let json = serde_json::from_str::<serde_json::Value>(&live_json(&info))?;
    let at = |path: &str| json.pointer(path).cloned().unwrap_or_default();
    assert_eq!(at("/skills/total"), 19);
    assert_eq!(at("/skills/left/name"), "Attack", "a basic skill has a name too");
    assert_eq!(at("/skills/right/name"), "Blood Boil");
    let names: Vec<_> = (0..3).map(|t| at(&format!("/skills/trees/{t}/name"))).collect();
    assert_eq!(names, ["Demon", "Eldritch", "Chaos"], "the game's tab order");
    let points: Vec<_> = (0..3).map(|t| at(&format!("/skills/trees/{t}/points"))).collect();
    assert_eq!(points, [17, 0, 2]);
    let demon = at("/skills/trees/0/skills");
    let demon = demon.as_array().context("demon skills")?;
    assert_eq!(demon.len(), 10, "every skill of the page, spent or not");
    // `(field, value)` pairs of the Demon tree's skill `id`.
    let fields = |id: u16, keys: &[&str]| -> Vec<serde_json::Value> {
        let skill = demon.iter().find(|k| k.get("id") == Some(&id.into()));
        keys.iter()
            .map(|k| skill.and_then(|s| s.get(k)).cloned().unwrap_or_default())
            .collect()
    };
    let keys = ["name", "row", "col", "req", "points", "left", "right", "locked"];
    assert_eq!(
        fields(380, &keys),
        serde_json::json!(["Blood Boil", 4, 2, 18, 1, false, true, false])
            .as_array()
            .context("expected")?
            .clone(),
    );
    assert_eq!(
        fields(376, &["points", "locked"]),
        [serde_json::json!(0), serde_json::json!(false)],
        "unspent, level 12 <= 20"
    );
    assert_eq!(
        fields(381, &["points", "locked"]),
        [serde_json::json!(0), serde_json::json!(true)],
        "Consume requires level 30, above 20"
    );
    Ok(())
}

#[test]
fn live_json_skills_are_null_when_unread() -> Result<()> {
    let blocks = crate::render::TextBlocks::default();
    let json = serde_json::from_str::<serde_json::Value>(&live_json(&live_info(7, &blocks, &[], &[], &[])))?;
    assert_eq!(json.get("skills"), Some(&serde_json::Value::Null));
    Ok(())
}

#[test]
fn live_json_has_the_stash_pages_and_materials() -> Result<()> {
    let blocks = crate::render::TextBlocks::default();
    let stash = test_stash();
    let json =
        serde_json::from_str::<serde_json::Value>(&live_json(&live_info(1, &blocks, &[], &stash, &[])))
            .map_err(|e| anyhow!("valid json: {e}"))?;
    // The normal pages in UI order with quality-tagged item lines (uniques gold); the
    // materials tab is not a page.
    assert_eq!(
        json.get("stash_pages"),
        Some(&serde_json::json!([
            {"page": 1, "items": [
                {"text": "Harlequin Crest (unique)", "colour": "#ffd278"},
                {"text": "eth Crystal Sword 4os", "colour": "#dddddd"},
            ]},
            {"page": 2, "items": []},
        ]))
    );
    // Gems by colour then grade, runes by number, the rest by name; the empty stack is out.
    assert_eq!(
        json.get("stash_materials"),
        Some(&serde_json::json!({
            "gems": [
                {"name": "Chipped Amethyst", "qty": 3, "code": "gcv"},
                {"name": "Chipped Topaz", "qty": 19, "code": "gcy"},
                {"name": "Perfect Topaz", "qty": 2, "code": "gpy"},
            ],
            "runes": [
                {"name": "El Rune", "qty": 8, "code": "r01"},
                {"name": "Io Rune", "qty": 1, "code": "r16"},
            ],
            "other": [
                {"name": "Full Rejuvenation Potion", "qty": 43, "code": "rvl"},
                {"name": "Rejuvenation Potion", "qty": 50, "code": "rvs"},
            ],
        }))
    );
    assert_eq!(json.get("stash"), None, "the flat line list is gone");
    Ok(())
}

#[test]
fn unknown_classes_slots_and_qualities_are_shown_not_dropped() -> Result<()> {
    let blocks = crate::render::TextBlocks::default();
    let json =
        serde_json::from_str::<serde_json::Value>(&live_json(&live_info(13, &blocks, &[(13, 42)], &[], &[])))
            .map_err(|e| anyhow!("valid json: {e}"))?;
    assert_eq!(
        json.pointer("/player/class").and_then(|v| v.as_str()),
        Some("class 13")
    );
    assert_eq!(
        json.pointer("/equipped/0/text").and_then(|v| v.as_str()),
        Some("slot 13: quality 42")
    );
    Ok(())
}

/// A skill-plan entry.
fn target(skill: &str, points: u8, by_level: u8) -> SkillTarget {
    SkillTarget {
        skill: skill.to_owned(),
        points,
        by_level,
        note: String::new(),
    }
}

#[test]
fn skill_plan_targets_marks_and_next_point() -> Result<()> {
    let plan = [
        target("Firestorm", 1, 2),
        target("Firestorm", 2, 3),
        target("Molten Boulder", 1, 6),
        target("Firestorm", 3, 8),
    ];
    assert_eq!(plan_target(&plan, "Firestorm", 1), 0, "nothing due yet");
    assert_eq!(plan_target(&plan, "firestorm", 7), 2, "the highest due target");
    assert_eq!(plan_target(&plan, "Fissure", 99), 0, "not in the plan");
    assert_eq!(plan_mark(&plan, "Firestorm", 7, 1), (2, Some("under")));
    assert_eq!(plan_mark(&plan, "Firestorm", 7, 2), (2, None));
    assert_eq!(
        plan_mark(&plan, "Firestorm", 7, 3),
        (2, Some("ahead")),
        "within the plan"
    );
    assert_eq!(
        plan_mark(&plan, "Firestorm", 7, 4),
        (2, Some("over")),
        "beyond the plan"
    );
    assert_eq!(plan_mark(&plan, "Fissure", 7, 1), (0, Some("over")));
    // The next point: the first due entry not reached, in plan order …
    let points = |fs: u8, mb: u8| {
        move |name: &str| {
            if name == "Firestorm" {
                fs
            } else if name == "Molten Boulder" {
                mb
            } else {
                0
            }
        }
    };
    let next = |level, fs, mb| next_point(&plan, level, points(fs, mb)).map(|t| (t.skill.as_str(), t.points));
    assert_eq!(next(7, 1, 0), Some(("Firestorm", 2)));
    assert_eq!(next(7, 2, 0), Some(("Molten Boulder", 1)));
    // … else the first upcoming one; nothing once the plan is done.
    assert_eq!(next(7, 2, 1), Some(("Firestorm", 3)));
    assert_eq!(next(9, 3, 1), None);
    // In the live JSON: the per-skill target and mark, and the hint (a level 7 druid with
    // Firestorm 1: under, next point Firestorm).
    let skills = crate::game::Skills {
        points: vec![(225, 1)],
        left: None,
        right: None,
    };
    let json = skills_json(5, Some(7), &skills, &plan);
    let firestorm = (0..3)
        .filter_map(|t| json.pointer(&format!("/trees/{t}/skills"))?.as_array())
        .flatten()
        .find(|k| k.get("name") == Some(&"Firestorm".into()))
        .cloned()
        .context("firestorm")?;
    assert_eq!(firestorm.get("target"), Some(&serde_json::json!(2)));
    assert_eq!(firestorm.get("plan"), Some(&serde_json::json!("under")));
    assert_eq!(
        firestorm.get("final"),
        Some(&serde_json::json!(3)),
        "the stage's last target"
    );
    assert_eq!(json.pointer("/next/skill"), Some(&serde_json::json!("Firestorm")));
    // Without a plan: no targets.
    let json = skills_json(5, Some(7), &skills, &[]);
    assert!(json.pointer("/trees/0/skills/0/target").is_none());
    assert_eq!(json.get("next"), Some(&serde_json::Value::Null));
    Ok(())
}

//! The skills panel: the class's three skill trees, marked against the playstyle's skill plan
//! ([`super::Panels::skill_plan`]).

use serde::Deserialize;

/// One skill plan entry (a build file's `[[skill_plan]]`, parsed by the playstyle): by character level `by_level` the guide has `points` hard
/// points in `skill` (CUMULATIVE, not the points added at that level).
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SkillTarget {
    /// The skill's displayed name, as in [`crate::skills_data::SKILLS`].
    pub skill: String,
    pub points: u8,
    pub by_level: u8,
    #[serde(default)]
    pub note: String,
}

/// The skill trees (`live_json`'s `skills` section): `{ "total", "left", "right", "trees":
/// [...] }`, `null` when the allocation is empty (unread) or the class unknown. `total` is
/// the hard points spent; `left` / `right` are the mouse-button skills as `{ "id", "name" }`
/// (or `null`). Each of the three trees (pages 1–3, the game's tab order) is `{ "name",
/// "points", "skills": [...] }` with every class skill on that page as `{ "id", "name",
/// "row", "col", "req", "points", "locked", "left", "right" }`: its grid place, required level,
/// hard points, whether the required level is above the character's, and whether it is on a
/// mouse button.
pub fn skills_json(
    class: u32,
    char_level: Option<u8>,
    skills: &crate::game::Skills,
    plan: &[SkillTarget],
) -> serde_json::Value {
    use crate::skills_data;
    let trees = usize::try_from(class)
        .ok()
        .and_then(|c| skills_data::TREES.get(c));
    let (Some(trees), false) = (trees, skills.is_empty()) else {
        return serde_json::Value::Null;
    };
    let points = |id: u16| {
        skills
            .points
            .iter()
            .find(|(i, _)| *i == id)
            .map_or(0, |(_, p)| *p)
    };
    let button = |id: Option<u16>| {
        id.map(|id| {
            let name = skills_data::name(id).map_or_else(|| format!("skill {id}"), str::to_owned);
            serde_json::json!({ "id": id, "name": name })
        })
    };
    let trees: Vec<serde_json::Value> = trees
        .iter()
        .zip(1u8..)
        .map(|(name, page)| {
            let on_page = skills_data::SKILLS
                .iter()
                .filter(|s| u32::from(s.class) == class && s.page == page);
            let spent: u32 = on_page.clone().map(|s| u32::from(points(s.id))).sum();
            let list: Vec<serde_json::Value> = on_page
                .map(|s| {
                    let mut json = serde_json::json!({
                        "id": s.id,
                        "name": s.name,
                        "row": s.row,
                        "col": s.col,
                        "req": s.req_level,
                        "points": points(s.id),
                        "locked": char_level.is_some_and(|l| s.req_level > l),
                        "left": skills.left == Some(s.id),
                        "right": skills.right == Some(s.id),
                    });
                    if let (false, Some(level), Some(obj)) =
                        (plan.is_empty(), char_level, json.as_object_mut())
                    {
                        let (target, mark) = plan_mark(plan, s.name, level, points(s.id));
                        obj.insert("target".to_owned(), serde_json::json!(target));
                        obj.insert(
                            "final".to_owned(),
                            serde_json::json!(plan_target(plan, s.name, u8::MAX)),
                        );
                        obj.insert("plan".to_owned(), serde_json::json!(mark));
                    }
                    json
                })
                .collect();
            serde_json::json!({ "name": name, "points": spent, "skills": list })
        })
        .collect();
    let total: u32 = skills.points.iter().map(|(_, p)| u32::from(*p)).sum();
    let by_name = |name: &str| {
        skills_data::SKILLS
            .iter()
            .find(|s| u32::from(s.class) == class && s.name.eq_ignore_ascii_case(name.trim()))
            .map_or(0, |s| points(s.id))
    };
    let next = char_level
        .and_then(|level| next_point(plan, level, by_name))
        .map(|t| serde_json::json!({ "skill": t.skill, "target": t.points, "by_level": t.by_level, "note": t.note }));
    serde_json::json!({
        "total": total,
        "left": button(skills.left),
        "right": button(skills.right),
        "trees": trees,
        "plan": !plan.is_empty(),
        "next": next,
    })
}

/// The skill plan's target for `skill` at character level `level`: the highest cumulative
/// target among its entries due by then (0 when none is).
pub fn plan_target(plan: &[SkillTarget], skill: &str, level: u8) -> u8 {
    plan.iter()
        .filter(|t| t.by_level <= level && t.skill.trim().eq_ignore_ascii_case(skill))
        .map(|t| t.points)
        .max()
        .unwrap_or(0)
}

/// A skill's target at `level` and how its hard `points` compare: `"under"` below the target,
/// `"over"` above anything the plan ever puts there (wasted points), `"ahead"` above the
/// current target but within the plan (an early quest point), `None` on target.
pub fn plan_mark(plan: &[SkillTarget], skill: &str, level: u8, points: u8) -> (u8, Option<&'static str>) {
    let target = plan_target(plan, skill, level);
    let most = plan_target(plan, skill, u8::MAX);
    let mark = if points < target {
        Some("under")
    } else if points > most {
        Some("over")
    } else if points > target {
        Some("ahead")
    } else {
        None
    };
    (target, mark)
}

/// Where the next skill point goes: the first plan entry, in order, that is due at `level`
/// and not reached yet (`points_of` gives a skill's hard points by name); when every due
/// entry is reached, the first upcoming entry not reached yet.
pub fn next_point(plan: &[SkillTarget], level: u8, points_of: impl Fn(&str) -> u8) -> Option<&SkillTarget> {
    let open = |t: &&SkillTarget| points_of(&t.skill) < t.points;
    plan.iter()
        .filter(|t| t.by_level <= level)
        .find(open)
        .or_else(|| plan.iter().filter(|t| t.by_level > level).find(open))
}

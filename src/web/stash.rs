//! The shared-stash panel: the normal pages and the gems / runes / materials tab.

/// The normal shared-stash pages as `[{ "page": n, "items": [{ "text", "colour" }] }]`, UI
/// order, 1-based page numbers ([`crate::stash::TabKind::Page`]); the materials tab is left
/// to [`stash_materials`].
pub fn stash_pages(tabs: &[crate::stash::StashTab]) -> Vec<serde_json::Value> {
    tabs.iter()
        .filter_map(|tab| match tab.kind {
            crate::stash::TabKind::Page(page) => Some(serde_json::json!({
                "page": page,
                "items": tab.items.iter().map(stash_item_line).collect::<Vec<_>>(),
            })),
            crate::stash::TabKind::Materials => None,
        })
        .collect()
}

/// One stash item with its quality tag as `{ "text", "colour" }`. Uniques are gold and sets
/// green (the overlay's drop colours); magic and rare get their classic colours, everything
/// else stays plain.
fn stash_item_line(item: &crate::stash::StashItem) -> serde_json::Value {
    use crate::offsets::item_data;
    let (tag, colour) = match u32::from(item.quality) {
        item_data::QUALITY_UNIQUE => ("unique", "#ffd278"),
        item_data::QUALITY_SET => ("set", "#5ae65a"),
        item_data::QUALITY_RARE => ("rare", "#ffff5f"),
        item_data::QUALITY_MAGIC => ("magic", "#8f88ff"),
        _ => ("", "#dddddd"),
    };
    let mut tag = tag.to_owned();
    // Base-name items already carry an "eth " prefix from the keep-list naming; only the
    // unique/set names need the tag.
    if item.ethereal
        && matches!(
            u32::from(item.quality),
            item_data::QUALITY_UNIQUE | item_data::QUALITY_SET
        )
    {
        tag.push_str(" eth");
    }
    if item.runeword {
        tag.push_str(" runeword");
    }
    let text = if tag.is_empty() {
        item.name.clone()
    } else {
        format!("{} ({tag})", item.name)
    };
    serde_json::json!({ "text": text, "colour": colour })
}

/// The gems/runes/materials tab's stacks as `{ "gems": [...], "runes": [...], "other": [...] }`,
/// each entry `{ "name", "qty", "code" }`: gems by colour (amethyst … skull) then grade
/// (chipped … perfect), runes by rune number, the rest by name. Emptied stacks (quantity 0)
/// are left out; a stack whose size was unreadable keeps `"qty": null`.
pub fn stash_materials(tabs: &[crate::stash::StashTab]) -> serde_json::Value {
    use crate::items::{self, DropKind};
    // A fn, not a closure: closures can't be generic over the three key types.
    fn values<K>(v: Vec<(K, serde_json::Value)>) -> Vec<serde_json::Value> {
        v.into_iter().map(|(_, e)| e).collect()
    }
    let mut gems = Vec::new();
    let mut runes = Vec::new();
    let mut other = Vec::new();
    let stacks = tabs
        .iter()
        .filter(|t| t.kind == crate::stash::TabKind::Materials)
        .flat_map(|t| &t.items)
        .filter(|i| i.quantity != Some(0));
    for item in stacks {
        let bytes = item.code.as_deref().and_then(items::code_bytes);
        let entry = serde_json::json!({ "name": item.name, "qty": item.quantity, "code": item.code });
        if let Some(gem) = bytes.and_then(items::gem) {
            gems.push((gem, entry));
        } else if let Some(DropKind::Rune(n)) = bytes.and_then(items::code_kind) {
            runes.push((n, entry));
        } else {
            other.push((item.name.clone(), entry));
        }
    }
    gems.sort_by_key(|(k, _)| *k);
    runes.sort_by_key(|(k, _)| *k);
    other.sort_by(|a, b| a.0.cmp(&b.0));
    serde_json::json!({ "gems": values(gems), "runes": values(runes), "other": values(other) })
}

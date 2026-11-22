//! The vendor panel: the open trade window, with the playstyle's notes.

use super::LiveInfo;

/// What the playstyle says about one item of the open trade window.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ShopNote {
    /// The build slot (`gloves`, `ring`, `merc helm`, …) whose piece the item is.
    pub slot: Option<String>,
    /// Why it beats what is worn in its body slot; `None`: not better.
    pub why: Option<String>,
    /// The mark it would get as a ground drop: a runeword base the builds want (keep) or a
    /// valuables match (trade).
    pub mark: Option<crate::guidance::Mark>,
}

/// The open trade window, `null` when none is open; `notes` are in window order. Identical
/// entries (potions, scrolls) are listed once with their `count`.
pub fn shop_json(info: &LiveInfo, notes: &[ShopNote]) -> serde_json::Value {
    let Some(shop) = info.shop else {
        return serde_json::Value::Null;
    };
    let lvl = info.char_level.map(u32::from);
    let no_note = ShopNote::default();
    let mut items: Vec<(serde_json::Value, u32)> = Vec::new();
    for (i, item) in shop.items.iter().enumerate() {
        let tab = match item.page {
            0 => "armor",
            1 | 2 => "weapons",
            _ => "misc",
        };
        let note = notes.get(i).unwrap_or(&no_note);
        let json = serde_json::json!({
            "name": item.name,
            "quality": item.quality,
            "tab": tab,
            "req": item.req_level,
            "high": item.req_level.zip(lvl).is_some_and(|(req, lvl)| req > lvl),
            "slot": note.slot,
            "better": note.why.is_some(),
            "why": note.why,
        });
        match items.iter_mut().find(|(j, _)| *j == json) {
            Some((_, count)) => *count += 1,
            None => items.push((json, 1)),
        }
    }
    let items: Vec<_> = items
        .into_iter()
        .map(|(mut json, count)| {
            if let Some(obj) = json.as_object_mut() {
                obj.insert("count".into(), count.into());
            }
            json
        })
        .collect();
    serde_json::json!({ "npc": shop.npc, "items": items })
}

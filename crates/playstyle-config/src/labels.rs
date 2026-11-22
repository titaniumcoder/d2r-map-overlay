//! The short map tag of a marked ground drop ([`d2r_core::guidance::Mark::label`]), at most
//! [`MAX`] characters (user request, 2026-11-22: "add labels but short. 5os for a base with 5
//! sockets. 3 to 4 letters"). First match wins:
//! 1. the `label` of the best certain valuables rule ([`valuables::rule_label`]): `Shak`, `TalS`;
//! 2. a fixed tag by item code: uber keys, organs, essences, token, Standard of Heroes;
//! 3. by kind: a rune's name ("Shae"), a charm's size ("GC") or unique name ("Anni"), a gem's
//!    grade and colour (`PAme`);
//! 4. a socketed base (not unique or set): "5os", ethereal "e4os";
//! 5. the first letters of the item's name: the unique or set name (also of an unidentified one
//!    whose base carries only that item, [`names::sole_name`]), else the base name.

use d2r_core::game::ItemDrop;
use d2r_core::items::{self, CharmSize, DropKind};
use d2r_core::names::{self, identity, shown};
use d2r_core::offsets::item_data;

use crate::valuables;

/// Most characters of a tag.
const MAX: usize = 4;

/// Tags by item code (the uber keys, organs, essences, token and the Standard of Heroes).
const BY_CODE: [(&str, &str); 12] = [
    ("bet", "Burn"),
    ("bey", "Eye"),
    ("ceh", "Chrg"),
    ("dhn", "Horn"),
    ("fed", "Fest"),
    ("mbr", "Brn"),
    ("pk1", "KoT"),
    ("pk2", "KoH"),
    ("pk3", "KoD"),
    ("std", "Std"),
    ("tes", "Twst"),
    ("toa", "Tok"),
];

/// The map tag of `drop` (see the module docs); empty only for an item with neither code nor
/// name.
#[must_use]
pub fn label(drop: &ItemDrop) -> String {
    let code = drop.code.as_deref().unwrap_or("");
    let unique_or_set = matches!(drop.quality, item_data::QUALITY_UNIQUE | item_data::QUALITY_SET);
    let name = identity(drop.quality, drop.flags, drop.unique_index)
        .or_else(|| names::sole_name(drop.quality, code));
    if let Some(tag) = valuables::rule_label(drop, name) {
        return short(tag);
    }
    if let Some(&(_, tag)) = BY_CODE.iter().find(|&&(c, _)| c == code) {
        return tag.to_owned();
    }
    match drop.kind {
        DropKind::Rune(_) => {
            if let Some(rune) = names::base_name(code).and_then(|n| n.strip_suffix(" Rune")) {
                return short(rune);
            }
        }
        DropKind::Charm { size, unique } => return charm(size, unique.then_some(name).flatten()),
        DropKind::Gem(colour) => {
            let grade = items::code_bytes(code).and_then(items::gem).map_or(2, |(_, g)| g);
            let prefix = match grade {
                4 => "P",
                3 => "F",
                _ => "",
            };
            return short(&format!("{prefix}{}", colour.name()));
        }
        DropKind::Base | DropKind::Set | DropKind::Unique => {}
    }
    if drop.sockets > 0 && !unique_or_set {
        let eth = if drop.ethereal { "e" } else { "" };
        return format!("{eth}{}os", drop.sockets);
    }
    let shown_name = name.map(shown).or_else(|| names::base_name(code)).unwrap_or(code);
    short(shown_name)
}

/// A charm's tag: a unique one's name ("Anni", "Torc", "Ghee", a sunder "Sund") when
/// `unique_name` knows it, else its size ("GC").
fn charm(size: CharmSize, unique_name: Option<&str>) -> String {
    let tag = match size {
        CharmSize::Small => "SC",
        CharmSize::Large => "LC",
        CharmSize::Grand => "GC",
    };
    match unique_name {
        Some("Annihilus") => "Anni".to_owned(),
        Some("Hellfire Torch") => "Torc".to_owned(),
        Some(n) if n.starts_with("Gheed") => "Ghee".to_owned(),
        Some(_) if size == CharmSize::Grand => "Sund".to_owned(),
        Some(n) => short(n),
        None => tag.to_owned(),
    }
}

/// The first [`MAX`] letters and digits of `name`, without a leading "The " ("The Stone of
/// Jordan" → `Ston`, "Tal Rasha's" → `TalR`).
fn short(name: &str) -> String {
    let name = name.strip_prefix("The ").unwrap_or(name);
    name.chars().filter(|c| c.is_alphanumeric()).take(MAX).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn drop(code: &str, kind: DropKind, quality: u32) -> ItemDrop {
        ItemDrop {
            id: 1,
            code: Some(code.into()),
            kind,
            quality,
            flags: 0,
            ethereal: false,
            sockets: 0,
            unique_index: None,
            stats: Vec::new(),
            item_data_head: String::new(),
            x: 0,
            y: 0,
        }
    }

    fn base(code: &str, sockets: u8, ethereal: bool) -> ItemDrop {
        ItemDrop {
            sockets,
            ethereal,
            ..drop(code, DropKind::Base, item_data::QUALITY_NORMAL)
        }
    }

    fn named(code: &str, kind: DropKind, quality: u32, index: u32) -> ItemDrop {
        ItemDrop {
            flags: item_data::FLAG_IDENTIFIED,
            unique_index: Some(index),
            ..drop(code, kind, quality)
        }
    }

    #[test]
    fn label_table() {
        let unique = item_data::QUALITY_UNIQUE;
        let magic = item_data::QUALITY_MAGIC;
        let normal = item_data::QUALITY_NORMAL;
        let charm = |size, unique| DropKind::Charm { size, unique };
        let cases = [
            (base("7wa", 5, false), "5os"),
            (base("7s8", 4, true), "e4os"),
            (base("uit", 4, false), "4os"),
            (drop("r13", DropKind::Rune(13), normal), "Shae"),
            (drop("r30", DropKind::Rune(30), normal), "Ber"),
            (drop("r33", DropKind::Rune(33), normal), "Zod"),
            (drop("pk1", DropKind::Base, normal), "KoT"),
            (drop("pk2", DropKind::Base, normal), "KoH"),
            (drop("pk3", DropKind::Base, normal), "KoD"),
            (drop("dhn", DropKind::Base, normal), "Horn"),
            (drop("bey", DropKind::Base, normal), "Eye"),
            (drop("mbr", DropKind::Base, normal), "Brn"),
            (drop("std", DropKind::Base, normal), "Std"),
            (drop("toa", DropKind::Base, normal), "Tok"),
            (drop("tes", DropKind::Base, normal), "Twst"),
            (drop("cm3", charm(CharmSize::Grand, false), magic), "GC"),
            (drop("cm2", charm(CharmSize::Large, false), magic), "LC"),
            (drop("cm1", charm(CharmSize::Small, false), magic), "SC"),
            // Unidentified: the only unique on its base.
            (drop("cm1", charm(CharmSize::Small, true), unique), "Anni"),
            (drop("cm2", charm(CharmSize::Large, true), unique), "Torc"),
            (named("cm3", charm(CharmSize::Grand, true), unique, 359), "Ghee"),
            (named("cm3", charm(CharmSize::Grand, true), unique, 401), "Sund"),
            (drop("cm3", charm(CharmSize::Grand, true), unique), "GC"),
            (
                drop("gpv", DropKind::Gem(items::GemColour::Amethyst), normal),
                "PAme",
            ),
            // Rule labels: Shako (unidentified, its only unique), Griffon's, Tal Rasha's set.
            (drop("uap", DropKind::Unique, unique), "Shak"),
            (drop("ci3", DropKind::Unique, unique), "Grif"),
            (named("rin", DropKind::Unique, unique, 122), "SoJ"),
            // An unidentified ring: any of many uniques, its base name only.
            (drop("rin", DropKind::Unique, unique), "Ring"),
        ];
        for (d, want) in cases {
            assert_eq!(label(&d), want, "{:?} {:?}", d.code, d.kind);
        }
    }

    #[test]
    fn short_skips_the_and_punctuation() {
        assert_eq!(short("The Stone of Jordan"), "Ston");
        assert_eq!(short("Tal Rasha's Lidless Eye"), "TalR");
        assert_eq!(short("Io"), "Io");
    }
}

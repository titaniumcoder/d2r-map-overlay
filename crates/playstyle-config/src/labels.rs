//! The short map tag of a marked ground drop ([`d2r_core::guidance::Mark::label`]), at most
//! [`MAX`] characters; gems, runes and materials get none (the loot filter already names them).

use d2r_core::game::ItemDrop;
use d2r_core::items::{CharmSize, DropKind};
use d2r_core::names::{self, identity, shown};
use d2r_core::offsets::item_data;

use crate::valuables;

/// Most characters of a tag.
const MAX: usize = 4;

/// The map tag of `drop`, first match wins: a valuables rule's label, a charm, a socketed base
/// ("5os", "e4os"), else the first letters of its name. Empty for a gem, rune or material.
#[must_use]
pub fn label(drop: &ItemDrop) -> String {
    let code = drop.code.as_deref().unwrap_or("");
    if matches!(drop.kind, DropKind::Gem(_) | DropKind::Rune(_)) || d2r_core::stash::tab_of(code).is_some() {
        return String::new();
    }
    let unique_or_set = matches!(drop.quality, item_data::QUALITY_UNIQUE | item_data::QUALITY_SET);
    let name = identity(drop.quality, drop.flags, drop.unique_index)
        .or_else(|| names::sole_name(drop.quality, code));
    if let Some(tag) = valuables::rule_label(drop, name) {
        return short(tag);
    }
    if let DropKind::Charm { size, unique } = drop.kind {
        return charm(size, unique.then_some(name).flatten());
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
            (drop("r13", DropKind::Rune(13), normal), ""),
            (drop("r30", DropKind::Rune(30), normal), ""),
            (drop("r33", DropKind::Rune(33), normal), ""),
            (drop("pk1", DropKind::Base, normal), ""),
            (drop("pk2", DropKind::Base, normal), ""),
            (drop("pk3", DropKind::Base, normal), ""),
            (drop("dhn", DropKind::Base, normal), ""),
            (drop("bey", DropKind::Base, normal), ""),
            (drop("mbr", DropKind::Base, normal), ""),
            (drop("std", DropKind::Base, normal), ""),
            (drop("toa", DropKind::Base, normal), ""),
            (drop("tes", DropKind::Base, normal), ""),
            (drop("cm3", charm(CharmSize::Grand, false), magic), "GC"),
            (drop("cm2", charm(CharmSize::Large, false), magic), "LC"),
            (drop("cm1", charm(CharmSize::Small, false), magic), "SC"),
            (drop("cm1", charm(CharmSize::Small, true), unique), "Anni"),
            (drop("cm2", charm(CharmSize::Large, true), unique), "Torc"),
            (named("cm3", charm(CharmSize::Grand, true), unique, 359), "Ghee"),
            (named("cm3", charm(CharmSize::Grand, true), unique, 401), "Sund"),
            (drop("cm3", charm(CharmSize::Grand, true), unique), "GC"),
            (
                drop("gpv", DropKind::Gem(d2r_core::items::GemColour::Amethyst), normal),
                "",
            ),
            (drop("uap", DropKind::Unique, unique), "Shak"),
            (drop("ci3", DropKind::Unique, unique), "Grif"),
            (named("rin", DropKind::Unique, unique, 122), "SoJ"),
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

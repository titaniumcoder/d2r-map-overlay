//! The breakpoint and gear guide (user request, 2026-11-17): from the character's class, build
//! and gear stats, text lines under the info line —
//! - the relevant speed breakpoints (FCR for casting builds; FHR for melee, since IAS frames
//!   depend on the weapon's base speed, which the reader does not know — IAS is only shown as a
//!   value) with what is missing for the next one, and
//! - what to look for on gear, always in the user's priority: the four resistances, then damage,
//!   then life, and
//! - the runewords that benefit the build at the current level or the next five, with where to
//!   farm the runes they need.
//!
//! Breakpoint tables are the classic D2 community tables (Amazon Basin); class ids are the
//! monstats txt rows. Runewords are a curated shopping list (base 1.10+), not a database.

use crate::build::Damage;
use crate::config::CLASS_NAMES;
use crate::game::{Player, SpeedStats};

/// FCR breakpoints per class: the total FCR at which the cast animation shortens (tables from
/// d2runewizard.com/breakpoints, D2R). The sorceress' Lightning spells have their own faster
/// table; this is the standard cast table. The Warlock casts like a necromancer/paladin.
const FCR: [&[u32]; 8] = [
    &[7, 14, 22, 32, 48, 68, 99, 152], // amazon
    &[9, 20, 37, 63, 105, 200],        // sorceress
    &[9, 18, 30, 48, 75, 125],         // necromancer
    &[9, 18, 30, 48, 75, 125],         // paladin
    &[9, 20, 37, 63, 105, 200],        // barbarian
    &[4, 10, 19, 30, 46, 68, 99, 163], // druid (human form)
    &[8, 16, 27, 42, 65, 102, 174],    // assassin
    &[9, 18, 30, 48, 75, 125],         // warlock
];

/// FHR breakpoints per class; weapon-independent, so correct without the weapon read.
const FHR: [&[u32]; 8] = [
    &[6, 13, 20, 32, 52, 86, 174, 600],        // amazon
    &[5, 9, 14, 20, 30, 42, 60, 86, 142, 280], // sorceress
    &[5, 10, 16, 26, 39, 56, 86, 152, 377],    // necromancer
    &[7, 15, 27, 48, 86, 200],                 // paladin
    &[7, 15, 27, 48, 86, 200],                 // barbarian
    &[5, 10, 16, 26, 39, 56, 86, 152, 377],    // druid (human form)
    &[7, 15, 27, 48, 86, 200],                 // assassin
    &[5, 10, 16, 26, 39, 56, 86, 152, 377],    // warlock
];

/// The rune names in order, El (1) to Zod (33).
const RUNES: [&str; 33] = [
    "El", "Eld", "Tir", "Nef", "Eth", "Ith", "Tal", "Ral", "Ort", "Thul", "Amn", "Sol", "Shael", "Dol",
    "Hel", "Io", "Lum", "Ko", "Fal", "Lem", "Pul", "Um", "Mal", "Ist", "Gul", "Vex", "Ohm", "Lo", "Sur",
    "Ber", "Jah", "Cham", "Zod",
];

/// Where a rune drops for a Normal-difficulty character: the Countess' special rune table caps
/// at Ral, the Normal Hellforge quest at Amn; from Sol on the Hellforge is a Nightmare quest.
fn rune_farm(rune: &str) -> &'static str {
    match rune {
        "Sol" | "Shael" | "Dol" | "Hel" | "Io" | "Lum" | "Ko" => "Hellforge quest (NM)",
        "Ort" | "Thul" | "Amn" => "Hellforge quest",
        _ => "Countess (Tower 5)",
    }
}

/// What a runeword is good for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Benefit {
    /// Speed/skills/mana: casting builds.
    Caster,
    /// Weapon damage: physical builds.
    Physical,
    /// Resistances: always on the list, they are the top gear priority.
    Resistance,
}

/// The useful low-level runewords (base 1.10+), with their packed runes and required character
/// level. Curated: the shopping list worth wearing on the way up.
const RUNEWORDS: [(&str, &str, u32, Benefit); 11] = [
    ("Steel", "TirEl", 13, Benefit::Physical),
    ("Stealth", "TalEth", 17, Benefit::Caster),
    ("Leaf", "TirRal", 19, Benefit::Caster),
    ("Ancient's Pledge", "RalOrtTal", 21, Benefit::Resistance),
    ("Zephyr", "OrtEth", 21, Benefit::Physical),
    ("Spirit", "TalThulOrtAmn", 25, Benefit::Caster),
    ("Strength", "AmnTir", 25, Benefit::Physical),
    ("Insight", "RalTirTalSol", 27, Benefit::Caster),
    ("Lore", "OrtSol", 27, Benefit::Caster),
    ("Honor", "AmnElIthTirSol", 27, Benefit::Physical),
    ("Smoke", "NefLum", 37, Benefit::Resistance),
];

/// True if the runeword benefits this build: resistance runewords always, the rest by damage
/// flavour.
const fn benefits(benefit: Benefit, build: &[Damage]) -> bool {
    match benefit {
        Benefit::Resistance => true,
        Benefit::Caster => is_caster(build),
        Benefit::Physical => !is_caster(build),
    }
}

/// True if the build's primary damage is a cast flavour (anything but physical): the FCR
/// breakpoints are what shortens its attacks.
const fn is_caster(build: &[Damage]) -> bool {
    !matches!(build.first(), Some(Damage::Physical))
}

/// The next entry of `table` strictly above `value`: `(threshold, missing)`, or `None` if the
/// table is exhausted (last breakpoint reached).
fn next_breakpoint(table: &[u32], value: u32) -> Option<(u32, u32)> {
    let &next = table.iter().find(|&&t| t > value)?;
    Some((next, next - value))
}

/// `current → next (+missing)` for the player's stat, or `at cap` past the last breakpoint.
fn stat_guide(label: &str, table: &[u32], value: u32) -> String {
    match next_breakpoint(table, value) {
        Some((next, missing)) => format!("{label} {value} → {next} (+{missing})"),
        None => format!("{label} {value} (at cap)"),
    }
}

/// The guide's first line: class, the relevant speed breakpoints with what is missing, and the
/// gear priority. `None` without readable speed stats or an unknown class.
#[must_use]
pub fn guide_line(player: &Player) -> Option<String> {
    let speed = player.speed?;
    let class = CLASS_NAMES.get(usize::try_from(player.class).ok()?)?;
    let fcr = FCR.get(usize::try_from(player.class).ok()?)?;
    let fhr = FHR.get(usize::try_from(player.class).ok()?)?;
    let gear = gear_priority(&speed, &player.build);
    if is_caster(&player.build) {
        Some(format!(
            "{class} · {} · {} · {gear}",
            stat_guide("FCR", fcr, speed.fcr),
            stat_guide("FHR", fhr, speed.fhr),
        ))
    } else {
        // IAS frames depend on the weapon's base speed: show the value, guide by FHR.
        Some(format!(
            "{class} · IAS {} · {} · {gear}",
            speed.ias,
            stat_guide("FHR", fhr, speed.fhr),
        ))
    }
}

/// The gear priority line with the player's current gear resistances: the four resistances
/// first, then the build's damage, then life.
fn gear_priority(speed: &SpeedStats, build: &[Damage]) -> String {
    let damage = build.first().map_or("dmg", |d| d.name());
    format!(
        "Gear: res f{} c{} l{} p{} > {damage} > life",
        speed.res_fire, speed.res_cold, speed.res_lightning, speed.res_poison
    )
}

/// Splits packed rune names (`TalThulOrtAmn`) by longest-prefix matching over [`RUNES`]; an
/// unknown substring stops the split and is kept as-is (the name is still displayed).
fn split_runes(runes: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut rest = runes;
    while !rest.is_empty() {
        let Some(name) = RUNES
            .iter()
            .filter(|n| rest.starts_with(**n))
            .max_by_key(|n| n.len())
        else {
            break;
        };
        out.push(*name);
        rest = &rest[name.len()..];
    }
    if out.is_empty() {
        out.push(runes);
    }
    out
}

/// The runeword suggestions for the player's level: entries required at the current level or
/// within the next five that benefit the build, as `Name (clvl): Rune Rune — farm: …`. At most
/// two.
#[must_use]
pub fn runeword_lines(player: &Player) -> Vec<String> {
    let Some(lvl) = player.char_level else {
        return Vec::new();
    };
    let window = u32::from(lvl)..=u32::from(lvl) + 5;
    let mut out = Vec::new();
    for &(name, runes, req, benefit) in RUNEWORDS.iter().filter(|&(_, _, req, _)| window.contains(req)) {
        if !benefits(benefit, &player.build) {
            continue;
        }
        let names = split_runes(runes);
        // The rarest (highest-order) rune of the word is the farming bottleneck — display order
        // is not rune order (Zephyr: Ort Eth, but Ort is the rarer one).
        let rarest = names
            .iter()
            .filter(|r| RUNES.contains(r))
            .max_by_key(|r| RUNES.iter().position(|n| n == *r));
        let farm = rarest.map_or("no rune table", |r| rune_farm(r));
        out.push(format!("{name} (clvl {req}): {} — farm: {farm}", names.join(" ")));
        if out.len() == 2 {
            break;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::{Addr, SpeedStats};

    fn player(class: u32, lvl: u8, fcr: u32, build: &[Damage]) -> Player {
        Player {
            addr: Addr(0),
            id: 1,
            name: "T".into(),
            class,
            x: 0,
            y: 0,
            level_id: 1,
            act: None,
            char_level: Some(lvl),
            speed: Some(SpeedStats {
                fcr,
                ias: 0,
                fhr: 0,
                res_fire: 0,
                res_cold: 0,
                res_lightning: 0,
                res_poison: 0,
            }),
            build: build.to_vec(),
        }
    }

    #[test]
    fn next_breakpoint_walks_the_table() {
        assert_eq!(next_breakpoint(&[9, 20, 37], 0), Some((9, 9)));
        assert_eq!(next_breakpoint(&[9, 20, 37], 20), Some((37, 17)));
        assert_eq!(next_breakpoint(&[9, 20, 37], 37), None, "at the cap");
        assert_eq!(next_breakpoint(&[9, 20, 37], 50), None);
    }

    #[test]
    fn guide_line_casters_get_fcr_melee_get_ias() {
        // Sorceress (class 1) with 20 FCR: next sorceress FCR breakpoint 37; FHR from 0 → 5.
        let p = player(1, 10, 20, &[Damage::Fire, Damage::Cold]);
        let line = guide_line(&p).unwrap_or_default();
        assert!(
            line.starts_with("sorceress · FCR 20 → 37 (+17) · FHR 0 → 5 (+5)"),
            "{line}"
        );
        assert!(line.ends_with("Gear: res f0 c0 l0 p0 > fire > life"), "{line}");
        // Barbarian (class 4), physical: IAS value, FHR guide (their table starts at 7).
        let p = player(4, 10, 0, &[Damage::Physical]);
        let line = guide_line(&p).unwrap_or_default();
        assert!(line.starts_with("barbarian · IAS 0 · FHR 0 → 7 (+7)"), "{line}");
        assert!(
            line.ends_with("Gear: res f0 c0 l0 p0 > physical > life"),
            "{line}"
        );
        // The Warlock (class 7) casts like a necromancer: FCR 20 → 30, FHR 0 → 5.
        let p = player(7, 10, 20, &[Damage::Poison]);
        let line = guide_line(&p).unwrap_or_default();
        assert!(
            line.starts_with("warlock · FCR 20 → 30 (+10) · FHR 0 → 5 (+5)"),
            "{line}"
        );
    }

    #[test]
    fn unknown_class_or_stats_give_no_line() {
        assert_eq!(guide_line(&player(8, 10, 0, &[])), None);
        let mut p = player(1, 10, 0, &[]);
        p.speed = None;
        assert_eq!(guide_line(&p), None);
    }

    #[test]
    fn runewords_match_the_level_window_and_build() {
        // Level 17 fire sorceress: Stealth (17) and Leaf (19) in the next-five window, Ancient's
        // Pledge (21) one too far.
        let p = player(1, 17, 0, &[Damage::Fire]);
        let lines = runeword_lines(&p);
        assert_eq!(lines.len(), 2, "{lines:?}");
        assert!(
            lines
                .first()
                .is_some_and(|l| l.starts_with("Stealth (clvl 17): Tal Eth")),
            "{lines:?}"
        );
        assert!(
            lines
                .get(1)
                .is_some_and(|l| l.starts_with("Leaf (clvl 19): Tir Ral")),
            "{lines:?}"
        );
        // A physical build at 13 gets Steel (Tir El, clvl 13); its rarest rune Tir (#3) is
        // Countess-farmable.
        let p = player(4, 13, 0, &[Damage::Physical]);
        assert_eq!(
            runeword_lines(&p),
            vec!["Steel (clvl 13): Tir El — farm: Countess (Tower 5)".to_owned()]
        );
        // At level 1 nothing is in the window yet (Steel needs 13).
        let p = player(4, 1, 0, &[Damage::Physical]);
        assert_eq!(runeword_lines(&p), Vec::<String>::new());
        // Resistance runewords are suggested for any build: Ancient's Pledge at 16-21.
        let p = player(4, 18, 0, &[Damage::Physical]);
        assert!(
            runeword_lines(&p)
                .iter()
                .any(|l| l.starts_with("Ancient's Pledge")),
        );
    }

    #[test]
    fn rune_names_split_without_spaces() {
        assert_eq!(split_runes("TalThulOrtAmn"), vec!["Tal", "Thul", "Ort", "Amn"]);
        // Longest prefix wins: Eld over El.
        assert_eq!(split_runes("EldOrt"), vec!["Eld", "Ort"]);
        assert_eq!(split_runes("NefLum"), vec!["Nef", "Lum"]);
        // Unknown text is kept whole for display.
        assert_eq!(split_runes("ZodEater"), vec!["Zod"]);
    }

    #[test]
    fn farm_hint_uses_rune_order_not_display_order() {
        // Zephyr (Ort Eth): Ort (#9) is the bottleneck even though Eth is displayed last.
        // Ancient's Pledge (Resistance) is in the window too and always suggested, first in
        // table order; both bottleneck at Ort, which is beyond the Countess' Normal special
        // table (caps at Ral) — Hellforge territory.
        let p = player(0, 18, 0, &[Damage::Physical]);
        assert_eq!(
            runeword_lines(&p),
            vec![
                "Ancient's Pledge (clvl 21): Ral Ort Tal — farm: Hellforge quest".to_owned(),
                "Zephyr (clvl 21): Ort Eth — farm: Hellforge quest".to_owned(),
            ]
        );
    }
}

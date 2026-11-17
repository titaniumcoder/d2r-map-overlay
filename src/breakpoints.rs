//! The breakpoint guide (user request, 2026-11-17): one line under the info line with the
//! character's speed stats and resistances —
//! - `fcr (frames / current / next)`: the current cast animation in frames, the total FCR, and
//!   the FCR needed for one frame less (the user's format: "6 fps, 22 current, 28 for 7 fps");
//! - `fhr (frames / current / next)` the same for hit recovery;
//! - `res f/c/l/p (difficulty penalty)`: the four resistances from gear with the difficulty
//!   penalty applied.
//!
//! Frames come from the D2 animation formulas (maxroll.gg/d2/resources/breakpoints-animations,
//! verified against the classic per-class breakpoint tables by hand): the effective rate is
//! `floor(stat × 120 / (stat + 120))` (FCR capped at 75), and the animation length is
//! `ceil(256 × base / floor(speed × (100 + EFCR) / 100) − 1)` for casting and the same with
//! `(50 + EFHR)` for hit recovery. The per-class bases below reproduce every classic breakpoint
//! of every class table exactly (the Warlock row assumes necromancer-like values — its
//! d2runewizard table matches; Druid values are for human form).

use crate::game::Player;

/// Cast animation data per class: (casting base, animation speed). The bases are one higher
/// than maxroll's table lists them (their own FCR tables and the formula agree on these).
const CAST: [(u32, u32); 8] = [
    (20, 256), // amazon
    (14, 256), // sorceress
    (16, 256), // necromancer
    (16, 256), // paladin
    (14, 256), // barbarian
    (15, 208), // druid (human form)
    (17, 256), // assassin
    (16, 256), // warlock (assumed necromancer-like)
];

/// Hit recovery data per class: (hit base, animation speed).
const HIT: [(u32, u32); 8] = [
    (6, 256), // amazon
    (8, 256), // sorceress
    (7, 256), // necromancer
    (5, 256), // paladin
    (5, 256), // barbarian
    (7, 256), // druid (human form)
    (5, 256), // assassin
    (7, 256), // warlock (assumed necromancer-like)
];

/// The resist difficulty penalty by difficulty id: none in Normal, −40 in Nightmare, −100 in
/// Hell.
const RES_PENALTY: [i32; 3] = [0, 40, 100];

/// Effective FCR: `floor(fcr × 120 / (fcr + 120))`, hard-capped at 75 (nothing above 200 FCR
/// does anything).
const fn efcr(fcr: u32) -> u32 {
    let e = fcr * 120 / (fcr + 120);
    if e > 75 { 75 } else { e }
}

/// Effective FHR: `floor(fhr × 120 / (fhr + 120))`; uncapped.
const fn efhr(fhr: u32) -> u32 {
    fhr * 120 / (fhr + 120)
}

/// The cast animation length in frames for `fcr` total.
const fn cast_frames(base: u32, speed: u32, fcr: u32) -> u32 {
    let d = speed * (100 + efcr(fcr)) / 100;
    (256 * base - 1) / d
}

/// The hit recovery animation length in frames for `fhr` total.
const fn hit_frames(base: u32, speed: u32, fhr: u32) -> u32 {
    let d = speed * (50 + efhr(fhr)) / 100;
    (256 * base - 1) / d
}

/// The least stat value that shortens the animation by one frame, searched up to a generous
/// bound; `None` when nothing within it helps any more.
fn next_threshold(
    current: u32,
    base: u32,
    speed: u32,
    f: fn(u32, u32, u32) -> u32,
    bound: u32,
) -> Option<u32> {
    let now = f(base, speed, current);
    (current + 1..=bound).find(|&t| f(base, speed, t) < now)
}

/// `(frames / current / next)` for one animation; without the third part past the last
/// breakpoint.
fn stat_part(
    label: &str,
    base: u32,
    speed: u32,
    current: u32,
    bound: u32,
    f: fn(u32, u32, u32) -> u32,
) -> String {
    let frames = f(base, speed, current);
    next_threshold(current, base, speed, f, bound).map_or_else(
        || format!("{label} ({frames} / {current})"),
        |next| format!("{label} ({frames} / {current} / {next})"),
    )
}

/// The guide band: `fcr (…) · fhr (…) · ias … · res f/c/l/p (penalty)`, each resistance below
/// 75 (after the difficulty penalty) coloured red (user request, 2026-11-17). `None` without
/// readable speed stats.
///
/// The IAS value is shown even though its frames depend on the weapon's base speed and the
/// attack skill used (user decision: what to show is not for the overlay to decide).
#[must_use]
pub fn guide_band(player: &Player, difficulty: Option<u8>) -> Option<crate::info::Band> {
    let speed = player.speed.as_ref()?;
    let (cast_base, cast_speed) = class_data(&CAST, player.class)?;
    let (hit_base, hit_speed) = class_data(&HIT, player.class)?;
    let wsm = player.wsm.map_or_else(String::new, |w| format!(" (wsm {w})"));
    let mut text = format!(
        "{} · {} · ias {}{wsm} · res ",
        stat_part("fcr", cast_base, cast_speed, speed.fcr, 200, cast_frames),
        stat_part("fhr", hit_base, hit_speed, speed.fhr, 480, hit_frames),
        speed.ias,
    );
    let mut spans = Vec::new();
    let penalty = difficulty
        .and_then(|d| RES_PENALTY.get(usize::from(d)).copied())
        .unwrap_or(0);
    let res = |v: u32| i32::try_from(v).unwrap_or(i32::MAX) - penalty;
    for (n, value) in [
        speed.res_fire,
        speed.res_cold,
        speed.res_lightning,
        speed.res_poison,
    ]
    .into_iter()
    .enumerate()
    {
        let v = res(value);
        if v < 75 {
            // The value's own bytes: ASCII, so the offset is a char boundary.
            spans.push((text.len(), crate::info::BAD_TEXT));
        }
        text.push_str(&v.to_string());
        if n < 3 {
            text.push('/');
        }
    }
    text.push_str(match difficulty {
        Some(1) => " (nm −40)",
        Some(2) => " (hell −100)",
        _ => "",
    });
    Some(crate::info::Band {
        text,
        colour: crate::info::GUIDE_TEXT,
        spans,
    })
}
/// The `(base, speed)` pair of `class`, or `None` for an unknown class id.
fn class_data(data: &[(u32, u32)], class: u32) -> Option<(u32, u32)> {
    data.get(usize::try_from(class).ok()?).copied()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::{Addr, SpeedStats};
    use anyhow::{Result, anyhow};

    fn player(class: u32, fcr: u32, fhr: u32, ias: u32) -> Player {
        Player {
            addr: Addr(0),
            id: 1,
            name: "T".into(),
            class,
            x: 0,
            y: 0,
            level_id: 1,
            act: None,
            char_level: Some(17),
            speed: Some(SpeedStats {
                fcr,
                ias,
                fhr,
                res_fire: 25,
                res_cold: 15,
                res_lightning: 25,
                res_poison: 5,
            }),
            build: Vec::new(),
            bis: None,
            wsm: None,
        }
    }

    #[test]
    fn cast_frames_match_the_classic_tables() {
        // Sorceress: 13 frames at 0, 12 at 9, 11 at 20, 10 at 37, 9 at 63, 8 at 105, 7 at 200.
        let (base, speed) = CAST[1];
        assert_eq!(cast_frames(base, speed, 0), 13);
        assert_eq!(cast_frames(base, speed, 9), 12);
        assert_eq!(cast_frames(base, speed, 20), 11);
        assert_eq!(cast_frames(base, speed, 37), 10);
        assert_eq!(cast_frames(base, speed, 63), 9);
        assert_eq!(cast_frames(base, speed, 105), 8);
        assert_eq!(cast_frames(base, speed, 200), 7);
        // Necromancer: 15 at 0, 14 at 9, 13 at 18, 12 at 30, 11 at 48, 10 at 75, 9 at 125.
        let (base, speed) = CAST[2];
        assert_eq!(cast_frames(base, speed, 0), 15);
        assert_eq!(cast_frames(base, speed, 9), 14);
        assert_eq!(cast_frames(base, speed, 18), 13);
        assert_eq!(cast_frames(base, speed, 30), 12);
        assert_eq!(cast_frames(base, speed, 48), 11);
        assert_eq!(cast_frames(base, speed, 75), 10);
        assert_eq!(cast_frames(base, speed, 125), 9);
        // Druid (human, speed 208): 18 at 0, 17 at 4, 16 at 10, 15 at 19, 14 at 30.
        let (base, speed) = CAST[5];
        assert_eq!(cast_frames(base, speed, 0), 18);
        assert_eq!(cast_frames(base, speed, 4), 17);
        assert_eq!(cast_frames(base, speed, 10), 16);
        assert_eq!(cast_frames(base, speed, 19), 15);
        assert_eq!(cast_frames(base, speed, 30), 14);
    }

    #[test]
    fn hit_frames_match_the_classic_tables() {
        // Sorceress: 9 frames at 50 FHR (maxroll's worked example).
        let (base, speed) = HIT[1];
        assert_eq!(hit_frames(base, speed, 50), 9);
        // Amazon: 11 at 0, 10 at 6, 9 at 13, 8 at 20, 7 at 32, 6 at 52, 5 at 86, 4 at 174.
        let (base, speed) = HIT[0];
        assert_eq!(hit_frames(base, speed, 0), 11);
        assert_eq!(hit_frames(base, speed, 6), 10);
        assert_eq!(hit_frames(base, speed, 13), 9);
        assert_eq!(hit_frames(base, speed, 20), 8);
        assert_eq!(hit_frames(base, speed, 32), 7);
        assert_eq!(hit_frames(base, speed, 52), 6);
        assert_eq!(hit_frames(base, speed, 86), 5);
        assert_eq!(hit_frames(base, speed, 174), 4);
        // Necromancer: 13 at 0, 12 at 5, 11 at 10, 10 at 16, 9 at 26, 8 at 39, 7 at 56, 6 at 86
        // (d2runewizard's FHR thresholds [5,10,16,26,39,56,86,152] each drop one frame).
        let (base, speed) = HIT[2];
        assert_eq!(hit_frames(base, speed, 0), 13);
        assert_eq!(hit_frames(base, speed, 5), 12);
        assert_eq!(hit_frames(base, speed, 10), 11);
        assert_eq!(hit_frames(base, speed, 16), 10);
        assert_eq!(hit_frames(base, speed, 26), 9);
        assert_eq!(hit_frames(base, speed, 39), 8);
        assert_eq!(hit_frames(base, speed, 56), 7);
        assert_eq!(hit_frames(base, speed, 86), 6);
    }

    #[test]
    fn next_threshold_finds_the_next_breakpoint() {
        let (base, speed) = CAST[1]; // sorceress
        // From 0 FCR the next cast breakpoint is 9 (13 → 12 frames).
        assert_eq!(next_threshold(0, base, speed, cast_frames, 200), Some(9));
        // From 20 the next is 37 (11 → 10 frames).
        assert_eq!(next_threshold(20, base, speed, cast_frames, 200), Some(37));
        // Past the cap (200) nothing helps any more.
        assert_eq!(next_threshold(200, base, speed, cast_frames, 200), None);
    }

    #[test]
    fn guide_band_has_the_users_format() -> Result<()> {
        // Warlock (class 7) with 20 FCR: 13 frames (base 15), next breakpoint 30.
        let p = player(7, 20, 0, 0);
        let Some(band) = guide_band(&p, Some(0)) else {
            return Err(anyhow!("no guide band"));
        };
        assert!(band.text.starts_with("fcr (13 / 20 / 30)"), "{}", band.text);
        // IAS is back in the line (user decision).
        assert!(band.text.contains("ias 0"), "{}", band.text);
        // Resists with no penalty in Normal.
        assert!(band.text.ends_with("res 25/15/25/5"), "{}", band.text);
        // The test values 25/15/25/5 are all below 75: all four values red already in Normal.
        assert_eq!(band.spans.len(), 4, "all four values red: {band:?}");
        // Nightmare applies −40; all four values drop below 75 and go red.
        let Some(band) = guide_band(&p, Some(1)) else {
            return Err(anyhow!("no guide band"));
        };
        assert!(
            band.text.ends_with("res -15/-25/-15/-35 (nm −40)"),
            "{}",
            band.text
        );
        assert_eq!(band.spans.len(), 4, "all four values red");
        for (off, colour) in &band.spans {
            assert_eq!(colour, &crate::info::BAD_TEXT);
            let rest = &band.text[*off..];
            assert!(rest.starts_with('-'), "a penalized value at {off}: {rest}");
        }
        Ok(())
    }

    #[test]
    fn unknown_class_or_stats_give_no_band() {
        assert_eq!(guide_band(&player(8, 0, 0, 0), None), None);
        let mut p = player(7, 0, 0, 0);
        p.speed = None;
        assert_eq!(guide_band(&p, None), None);
    }
}

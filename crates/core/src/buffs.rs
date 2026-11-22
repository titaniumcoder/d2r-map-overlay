//! The player's states: names for the journal, and the timed buffs the overlay counts down.
//!
//! The client keeps no expiry frame (PLAN, Memory layer), so a countdown starts when a state is
//! set or renewed (a new list of it attached) and runs for the game table's duration, or for a
//! warcry the duration it last ran.

use std::time::{Duration, Instant};

use crate::game::{Addr, StateList};
use crate::states_data::STATES;

/// Internal per-hit/cast/step states (`justhit`, `skilldelay`, `cooldown`, `skill_move`,
/// `just_portaled`, `skill_channeling`): not journaled, as they flicker every tick.
pub const HIDDEN: &[u32] = &[18, 86, 102, 121, 185, 216];

/// `states` without the [`HIDDEN`] ones.
#[must_use]
pub fn shown(states: &[u32]) -> Vec<u32> {
    states.iter().copied().filter(|id| !HIDDEN.contains(id)).collect()
}

#[must_use]
pub fn state_name(id: u32) -> Option<&'static str> {
    STATES.get(usize::try_from(id).ok()?).copied()
}

const FRAMES_PER_SECOND: u64 = 25;
const FERAL_RAGE: u32 = 120;
/// The stat of Feral Rage's list that counts its stacks: not yet recorded (PLAN, Memory layer).
const FERAL_RAGE_STACKS: Option<u16> = None;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Length {
    /// Game frames, from shrines.txt, misc.txt or skills.txt.
    Frames(u64),
    /// Depends on the caster's warcry levels: the length it last ran until cleared.
    Learned,
    /// No timer: Cyclone Armor lasts until its absorb is used up.
    Untimed,
}

/// A buff the overlay shows: its state, a tag of at most 4 characters and how long it lasts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timed {
    pub state: u32,
    pub tag: &'static str,
    pub length: Length,
}

const fn timed(state: u32, tag: &'static str, length: Length) -> Timed {
    Timed { state, tag, length }
}

/// In strip order.
pub const TIMED: &[Timed] = &[
    timed(32, "BO", Length::Learned),
    timed(51, "BC", Length::Learned),
    timed(26, "SH", Length::Learned),
    timed(FERAL_RAGE, "FR", Length::Frames(500)),
    timed(151, "CA", Length::Untimed),
    timed(137, "XP", Length::Frames(3600)),
    timed(128, "ARM", Length::Frames(2400)),
    timed(129, "CMB", Length::Frames(2400)),
    timed(131, "RFI", Length::Frames(3600)),
    timed(132, "RCO", Length::Frames(3600)),
    timed(130, "RLI", Length::Frames(3600)),
    timed(133, "RPO", Length::Frames(3600)),
    timed(134, "SKL", Length::Frames(2400)),
    timed(135, "MRC", Length::Frames(2400)),
    timed(136, "STA", Length::Frames(4800)),
    timed(179, "Thaw", Length::Frames(750)),
    timed(178, "Anti", Length::Frames(750)),
    timed(180, "Stam", Length::Frames(750)),
];

#[must_use]
pub fn timed_buff(state: u32) -> Option<&'static Timed> {
    TIMED.iter().find(|t| t.state == state)
}

/// One buff on the strip; `left` is `None` when its length is unknown (not learned yet, untimed,
/// or renewed unseen and past its end).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    pub tag: &'static str,
    pub stacks: Option<u32>,
    pub left: Option<Duration>,
}

/// Below this an entry is drawn as running out.
pub const RUNNING_OUT: Duration = Duration::from_secs(10);

impl Entry {
    /// `BO 1:42`, `FR ×3 12s`, `CA`.
    #[must_use]
    pub fn text(&self) -> String {
        let mut parts = vec![self.tag.to_owned()];
        if let Some(n) = self.stacks {
            parts.push(format!("×{n}"));
        }
        if let Some(left) = self.left {
            let secs = left.as_secs() + u64::from(left.subsec_nanos() > 0);
            parts.push(if secs < 60 {
                format!("{secs}s")
            } else {
                format!("{}:{:02}", secs / 60, secs % 60)
            });
        }
        parts.join(" ")
    }

    #[must_use]
    pub fn running_out(&self) -> bool {
        self.left.is_some_and(|left| left < RUNNING_OUT)
    }
}

#[derive(Debug, Clone, Copy)]
struct Active {
    state: u32,
    since: Instant,
    stacks: Option<u32>,
}

/// Follows the player's timed buffs from tick to tick.
#[derive(Debug, Default)]
pub struct Tracker {
    active: Vec<Active>,
    /// How long each [`Length::Learned`] buff last ran; kept across games.
    learned: Vec<(u32, Duration)>,
    newest_list: Option<Addr>,
}

impl Tracker {
    pub fn new_game(&mut self) {
        self.active.clear();
        self.newest_list = None;
    }

    pub fn observe(&mut self, now: Instant, states: &[u32], newest: Option<&StateList>) {
        self.end_cleared(now, states);
        self.start_set(now, states);
        self.renew(now, newest);
    }

    fn end_cleared(&mut self, now: Instant, states: &[u32]) {
        let (kept, ended): (Vec<Active>, Vec<Active>) =
            self.active.iter().partition(|a| states.contains(&a.state));
        self.active = kept;
        for a in ended {
            if timed_buff(a.state).is_some_and(|t| t.length == Length::Learned) {
                self.learn(a.state, now.saturating_duration_since(a.since));
            }
        }
    }

    fn learn(&mut self, state: u32, ran: Duration) {
        self.learned.retain(|&(s, _)| s != state);
        self.learned.push((state, ran));
    }

    fn start_set(&mut self, now: Instant, states: &[u32]) {
        for t in TIMED {
            if states.contains(&t.state) && !self.active.iter().any(|a| a.state == t.state) {
                self.active.push(Active {
                    state: t.state,
                    since: now,
                    stacks: None,
                });
            }
        }
    }

    /// A list newly attached for an active buff renews it.
    fn renew(&mut self, now: Instant, newest: Option<&StateList>) {
        let Some(list) = newest else { return };
        if self.newest_list.replace(list.list) == Some(list.list) {
            return;
        }
        if let Some(a) = self.active.iter_mut().find(|a| a.state == list.state) {
            a.since = now;
            a.stacks = stacks(list, FERAL_RAGE_STACKS);
        }
    }

    fn length(&self, t: &Timed) -> Option<Duration> {
        match t.length {
            Length::Frames(frames) => Some(Duration::from_millis(frames * 1000 / FRAMES_PER_SECOND)),
            Length::Learned => self.learned.iter().find(|(s, _)| *s == t.state).map(|&(_, d)| d),
            Length::Untimed => None,
        }
    }

    /// The active buffs in [`TIMED`] order.
    #[must_use]
    pub fn entries(&self, now: Instant) -> Vec<Entry> {
        TIMED
            .iter()
            .filter_map(|t| {
                let a = self.active.iter().find(|a| a.state == t.state)?;
                let ran = now.saturating_duration_since(a.since);
                let left = self
                    .length(t)
                    .and_then(|length| length.checked_sub(ran))
                    .filter(|left| !left.is_zero());
                Some(Entry {
                    tag: t.tag,
                    stacks: a.stacks,
                    left,
                })
            })
            .collect()
    }
}

/// Feral Rage's stack count: the value of `stat` in its list.
fn stacks(list: &StateList, stat: Option<u16>) -> Option<u32> {
    if list.state != FERAL_RAGE {
        return None;
    }
    let stat = stat?;
    list.stats.iter().find(|&&(s, _)| s == stat).map(|&(_, v)| v)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_lookup() {
        assert_eq!(state_name(0), Some("none"));
        assert_eq!(state_name(189), Some("desecrated"));
        assert_eq!(state_name(120), Some("feralrage"));
        assert_eq!(state_name(100_000), None);
    }

    #[test]
    fn hidden_states_are_dropped() {
        assert_eq!(shown(&[32, 86, 120, 18]), [32, 120]);
    }

    #[test]
    fn timed_states_and_tags() {
        let names: Vec<(&str, Option<&str>)> = TIMED.iter().map(|t| (t.tag, state_name(t.state))).collect();
        assert_eq!(
            names,
            [
                ("BO", Some("battleorders")),
                ("BC", Some("battlecommand")),
                ("SH", Some("shout")),
                ("FR", Some("feralrage")),
                ("CA", Some("cyclonearmor")),
                ("XP", Some("shrine_experience")),
                ("ARM", Some("shrine_armor")),
                ("CMB", Some("shrine_combat")),
                ("RFI", Some("shrine_resist_fire")),
                ("RCO", Some("shrine_resist_cold")),
                ("RLI", Some("shrine_resist_lightning")),
                ("RPO", Some("shrine_resist_poison")),
                ("SKL", Some("shrine_skill")),
                ("MRC", Some("shrine_mana_regen")),
                ("STA", Some("shrine_stamina")),
                ("Thaw", Some("thawing")),
                ("Anti", Some("antidote")),
                ("Stam", Some("staminapot")),
            ]
        );
        assert!(TIMED.iter().all(|t| t.tag.chars().count() <= 4));
        for not_timed in [2, 9, 11, 100, 106, 148, 149, 48] {
            assert_eq!(timed_buff(not_timed), None, "state {not_timed}");
        }
    }

    fn secs(s: u64) -> Duration {
        Duration::from_secs(s)
    }

    fn list(addr: u64, state: u32, entries: &[(u16, u32)]) -> StateList {
        StateList {
            list: Addr(addr),
            state,
            stats: entries.to_vec(),
        }
    }

    #[test]
    fn a_shrine_counts_down_its_table_length() {
        let t0 = Instant::now();
        let mut tr = Tracker::default();
        tr.observe(t0, &[48, 137, 148], None);
        let entry = |at| tr.entries(t0 + at);
        assert_eq!(
            entry(secs(0)),
            [Entry {
                tag: "XP",
                stacks: None,
                left: Some(secs(144)),
            }],
            "3600 frames at 25 fps; auras and Oak Sage-like states are not timed"
        );
        assert_eq!(entry(secs(100)).first().and_then(|e| e.left), Some(secs(44)));
        assert_eq!(
            entry(secs(150)).first().map(|e| e.left),
            Some(None),
            "past its end yet still set: no time"
        );
    }

    #[test]
    fn a_warcry_shows_no_time_until_it_ran_out_once() {
        let t0 = Instant::now();
        let mut tr = Tracker::default();
        tr.observe(t0, &[32], None);
        assert_eq!(tr.entries(t0 + secs(5)).first().map(|e| e.left), Some(None));
        tr.observe(t0 + secs(90), &[], None);
        assert_eq!(tr.entries(t0 + secs(90)), []);

        tr.new_game();
        let t1 = t0 + secs(200);
        tr.observe(t1, &[32], None);
        assert_eq!(
            tr.entries(t1 + secs(30)).first().and_then(|e| e.left),
            Some(secs(60)),
            "learned 90 s, kept across games"
        );
        tr.observe(t1 + secs(70), &[], None);
        tr.observe(t1 + secs(80), &[32], None);
        assert_eq!(
            tr.entries(t1 + secs(80)).first().and_then(|e| e.left),
            Some(secs(70)),
            "re-learned from the latest run"
        );
    }

    #[test]
    fn a_new_list_renews_an_active_buff() {
        let t0 = Instant::now();
        let mut tr = Tracker::default();
        let fr = list(0x100, FERAL_RAGE, &[(67, 20)]);
        tr.observe(t0, &[FERAL_RAGE], Some(&fr));
        tr.observe(t0 + secs(15), &[FERAL_RAGE], Some(&fr));
        assert_eq!(
            tr.entries(t0 + secs(15)).first().and_then(|e| e.left),
            Some(secs(5)),
            "the same list renews nothing"
        );
        let recast = list(0x200, FERAL_RAGE, &[(67, 24)]);
        tr.observe(t0 + secs(15), &[FERAL_RAGE], Some(&recast));
        assert_eq!(
            tr.entries(t0 + secs(15)).first().and_then(|e| e.left),
            Some(secs(20))
        );
        let run_toggle_then_other = list(0x300, 137, &[]);
        tr.observe(t0 + secs(16), &[FERAL_RAGE], Some(&run_toggle_then_other));
        assert_eq!(
            tr.entries(t0 + secs(16)).first().and_then(|e| e.left),
            Some(secs(19)),
            "another state's list leaves Feral Rage alone"
        );
    }

    #[test]
    fn a_warcry_renewed_before_it_ends_learns_from_the_renewal() {
        let t0 = Instant::now();
        let mut tr = Tracker::default();
        tr.observe(t0, &[32], Some(&list(0x100, 32, &[])));
        tr.observe(t0 + secs(50), &[32], Some(&list(0x200, 32, &[])));
        tr.observe(t0 + secs(130), &[], None);
        tr.observe(t0 + secs(200), &[32], None);
        assert_eq!(
            tr.entries(t0 + secs(200)).first().and_then(|e| e.left),
            Some(secs(80))
        );
    }

    #[test]
    fn feral_rage_stacks_need_their_stat() {
        let fr = list(0x100, FERAL_RAGE, &[(67, 20), (500, 3)]);
        assert_eq!(stacks(&fr, FERAL_RAGE_STACKS), None, "stat not yet known");
        assert_eq!(stacks(&fr, Some(500)), Some(3));
        assert_eq!(stacks(&fr, Some(501)), None);
        assert_eq!(
            stacks(&list(0x100, 32, &[(500, 3)]), Some(500)),
            None,
            "not Feral Rage"
        );
    }

    #[test]
    fn untimed_and_text() {
        let t0 = Instant::now();
        let mut tr = Tracker::default();
        tr.observe(t0, &[151, 179], None);
        let texts: Vec<String> = tr.entries(t0 + secs(21)).iter().map(Entry::text).collect();
        assert_eq!(texts, ["CA", "Thaw 9s"]);
        let entry = |stacks, left| Entry {
            tag: "FR",
            stacks,
            left,
        };
        assert_eq!(entry(Some(3), Some(secs(12))).text(), "FR ×3 12s");
        assert_eq!(entry(None, Some(secs(102))).text(), "FR 1:42");
        assert_eq!(
            entry(None, Some(Duration::from_millis(59_200))).text(),
            "FR 1:00",
            "rounded up"
        );
        assert_eq!(entry(None, Some(secs(60))).text(), "FR 1:00");
        assert!(entry(None, Some(Duration::from_millis(9_900))).running_out());
        assert!(!entry(None, Some(secs(10))).running_out());
        assert!(!entry(None, None).running_out());
    }
}

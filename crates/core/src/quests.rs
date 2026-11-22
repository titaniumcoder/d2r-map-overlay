//! The player's quest flags ([`crate::offsets::quest_flags`]): which quest a build recipe's
//! `quest` key names, and whether that quest's reward is still to take.

use anyhow::{Context, Result, ensure};

use super::offsets::quest_flags;
use crate::reader::ReadMem;

/// Quest names and their word in the record (classic d2s order; words 0, 7, 8, 15, 16, 23, 24
/// and 28 are act intros/completions, 29–34 unused, 41 Akara's respec).
pub const QUESTS: [(&str, usize); 28] = [
    ("Den of Evil", 1),
    ("Sisters' Burial Grounds", 2),
    ("Tools of the Trade", 3),
    ("The Search for Cain", 4),
    ("The Forgotten Tower", 5),
    ("Sisters to the Slaughter", 6),
    ("Radament's Lair", 9),
    ("The Horadric Staff", 10),
    ("Tainted Sun", 11),
    ("Arcane Sanctuary", 12),
    ("The Summoner", 13),
    ("The Seven Tombs", 14),
    ("Lam Esen's Tome", 17),
    ("Khalim's Will", 18),
    ("Blade of the Old Religion", 19),
    ("The Golden Bird", 20),
    ("The Blackened Temple", 21),
    ("The Guardian", 22),
    ("The Fallen Angel", 25),
    ("Terror's End", 26),
    ("Hell's Forge", 27),
    ("Siege on Harrogath", 35),
    ("Rescue on Mount Arreat", 36),
    ("Prison of Ice", 37),
    ("Betrayal of Harrogath", 38),
    ("Rite of Passage", 39),
    ("Eve of Destruction", 40),
    ("Akara respec", 41),
];

/// Number of u16 words in the record.
const WORDS: usize = 48;

/// The word of the quest named `name` (case and apostrophes ignored).
#[must_use]
pub fn word_of(name: &str) -> Option<usize> {
    QUESTS
        .iter()
        .find(|(n, _)| crate::names::same_name(n, name))
        .map(|&(_, w)| w)
}

/// Where a quest's reward stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reward {
    /// The quest isn't finished: the reward is still to earn.
    NotEarned,
    /// Finished, reward not yet taken.
    Ready,
    /// Taken (or the quest finished without a pending reward).
    Used,
}

/// The reward state of quest word `word` in `words`; `None` when the record is unread
/// (empty) or too short.
#[must_use]
pub fn reward(words: &[u16], word: usize) -> Option<Reward> {
    let w = *words.get(word)?;
    Some(if w & quest_flags::REWARD_PENDING != 0 {
        Reward::Ready
    } else if w & quest_flags::REWARD_GRANTED != 0 {
        Reward::Used
    } else {
        Reward::NotEarned
    })
}

/// The quests `words` shows as done (reward taken or ready to take), in [`QUESTS`] order;
/// Akara's respec is no quest and left out. Empty when the record is unread.
pub fn done(words: &[u16]) -> impl Iterator<Item = &'static str> + '_ {
    QUESTS
        .iter()
        .filter(|&&(name, _)| name != "Akara respec")
        .filter(|&&(_, word)| matches!(reward(words, word), Some(Reward::Ready | Reward::Used)))
        .map(|&(name, _)| name)
}

/// Reads the record through the `.data` global at `global` (live address).
///
/// # Errors
/// Fails on an unreadable pointer chain or a header of another shape (no game yet).
pub fn read(mem: &impl ReadMem, global: u64) -> Result<Vec<u16>> {
    let header = mem.read_ptr(global)?.context("quest flags pointer is null")?;
    let bits = mem.read_u32(header + quest_flags::BITS)?;
    let bytes = mem.read_u64(header + quest_flags::BYTES)?;
    ensure!(
        bits == quest_flags::BITS_VALUE && bytes == quest_flags::BYTES_VALUE,
        "quest flags header {header:#x}: {bits:#x} bits, {bytes:#x} bytes"
    );
    let buffer = mem
        .read_ptr(header + quest_flags::BUFFER)?
        .context("quest flags buffer is null")?;
    let raw = mem.read_bytes(buffer, WORDS * 2)?;
    Ok(raw
        .as_chunks::<2>()
        .0
        .iter()
        .map(|&c| u16::from_le_bytes(c))
        .collect())
}

/// True if `header` (a [`quest_flags::HEADER_LEN`]-byte read) has the record header's shape.
pub fn is_header(header: &[u8]) -> bool {
    let u32_at = |o: u64| {
        usize::try_from(o)
            .ok()
            .and_then(|o| header.get(o..o + 4)?.try_into().ok())
            .map(u32::from_le_bytes)
    };
    let u64_at = |o: u64| {
        usize::try_from(o)
            .ok()
            .and_then(|o| header.get(o..o + 8)?.try_into().ok())
            .map(u64::from_le_bytes)
    };
    u32_at(quest_flags::BITS) == Some(quest_flags::BITS_VALUE)
        && u64_at(quest_flags::BYTES) == Some(quest_flags::BYTES_VALUE)
        && u64_at(quest_flags::BUFFER).is_some_and(crate::reader::is_user_ptr)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::FakeMem;

    /// A live Normal record: Acts 1–2 done, Act 3 entered, Blood Raven skipped, respec unused.
    const ELEMENTAL: [u16; 42] = [
        0x0001, 0x9001, 0x000c, 0x1049, 0x1019, 0x1055, 0x1019, 0x0001, 0x0001, 0x1011, 0x3c79, 0x1005,
        0x1181, 0x1005, 0x3e25, 0x2001, 0x0001, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        0, 0, 0, 0, 0x8002,
    ];

    #[test]
    fn names_resolve_loosely() {
        assert_eq!(word_of("Tools of the Trade"), Some(3));
        assert_eq!(word_of("tools of the trade"), Some(3));
        assert_eq!(word_of("Siege on Harrogath"), Some(35));
        assert_eq!(word_of("Radaments Lair"), Some(9), "apostrophe ignored");
        assert_eq!(word_of("Charsi"), None);
    }

    #[test]
    fn rewards_from_the_live_record() {
        let tools = word_of("Tools of the Trade").unwrap_or_default();
        assert_eq!(reward(&ELEMENTAL, tools), Some(Reward::Used), "imbue used");
        assert_eq!(
            reward(&ELEMENTAL, 2),
            Some(Reward::NotEarned),
            "Blood Raven skipped"
        );
        assert_eq!(
            reward(&ELEMENTAL, 35),
            Some(Reward::NotEarned),
            "Act 5 not reached"
        );
        assert_eq!(reward(&ELEMENTAL, 41), Some(Reward::Ready), "respec not used");
        assert_eq!(reward(&[], 3), None, "unread");
    }

    #[test]
    fn done_lists_finished_quests_only() {
        let done: Vec<&str> = done(&ELEMENTAL).collect();
        assert!(done.contains(&"Den of Evil"));
        assert!(done.contains(&"The Seven Tombs"));
        assert!(!done.contains(&"Sisters' Burial Grounds"), "skipped");
        assert!(!done.contains(&"Akara respec"), "no quest");
        assert!(!done.contains(&"The Guardian"), "Act 3 not done");
        assert_eq!(super::done(&[]).count(), 0, "unread");
    }

    #[test]
    fn reads_through_the_global() -> Result<()> {
        const GLOBAL: u64 = 0x10_0000;
        const HEADER: u64 = 0x20_0000;
        let mut mem = FakeMem::default();
        mem.add(GLOBAL, HEADER.to_le_bytes().to_vec());
        let mut header = vec![0u8; quest_flags::HEADER_LEN];
        header.splice(0..8, (HEADER + 0x30).to_le_bytes());
        header.splice(8..12, quest_flags::BITS_VALUE.to_le_bytes());
        header.splice(0x28..0x30, quest_flags::BYTES_VALUE.to_le_bytes());
        assert!(is_header(&header));
        let mut words: Vec<u8> = ELEMENTAL.iter().flat_map(|w| w.to_le_bytes()).collect();
        words.resize(WORDS * 2, 0);
        header.extend(words);
        mem.add(HEADER, header.clone());
        let read_back = read(&mem, GLOBAL)?;
        assert_eq!(read_back.len(), WORDS);
        assert_eq!(read_back.get(..ELEMENTAL.len()), Some(&ELEMENTAL[..]));
        header.splice(8..12, 0x200u32.to_le_bytes());
        assert!(!is_header(&header));
        mem.add(HEADER, header);
        assert!(
            read(&mem, GLOBAL).is_err(),
            "another shape (no game yet) is an error, not garbage"
        );
        Ok(())
    }
}

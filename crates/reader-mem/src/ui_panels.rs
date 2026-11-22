//! The game's open-panel flags ([`ui_panels`]): whether the stash window is open. Read only on
//! the build the array was found on, and only while the known flags read 0 or 1.

use d2r_core::offsets::ui_panels;
use d2r_core::reader::ReadMem;

/// Where to read the flags: the array's address in the image alias, on the recorded build.
#[must_use]
pub fn address(build: &str, alias_base: u64) -> Option<u64> {
    (build == ui_panels::BUILD).then_some(alias_base + ui_panels::RVA)
}

/// Whether the stash window is open; `None` when the array can't be read or a known flag
/// isn't 0 or 1 (not the array).
pub fn stash_open(mem: &impl ReadMem, addr: u64) -> Option<bool> {
    let flags = mem.read_bytes(addr, ui_panels::FLAGS).ok()?;
    decode_stash(&flags)
}

fn decode_stash(flags: &[u8]) -> Option<bool> {
    let known = [
        ui_panels::INVENTORY,
        ui_panels::NPC_INTERACT,
        ui_panels::NPC_SHOP,
        ui_panels::STASH,
        ui_panels::CUBE,
    ];
    let mut values = known.iter().map(|&at| flags.get(at).copied());
    if values.any(|b| b.is_none_or(|b| b > 1)) {
        return None;
    }
    flags.get(ui_panels::STASH).map(|&b| b == 1)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The flag bytes with `on` set; the rest of the array was 0 throughout both recordings.
    fn flags(on: &[usize]) -> Vec<u8> {
        (0..ui_panels::FLAGS).map(|i| u8::from(on.contains(&i))).collect()
    }

    #[test]
    fn the_stash_flag_from_recording_2() {
        // 2026-11-22 recording 2: t 4628 the stash opened (inventory and stash 0 → 1), t 4648
        // Esc (both 1 → 0), t 4672 the trade window (inventory, NPC, trade).
        assert_eq!(decode_stash(&flags(&[0x01, 0x18])), Some(true));
        assert_eq!(decode_stash(&flags(&[])), Some(false));
        assert_eq!(decode_stash(&flags(&[0x01, 0x08, 0x0B])), Some(false));
    }

    #[test]
    fn not_flags_is_no_answer() {
        let other: Vec<u8> = flags(&[0x18])
            .into_iter()
            .enumerate()
            .map(|(i, b)| if i == ui_panels::INVENTORY { 7 } else { b })
            .collect();
        assert_eq!(
            decode_stash(&other),
            None,
            "the inventory flag reads 7: not the array"
        );
        assert_eq!(decode_stash(&[0, 1]), None, "a short read");
    }

    #[test]
    fn only_on_the_recorded_build() {
        assert_eq!(
            address("6ad81202-279c000", 0x7000_0000_0000),
            Some(0x7000_01ec_e2d8)
        );
        assert_eq!(address("6b000000-279c000", 0x7000_0000_0000), None);
    }
}

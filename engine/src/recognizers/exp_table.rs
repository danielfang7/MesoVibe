//! EXP needed for each level of MapleStory Classic World: the EXP that fills the bar from 0% to
//! 100% at that level.
//!
//! From meowdb's EXP table, read 2026-11-02:
//! <https://meowdb.com/msclassic/guides/exp-table-level-1-to-100>. meowdb says levels 1 to 49
//! match current play and later rows are its planning numbers. Level 41's 324,240 matches the
//! game's own EXP tooltip in recorded captures, and level 42's 356,916 is the only value the HUD's
//! percentages allow over 33 minutes of a recorded run at that level.
//!
//! The EXP bar reader uses the table as a head start and confirms it from the HUD, so a wrong row
//! costs speed, not correctness (see `exp_bar`). These are numbers the game shows, not game files
//! or pixels.

/// EXP needed at level `n`, at index `n - 1`.
pub const NEEDED: [u64; 99] = [
    15, 34, 57, 92, 135, 372, 560, 840, 1242, 1716, 2360, 3216, 4200, 5460, 7050, 8840, 11040,
    13716, 16680, 20216, 24402, 28980, 34320, 40512, 47216, 54900, 63666, 73080, 83720, 95700,
    108480, 122760, 138666, 155540, 174216, 194832, 216600, 240500, 266682, 294216, 324240, 356916,
    391160, 428280, 468450, 510420, 555680, 604416, 655200, 709716, 748608, 789631, 832902, 878545,
    926689, 977471, 1031036, 1087536, 1147132, 1209994, 1276301, 1346242, 1420016, 1497832,
    1579913, 1666492, 1757815, 1854143, 1955750, 2062925, 2175973, 2295216, 2420993, 2553663,
    2693603, 2841212, 2996910, 3161140, 3334370, 3517093, 3709829, 3913127, 4127566, 4353756,
    4592341, 4844001, 5109452, 5389449, 5684790, 11938059, 25069923, 52646838, 110558359,
    232172553, 487562361, 1023880958, 2150150011, 4515315023, 9482161548,
];

/// The highest level meowdb lists as matching current play.
pub const CHECKED_THROUGH: u16 = 49;

/// EXP needed at `level`, if the table has it.
pub fn needed(level: u16) -> Option<u64> {
    NEEDED.get(usize::from(level.checked_sub(1)?)).copied()
}

/// The levels whose EXP needed lies in `lo..=hi`, lowest first.
pub fn levels_within(lo: u64, hi: u64) -> impl Iterator<Item = u16> {
    let first = NEEDED.partition_point(|&n| n < lo);
    NEEDED[first..]
        .iter()
        .take_while(move |&&n| n <= hi)
        .enumerate()
        .map(move |(i, _)| (first + i + 1) as u16)
}

/// Whether any level's EXP needed lies in `lo..=hi`.
pub fn any_within(lo: u64, hi: u64) -> bool {
    levels_within(lo, hi).next().is_some()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_level_needs_more_than_the_last() {
        assert!(NEEDED.windows(2).all(|w| w[0] < w[1]));
    }

    #[test]
    fn level_41_matches_the_games_own_tooltip() {
        // "EXP: 128,555 / 324,240" over the EXP bar at level 41 (recorded captures, 2026-11-02).
        assert_eq!(needed(41), Some(324_240));
        // The only EXP needed that fits every reading at level 42 in a recorded run (2026-11-02).
        assert_eq!(needed(42), Some(356_916));
        assert_eq!(needed(1), Some(15));
        assert_eq!(needed(0), None);
        assert_eq!(needed(100), None);
    }

    #[test]
    fn finds_the_levels_inside_a_range() {
        assert_eq!(
            levels_within(324_236, 324_252).collect::<Vec<_>>(),
            vec![41]
        );
        assert_eq!(
            levels_within(300_000, 360_000).collect::<Vec<_>>(),
            vec![41, 42]
        );
        assert_eq!(levels_within(1, 14).count(), 0);
        assert_eq!(
            levels_within(9_482_161_548, u64::MAX).collect::<Vec<_>>(),
            vec![99]
        );
        assert!(any_within(15, 15));
        assert!(!any_within(16, 33));
    }
}

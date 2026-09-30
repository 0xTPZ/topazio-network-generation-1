use crate::{block::Block, crypto::Hash};

pub const INITIAL_DIFFICULTY_BITS: u16 = 12;

pub const TARGET_BLOCK_TIME_SECS: u64 = 30;

pub const DIFFICULTY_ADJUSTMENT_INTERVAL: u64 = 120;

pub const MAX_FUTURE_DRIFT_SECS: u64 = 2 * 60 * 60;

/// Number of blocks a coinbase output must wait before it can be spent.
///
/// A coinbase created at height H becomes spendable at the beginning of the
/// state transition for height H + COINBASE_MATURITY_BLOCKS.
pub const COINBASE_MATURITY_BLOCKS: u64 = 100;

/// Consensus monetary values are represented in atomic TPZ units.
pub type AtomicUnits = u64;

pub const TPZ_DECIMALS: u8 = 8;

pub const ATOMIC_UNITS_PER_TPZ: AtomicUnits = 100_000_000;

pub const INITIAL_BLOCK_REWARD: AtomicUnits = 250_000_000;

pub const HALVING_INTERVAL: u64 = 4_200_000;

pub const MAX_SUPPLY: AtomicUnits = 2_100_000_000_000_000;

pub const LAST_SUBSIDIZED_BLOCK: u64 = 117_600_000;

pub const FIRST_ZERO_SUBSIDY_BLOCK: u64 = LAST_SUBSIDIZED_BLOCK + 1;

pub const EXPECTED_FINAL_SUPPLY: AtomicUnits = 2_099_999_945_400_000;

pub const HARD_CAP_REMAINDER: AtomicUnits = MAX_SUPPLY - EXPECTED_FINAL_SUPPLY;

// Difficulty alias retained for the existing laboratory binaries and tests.
pub const LAB_DIFFICULTY_BITS: u16 = INITIAL_DIFFICULTY_BITS;

/// Returns the subsidy for a block height in atomic TPZ units.
///
/// Height zero is the genesis block and has no subsidy. For every later
/// height, the first halving era covers blocks 1 through HALVING_INTERVAL.
pub fn reward_units(height: u64) -> AtomicUnits {
    if height == 0 {
        return 0;
    }

    let era = (height - 1) / HALVING_INTERVAL;

    if era >= u64::BITS as u64 {
        return 0;
    }

    INITIAL_BLOCK_REWARD >> era as u32
}

pub fn block_reward(height: u64) -> AtomicUnits {
    reward_units(height)
}

/// Returns the curve supply after `mined_blocks` non-genesis blocks.
///
/// The calculation iterates over subsidy eras, not individual blocks, so it
/// remains bounded and safe even for an extreme height. Once the subsidy is
/// zero, additional blocks do not change supply.
pub fn expected_supply(mined_blocks: u64) -> Result<AtomicUnits, String> {
    let last_mined_block = mined_blocks.min(LAST_SUBSIDIZED_BLOCK);

    if last_mined_block == 0 {
        return Ok(0);
    }

    let last_era = (last_mined_block - 1) / HALVING_INTERVAL;
    let mut supply = 0 as AtomicUnits;

    for era in 0..=last_era {
        let era_start = era
            .checked_mul(HALVING_INTERVAL)
            .and_then(|start| start.checked_add(1))
            .ok_or_else(|| "expected supply era overflow".to_string())?;
        let era_end = era
            .checked_add(1)
            .and_then(|next_era| next_era.checked_mul(HALVING_INTERVAL))
            .unwrap_or(u64::MAX)
            .min(last_mined_block);
        let blocks_in_era = era_end
            .checked_sub(era_start)
            .and_then(|count| count.checked_add(1))
            .ok_or_else(|| "expected supply block count overflow".to_string())?;
        let era_supply = blocks_in_era
            .checked_mul(reward_units(era_start))
            .ok_or_else(|| "expected supply multiplication overflow".to_string())?;

        supply = supply
            .checked_add(era_supply)
            .ok_or_else(|| "expected supply overflow".to_string())?;
    }

    if supply > MAX_SUPPLY {
        return Err("configured maximum supply exceeded".to_string());
    }

    Ok(supply)
}

pub fn block_work(difficulty_bits: u16) -> u128 {
    if difficulty_bits >= 127 {
        return u128::MAX;
    }

    1u128 << difficulty_bits
}

pub fn next_difficulty_bits(
    current: u16,
    previous_timestamp: u64,
    window_start_timestamp: u64,
) -> u16 {
    if previous_timestamp.saturating_sub(window_start_timestamp)
        < TARGET_BLOCK_TIME_SECS.saturating_mul(DIFFICULTY_ADJUSTMENT_INTERVAL)
    {
        return current.saturating_add(1).min(256);
    }

    if previous_timestamp.saturating_sub(window_start_timestamp)
        > TARGET_BLOCK_TIME_SECS.saturating_mul(DIFFICULTY_ADJUSTMENT_INTERVAL)
    {
        return current.saturating_sub(1);
    }

    current
}

pub fn validate_timestamp(timestamp: u64, previous_timestamp: u64, now: u64) -> bool {
    timestamp > previous_timestamp && timestamp <= now.saturating_add(MAX_FUTURE_DRIFT_SECS)
}

pub fn hash_meets_difficulty(hash: &Hash, difficulty_bits: u16) -> bool {
    if difficulty_bits > 256 {
        return false;
    }

    let full_zero_bytes = (difficulty_bits / 8) as usize;

    let remaining_bits = (difficulty_bits % 8) as u8;

    for byte in hash.iter().take(full_zero_bytes) {
        if *byte != 0 {
            return false;
        }
    }

    if remaining_bits > 0 {
        let mask = 0xFFu8 << (8 - remaining_bits);

        if hash[full_zero_bytes] & mask != 0 {
            return false;
        }
    }

    true
}

pub fn validate_proof_of_work(block: &Block, difficulty_bits: u16) -> bool {
    hash_meets_difficulty(&block.hash(), difficulty_bits)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_difficulty_accepts_any_hash() {
        let hash = [255u8; 32];

        assert!(hash_meets_difficulty(&hash, 0,));
    }

    #[test]
    fn eight_bits_requires_first_byte_zero() {
        let valid = [0u8; 32];

        let mut invalid = [0u8; 32];

        invalid[0] = 1;

        assert!(hash_meets_difficulty(&valid, 8,));

        assert!(!hash_meets_difficulty(&invalid, 8,));
    }

    #[test]
    fn twelve_bits_requires_one_byte_and_four_bits_zero() {
        let mut valid = [0u8; 32];

        valid[1] = 0x0F;

        let mut invalid = [0u8; 32];

        invalid[1] = 0x10;

        assert!(hash_meets_difficulty(&valid, 12,));

        assert!(!hash_meets_difficulty(&invalid, 12,));
    }

    #[test]
    fn difficulty_above_hash_size_is_invalid() {
        let hash = [0u8; 32];

        assert!(!hash_meets_difficulty(&hash, 257,));
    }

    #[test]
    fn monetary_constants_use_atomic_units() {
        assert_eq!(TPZ_DECIMALS, 8);
        assert_eq!(ATOMIC_UNITS_PER_TPZ, 100_000_000);
        assert_eq!(INITIAL_BLOCK_REWARD, 250_000_000);
        assert_eq!(MAX_SUPPLY, 2_100_000_000_000_000);
    }

    #[test]
    fn genesis_reward_is_zero() {
        assert_eq!(reward_units(0), 0);
    }

    #[test]
    fn first_era_starts_at_block_one() {
        assert_eq!(reward_units(1), 250_000_000);
        assert_eq!(reward_units(HALVING_INTERVAL), 250_000_000);
        assert_eq!(reward_units(HALVING_INTERVAL + 1), 125_000_000);
    }

    #[test]
    fn every_era_boundary_has_the_expected_reward() {
        for era in 0..28u64 {
            let first_block = era * HALVING_INTERVAL + 1;
            let last_block = (era + 1) * HALVING_INTERVAL;
            let expected = INITIAL_BLOCK_REWARD >> era;

            assert_eq!(reward_units(first_block), expected);
            assert_eq!(reward_units(last_block), expected);

            if era < 27 {
                assert_eq!(reward_units(last_block + 1), expected >> 1);
            }
        }
    }

    #[test]
    fn reward_never_increases_or_exceeds_initial_reward() {
        let mut previous = reward_units(0);

        for height in [
            1,
            HALVING_INTERVAL,
            HALVING_INTERVAL + 1,
            2 * HALVING_INTERVAL,
            2 * HALVING_INTERVAL + 1,
            LAST_SUBSIDIZED_BLOCK,
            FIRST_ZERO_SUBSIDY_BLOCK,
            u64::MAX,
        ] {
            let reward = reward_units(height);
            assert!(reward <= previous || height == 1);
            assert!(reward <= INITIAL_BLOCK_REWARD);
            previous = reward;
        }
    }

    #[test]
    fn final_positive_era_and_first_zero_subsidy_are_exact() {
        assert_eq!(reward_units(LAST_SUBSIDIZED_BLOCK), 1);
        assert_eq!(reward_units(FIRST_ZERO_SUBSIDY_BLOCK), 0);
    }

    #[test]
    fn expected_supply_reaches_exact_final_curve_and_stays_below_cap() {
        assert_eq!(
            expected_supply(LAST_SUBSIDIZED_BLOCK).unwrap(),
            EXPECTED_FINAL_SUPPLY
        );
        assert_eq!(expected_supply(u64::MAX).unwrap(), EXPECTED_FINAL_SUPPLY);
        assert!(EXPECTED_FINAL_SUPPLY <= MAX_SUPPLY);
    }

    #[test]
    fn hard_cap_difference_is_not_emitted() {
        assert_eq!(HARD_CAP_REMAINDER, 54_600_000);
        assert_eq!(MAX_SUPPLY - EXPECTED_FINAL_SUPPLY, HARD_CAP_REMAINDER);
    }

    #[test]
    fn expected_supply_never_exceeds_hard_cap() {
        for height in [0, 1, HALVING_INTERVAL, LAST_SUBSIDIZED_BLOCK, u64::MAX] {
            assert!(expected_supply(height).unwrap() <= MAX_SUPPLY);
        }
    }

    #[test]
    fn extreme_heights_do_not_panic_or_overflow() {
        assert_eq!(reward_units(u64::MAX), 0);
        assert_eq!(expected_supply(u64::MAX).unwrap(), EXPECTED_FINAL_SUPPLY);
    }

    #[test]
    fn monetary_supply_uses_integer_arithmetic() {
        let integer_sum = (1..=LAST_SUBSIDIZED_BLOCK)
            .step_by(HALVING_INTERVAL as usize)
            .enumerate()
            .map(|(era, first_block)| {
                let remaining = LAST_SUBSIDIZED_BLOCK - first_block + 1;
                let blocks = remaining.min(HALVING_INTERVAL);
                blocks * (INITIAL_BLOCK_REWARD >> era)
            })
            .sum::<AtomicUnits>();

        assert_eq!(integer_sum, EXPECTED_FINAL_SUPPLY);
    }

    #[test]
    fn stronger_difficulty_has_more_work() {
        assert!(block_work(13) > block_work(12));
    }

    #[test]
    fn timestamp_must_be_monotonic_and_not_far_future() {
        assert!(validate_timestamp(101, 100, 100));
        assert!(!validate_timestamp(100, 100, 100));
        assert!(!validate_timestamp(MAX_FUTURE_DRIFT_SECS + 2, 0, 0));
    }
}

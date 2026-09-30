use crate::block::Block;
use crate::crypto::{Hash, hash_to_hex};

pub const GENESIS_TIMESTAMP: u64 = 1_788_825_600;
pub const GENESIS_NONCE: u64 = 0;

pub const GENESIS_MESSAGE: &[u8] = b"TOPAZIO NETWORK | TPZ | V1 TESTNET | GENESIS | 2026-09-11";

pub const EXPECTED_GENESIS_HASH: &str =
    "70cc5c3efc942debe96d7c812533936dfc7521b894d0db956713258a1096a221";

pub fn create_genesis_block() -> Block {
    Block::new(
        0,
        [0u8; 32],
        GENESIS_TIMESTAMP,
        GENESIS_NONCE,
        GENESIS_MESSAGE.to_vec(),
    )
}

pub fn genesis_hash() -> Hash {
    create_genesis_block().hash()
}

pub fn genesis_hash_hex() -> String {
    hash_to_hex(&genesis_hash())
}

pub fn verify_genesis() -> bool {
    genesis_hash_hex() == EXPECTED_GENESIS_HASH
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn genesis_has_height_zero() {
        let genesis = create_genesis_block();

        assert_eq!(genesis.height, 0);
    }

    #[test]
    fn genesis_has_zero_previous_hash() {
        let genesis = create_genesis_block();

        assert_eq!(genesis.previous_hash, [0u8; 32]);
    }

    #[test]
    fn genesis_is_deterministic() {
        let genesis_a = create_genesis_block();
        let genesis_b = create_genesis_block();

        assert_eq!(genesis_a.hash(), genesis_b.hash());
    }

    #[test]
    fn genesis_hash_matches_expected_identity() {
        assert_eq!(genesis_hash_hex(), EXPECTED_GENESIS_HASH);
    }

    #[test]
    fn genesis_verification_succeeds() {
        assert!(verify_genesis());
    }
}

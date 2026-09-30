use crate::{block::Block, consensus::validate_proof_of_work};

pub fn mine_block(mut block: Block, difficulty_bits: u16, max_nonce: u64) -> Result<Block, String> {
    for nonce in 0..=max_nonce {
        block.nonce = nonce;

        if validate_proof_of_work(&block, difficulty_bits) {
            return Ok(block);
        }
    }

    Err(format!(
        "proof of work not found before nonce {}",
        max_nonce
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{consensus::LAB_DIFFICULTY_BITS, genesis::create_genesis_block};

    #[test]
    fn miner_finds_valid_proof_of_work() {
        let genesis = create_genesis_block();

        let candidate = Block::new(
            1,
            genesis.hash(),
            1_788_825_601,
            0,
            b"Topazio mining test".to_vec(),
        );

        let mined = mine_block(candidate, LAB_DIFFICULTY_BITS, 1_000_000)
            .expect("miner should find a proof of work");

        assert!(validate_proof_of_work(&mined, LAB_DIFFICULTY_BITS));
    }

    #[test]
    fn mined_block_keeps_original_data() {
        let genesis = create_genesis_block();

        let data = b"TPZ mining payload".to_vec();

        let candidate = Block::new(1, genesis.hash(), 1_788_825_601, 0, data.clone());

        let mined = mine_block(candidate, LAB_DIFFICULTY_BITS, 1_000_000).unwrap();

        assert_eq!(mined.data, data);
        assert_eq!(mined.height, 1);
        assert_eq!(mined.previous_hash, genesis.hash());
    }

    #[test]
    fn impossible_search_range_can_fail() {
        let genesis = create_genesis_block();

        let candidate = Block::new(
            1,
            genesis.hash(),
            1_788_825_601,
            0,
            b"restricted mining search".to_vec(),
        );

        let result = mine_block(candidate, 256, 10);

        assert!(result.is_err());
    }
}

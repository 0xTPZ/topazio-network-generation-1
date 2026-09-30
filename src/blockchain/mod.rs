use crate::{
    block::Block,
    consensus::{
        DIFFICULTY_ADJUSTMENT_INTERVAL, LAB_DIFFICULTY_BITS, block_reward, block_work,
        expected_supply, next_difficulty_bits as adjust_difficulty, validate_proof_of_work,
        validate_timestamp,
    },
    genesis::{create_genesis_block, verify_genesis},
    state::State,
    transaction::Transaction,
};

#[derive(Debug, Clone)]
pub struct Blockchain {
    blocks: Vec<Block>,
    difficulty_bits: u16,
    state: State,
}

impl Blockchain {
    pub fn new() -> Self {
        Self::with_difficulty(LAB_DIFFICULTY_BITS)
    }

    pub fn with_difficulty(difficulty_bits: u16) -> Self {
        Self {
            blocks: vec![create_genesis_block()],
            difficulty_bits: difficulty_bits.min(256),
            state: State::new(),
        }
    }

    pub fn from_blocks(difficulty_bits: u16, blocks: Vec<Block>) -> Result<Self, String> {
        if blocks.is_empty() {
            return Err("cannot restore blockchain without blocks".to_string());
        }

        if blocks[0] != create_genesis_block() {
            return Err("stored blockchain genesis does not match network genesis".to_string());
        }

        let mut blockchain = Self::with_difficulty(difficulty_bits);

        for block in blocks.into_iter().skip(1) {
            blockchain
                .append_block(block)
                .map_err(|error| format!("failed to restore blockchain: {}", error))?;
        }

        blockchain.validate()?;

        Ok(blockchain)
    }

    pub fn len(&self) -> usize {
        self.blocks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }

    pub fn blocks(&self) -> &[Block] {
        &self.blocks
    }

    pub fn genesis(&self) -> &Block {
        &self.blocks[0]
    }

    pub fn tip(&self) -> &Block {
        self.blocks
            .last()
            .expect("blockchain must always contain genesis")
    }

    pub fn difficulty_bits(&self) -> u16 {
        self.difficulty_bits
    }

    pub fn next_difficulty_bits(&self) -> u16 {
        let interval = DIFFICULTY_ADJUSTMENT_INTERVAL as usize;
        if self.blocks.len() <= interval {
            return self.difficulty_bits;
        }

        let window_start = self.blocks[self.blocks.len() - interval - 1].timestamp;
        adjust_difficulty(self.difficulty_bits, self.tip().timestamp, window_start)
    }

    pub fn cumulative_work(&self) -> u128 {
        self.blocks.iter().skip(1).fold(0u128, |work, block| {
            work.saturating_add(block_work(block.difficulty_bits))
        })
    }

    /// Returns whether `self` is the deterministic winner against `other`.
    ///
    /// Cumulative work is the primary consensus rule. The tip hash is the
    /// deterministic tie-breaker for otherwise equally-worked valid chains.
    pub fn is_better_chain_than(&self, other: &Self) -> bool {
        match self.cumulative_work().cmp(&other.cumulative_work()) {
            std::cmp::Ordering::Greater => true,
            std::cmp::Ordering::Less => false,
            std::cmp::Ordering::Equal => self.tip().hash() < other.tip().hash(),
        }
    }

    pub fn next_height(&self) -> u64 {
        self.tip().height + 1
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    pub fn total_supply(&self) -> Result<u64, String> {
        self.state.total_supply()
    }

    pub fn state_mut_for_lab(&mut self) -> &mut State {
        &mut self.state
    }

    pub fn append_block(&mut self, block: Block) -> Result<(), String> {
        if block.height != self.next_height() {
            return Err("invalid block height".to_string());
        }

        if block.previous_hash != self.tip().hash() {
            return Err("invalid previous hash".to_string());
        }

        if block.difficulty_bits > 256 {
            return Err("invalid block difficulty".to_string());
        }

        let previous_difficulty = self.tip().difficulty_bits;

        let expected_difficulty = if block.height == 1 {
            self.difficulty_bits
        } else {
            self.next_difficulty_bits()
        };

        if block.difficulty_bits != expected_difficulty {
            return Err(format!(
                "invalid difficulty: expected {}, received {}",
                expected_difficulty, block.difficulty_bits
            ));
        }

        if block.height > 1 && block.difficulty_bits.abs_diff(previous_difficulty) > 1 {
            return Err("block difficulty changed by more than one bit".to_string());
        }

        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| "system clock is before Unix epoch".to_string())?
            .as_secs();

        if !validate_timestamp(block.timestamp, self.tip().timestamp, now) {
            return Err("invalid block timestamp".to_string());
        }

        if !validate_proof_of_work(&block, block.difficulty_bits) {
            return Err("invalid proof of work".to_string());
        }

        let transactions = block
            .transactions()
            .map_err(|error| format!("invalid block transaction data: {}", error))?;

        if transactions.len() > crate::transaction::MAX_TRANSACTIONS_PER_BLOCK {
            return Err("block contains too many transactions".to_string());
        }

        self.validate_coinbase_rules(block.height, &transactions)?;

        let (next_state, valid_fees) =
            Self::apply_block_transactions(&self.state, block.height, &transactions)?;

        Self::validate_coinbase_value(block.height, transactions[0].amount, valid_fees)?;

        next_state
            .total_supply()
            .map_err(|error| format!("invalid supply in block {}: {}", block.height, error))?;

        self.blocks.push(block);

        self.difficulty_bits = self.tip().difficulty_bits;

        self.state = next_state;

        Ok(())
    }

    fn validate_coinbase_rules(
        &self,
        block_height: u64,
        transactions: &[Transaction],
    ) -> Result<(), String> {
        if transactions.is_empty() {
            return Err("block must contain coinbase transaction".to_string());
        }

        let coinbase_count = transactions
            .iter()
            .filter(|transaction| transaction.is_coinbase())
            .count();

        if coinbase_count != 1 {
            return Err("block must contain exactly one coinbase transaction".to_string());
        }

        let coinbase = &transactions[0];

        if !coinbase.is_coinbase() {
            return Err("coinbase must be the first transaction".to_string());
        }

        if coinbase.nonce != block_height {
            return Err(format!(
                "invalid coinbase height: expected {}, received {}",
                block_height, coinbase.nonce
            ));
        }

        Ok(())
    }

    fn validate_coinbase_value(
        block_height: u64,
        coinbase_value: u64,
        valid_fees: u64,
    ) -> Result<(), String> {
        let maximum_coinbase = block_reward(block_height)
            .checked_add(valid_fees)
            .ok_or_else(|| "coinbase subsidy plus fees overflow".to_string())?;

        if coinbase_value > maximum_coinbase {
            return Err(format!(
                "coinbase value exceeds subsidy plus fees: maximum {}, received {}",
                maximum_coinbase, coinbase_value
            ));
        }

        Ok(())
    }

    fn apply_block_transactions(
        base_state: &State,
        block_height: u64,
        transactions: &[Transaction],
    ) -> Result<(State, u64), String> {
        base_state.total_supply()?;

        let mut next_state = base_state.clone();
        next_state
            .advance_to_height(block_height)
            .map_err(|error| {
                format!(
                    "failed to advance state to block {}: {}",
                    block_height, error
                )
            })?;
        let mut valid_fees = 0u64;

        for transaction in transactions
            .iter()
            .filter(|transaction| !transaction.is_coinbase())
        {
            transaction.validate().map_err(|error| {
                format!("invalid transaction in block {}: {}", block_height, error)
            })?;
            valid_fees = valid_fees
                .checked_add(transaction.fee)
                .ok_or_else(|| "block fees overflow".to_string())?;
        }

        for transaction in transactions {
            let result = if transaction.is_coinbase() {
                next_state.apply_coinbase_with_fee_offset(transaction, valid_fees)
            } else {
                next_state.apply_transfer_for_block(transaction)
            };
            result.map_err(|error| {
                format!("invalid transaction in block {}: {}", block_height, error)
            })?;
        }

        Ok((next_state, valid_fees))
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.blocks.is_empty() {
            return Err("blockchain cannot be empty".to_string());
        }

        if !verify_genesis() {
            return Err("configured genesis identity is invalid".to_string());
        }

        if self.blocks[0] != create_genesis_block() {
            return Err("blockchain genesis does not match network genesis".to_string());
        }

        let mut rebuilt_state = State::new();

        for index in 1..self.blocks.len() {
            let previous = &self.blocks[index - 1];

            let current = &self.blocks[index];

            if current.height != previous.height + 1 {
                return Err(format!("invalid block height at index {}", index));
            }

            if current.previous_hash != previous.hash() {
                return Err(format!(
                    "invalid previous hash at block height {}",
                    current.height
                ));
            }

            if !validate_proof_of_work(current, current.difficulty_bits) {
                return Err(format!(
                    "invalid proof of work at block height {}",
                    current.height
                ));
            }

            let transactions = current.transactions().map_err(|error| {
                format!(
                    "invalid transaction encoding at block height {}: {}",
                    current.height, error
                )
            })?;

            if transactions.len() > crate::transaction::MAX_TRANSACTIONS_PER_BLOCK {
                return Err(format!(
                    "too many transactions at block height {}",
                    current.height
                ));
            }

            if current.height > 1 && current.difficulty_bits.abs_diff(previous.difficulty_bits) > 1
            {
                return Err(format!(
                    "difficulty changed by more than one bit at block height {}",
                    current.height
                ));
            }

            self.validate_coinbase_rules(current.height, &transactions)?;

            let (next_state, valid_fees) =
                Self::apply_block_transactions(&rebuilt_state, current.height, &transactions)?;

            Self::validate_coinbase_value(current.height, transactions[0].amount, valid_fees)?;

            rebuilt_state = next_state;
        }

        if rebuilt_state != self.state {
            return Err(
                "stored state does not match state reconstructed from blockchain".to_string(),
            );
        }

        let mined_blocks = self.blocks.len().saturating_sub(1);

        let mined_blocks =
            u64::try_from(mined_blocks).map_err(|_| "block count is too large".to_string())?;

        let expected_supply = expected_supply(mined_blocks)?;

        let actual_supply = rebuilt_state.total_supply()?;

        if actual_supply > expected_supply {
            return Err(format!(
                "invalid total supply: maximum {}, received {}",
                expected_supply, actual_supply
            ));
        }

        Ok(())
    }

    pub fn replace_with_stronger_chain(
        &mut self,
        candidate_blocks: Vec<Block>,
    ) -> Result<bool, String> {
        let candidate = Self::from_blocks(self.difficulty_bits, candidate_blocks)?;

        if !candidate.is_better_chain_than(self) {
            return Ok(false);
        }

        *self = candidate;
        Ok(true)
    }
}

impl Default for Blockchain {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::{
        consensus::{COINBASE_MATURITY_BLOCKS, INITIAL_BLOCK_REWARD},
        identity::KeyPair,
        mining::mine_block,
        transaction::{Transaction, encode_transactions},
    };

    fn miner_key() -> KeyPair {
        KeyPair::from_secret_bytes([5u8; 32])
    }

    fn alice_key() -> KeyPair {
        KeyPair::from_secret_bytes([7u8; 32])
    }

    fn bob_key() -> KeyPair {
        KeyPair::from_secret_bytes([9u8; 32])
    }

    fn mine_block_with_transactions(chain: &Blockchain, transactions: &[Transaction]) -> Block {
        let block = Block::with_transactions_at_difficulty(
            chain.next_height(),
            chain.tip().hash(),
            1_788_825_600 + chain.next_height(),
            0,
            chain.difficulty_bits(),
            transactions,
        )
        .unwrap();

        mine_block(block, chain.difficulty_bits(), 1_000_000).unwrap()
    }

    fn mine_reward_block(chain: &Blockchain, miner: &KeyPair) -> Block {
        let coinbase =
            Transaction::coinbase(miner.address(), INITIAL_BLOCK_REWARD, chain.next_height())
                .unwrap();

        mine_block_with_transactions(chain, &[coinbase])
    }

    fn mature_first_coinbase(chain: &mut Blockchain, miner: &KeyPair) {
        while chain.tip().height < COINBASE_MATURITY_BLOCKS {
            let height = chain.next_height();
            let coinbase = Transaction::coinbase(miner.address(), 0, height).unwrap();
            let block = mine_block_with_transactions(chain, &[coinbase]);
            chain.append_block(block).unwrap();
        }
    }

    #[test]
    fn new_blockchain_contains_only_genesis() {
        let blockchain = Blockchain::new();

        assert_eq!(blockchain.len(), 1);
    }

    #[test]
    fn new_blockchain_is_valid() {
        assert!(Blockchain::new().validate().is_ok());
    }

    #[test]
    fn new_blockchain_has_zero_supply() {
        let blockchain = Blockchain::new();

        assert_eq!(blockchain.total_supply().unwrap(), 0);
    }

    #[test]
    fn valid_reward_block_can_be_appended() {
        let mut blockchain = Blockchain::new();

        let miner = miner_key();

        let block = mine_reward_block(&blockchain, &miner);

        blockchain.append_block(block).unwrap();

        assert_eq!(
            blockchain.state().balance_of(&miner.address()),
            INITIAL_BLOCK_REWARD
        );

        assert_eq!(blockchain.total_supply().unwrap(), INITIAL_BLOCK_REWARD);

        assert!(blockchain.validate().is_ok());
    }

    #[test]
    fn multiple_mined_blocks_accumulate_rewards() {
        let mut blockchain = Blockchain::new();

        let miner = miner_key();

        for _ in 0..3 {
            let block = mine_reward_block(&blockchain, &miner);

            blockchain.append_block(block).unwrap();
        }

        assert_eq!(
            blockchain.state().balance_of(&miner.address()),
            INITIAL_BLOCK_REWARD * 3
        );

        assert_eq!(blockchain.total_supply().unwrap(), INITIAL_BLOCK_REWARD * 3);

        assert!(blockchain.validate().is_ok());
    }

    #[test]
    fn block_without_coinbase_is_rejected() {
        let mut blockchain = Blockchain::new();

        let block = Block::with_transactions(
            blockchain.next_height(),
            blockchain.tip().hash(),
            1_788_825_601,
            0,
            &[],
        )
        .unwrap();

        let mined = mine_block(block, blockchain.difficulty_bits(), 1_000_000).unwrap();

        assert!(blockchain.append_block(mined).is_err());
    }

    #[test]
    fn excessive_coinbase_reward_is_rejected() {
        let mut blockchain = Blockchain::new();

        let miner = miner_key();

        let coinbase = Transaction::coinbase(
            miner.address(),
            INITIAL_BLOCK_REWARD + 1,
            blockchain.next_height(),
        )
        .unwrap();

        let block = mine_block_with_transactions(&blockchain, &[coinbase]);

        assert!(blockchain.append_block(block).is_err());

        assert_eq!(blockchain.total_supply().unwrap(), 0);
    }

    #[test]
    fn multiple_coinbases_are_rejected() {
        let mut blockchain = Blockchain::new();

        let miner = miner_key();

        let coinbase_a = Transaction::coinbase(
            miner.address(),
            INITIAL_BLOCK_REWARD,
            blockchain.next_height(),
        )
        .unwrap();

        let coinbase_b = coinbase_a.clone();

        let block = mine_block_with_transactions(&blockchain, &[coinbase_a, coinbase_b]);

        assert!(blockchain.append_block(block).is_err());
    }

    #[test]
    fn coinbase_with_wrong_height_is_rejected() {
        let mut blockchain = Blockchain::new();

        let miner = miner_key();

        let coinbase = Transaction::coinbase(miner.address(), INITIAL_BLOCK_REWARD, 999).unwrap();

        let block = mine_block_with_transactions(&blockchain, &[coinbase]);

        assert!(blockchain.append_block(block).is_err());
    }

    #[test]
    fn immature_mined_reward_cannot_be_spent_before_maturity() {
        let mut blockchain = Blockchain::with_difficulty(0);

        let alice = alice_key();

        let bob = bob_key();

        let reward_block = mine_reward_block(&blockchain, &alice);

        blockchain.append_block(reward_block).unwrap();

        let coinbase = Transaction::coinbase(
            alice.address(),
            INITIAL_BLOCK_REWARD,
            blockchain.next_height(),
        )
        .unwrap();

        let transfer = Transaction::signed(&alice, bob.address(), 25, 0, 0).unwrap();

        let block = mine_block_with_transactions(&blockchain, &[coinbase, transfer]);

        assert!(blockchain.append_block(block).is_err());
        assert_eq!(blockchain.tip().height, 1);
        assert_eq!(blockchain.state().spendable_balance_of(&alice.address()), 0);
    }

    #[test]
    fn matured_coinbase_can_be_spent_at_h_plus_100() {
        let mut blockchain = Blockchain::with_difficulty(0);
        let alice = alice_key();
        let bob = bob_key();

        blockchain
            .append_block(mine_reward_block(&blockchain, &alice))
            .unwrap();
        mature_first_coinbase(&mut blockchain, &alice);

        assert_eq!(blockchain.tip().height, COINBASE_MATURITY_BLOCKS);
        assert_eq!(blockchain.state().spendable_balance_of(&alice.address()), 0);

        let height = blockchain.next_height();
        let coinbase = Transaction::coinbase(alice.address(), 0, height).unwrap();
        let transfer = Transaction::signed(&alice, bob.address(), 25, 0, 0).unwrap();
        blockchain
            .append_block(mine_block_with_transactions(
                &blockchain,
                &[coinbase, transfer],
            ))
            .unwrap();

        assert_eq!(
            blockchain.state().spendable_balance_of(&alice.address()),
            INITIAL_BLOCK_REWARD - 25
        );
        assert_eq!(blockchain.state().spendable_balance_of(&bob.address()), 25);
        assert!(blockchain.validate().is_ok());
    }

    #[test]
    fn subsidy_and_fees_share_the_same_maturity() {
        let mut blockchain = Blockchain::with_difficulty(0);
        let alice = alice_key();
        let bob = bob_key();
        let miner = miner_key();

        blockchain
            .append_block(mine_reward_block(&blockchain, &alice))
            .unwrap();
        mature_first_coinbase(&mut blockchain, &alice);

        let transfer = Transaction::signed(&alice, bob.address(), 10, 7, 0).unwrap();
        let height = blockchain.next_height();
        let coinbase =
            Transaction::coinbase(miner.address(), INITIAL_BLOCK_REWARD + 7, height).unwrap();
        blockchain
            .append_block(mine_block_with_transactions(&blockchain, &[coinbase]))
            .unwrap_err();

        let block = mine_block_with_transactions(
            &blockchain,
            &[
                Transaction::coinbase(miner.address(), INITIAL_BLOCK_REWARD + 7, height).unwrap(),
                transfer,
            ],
        );
        blockchain.append_block(block).unwrap();

        assert_eq!(
            blockchain.state().immature_balance_of(&miner.address()),
            INITIAL_BLOCK_REWARD + 7
        );
        assert_eq!(blockchain.state().spendable_balance_of(&miner.address()), 0);
    }

    #[test]
    fn replay_preserves_maturity_state() {
        let mut blockchain = Blockchain::with_difficulty(0);
        let miner = miner_key();
        blockchain
            .append_block(mine_reward_block(&blockchain, &miner))
            .unwrap();
        mature_first_coinbase(&mut blockchain, &miner);

        let replayed =
            Blockchain::from_blocks(blockchain.difficulty_bits(), blockchain.blocks().to_vec())
                .unwrap();
        assert_eq!(replayed.state(), blockchain.state());
        assert_eq!(replayed.state().spendable_balance_of(&miner.address()), 0);
        assert_eq!(
            replayed.state().immature_balance_of(&miner.address()),
            INITIAL_BLOCK_REWARD
        );
    }

    #[test]
    fn reorg_removes_coinbase_from_active_state() {
        let alice = alice_key();
        let bob = bob_key();
        let mut current = Blockchain::with_difficulty(0);
        current
            .append_block(mine_reward_block(&current, &alice))
            .unwrap();

        let mut alternate = Blockchain::with_difficulty(0);
        alternate
            .append_block(mine_reward_block(&alternate, &bob))
            .unwrap();
        alternate
            .append_block(mine_reward_block(&alternate, &bob))
            .unwrap();

        assert!(
            current
                .replace_with_stronger_chain(alternate.blocks().to_vec())
                .unwrap()
        );
        assert_eq!(current.state().total_balance_of(&alice.address()), 0);
        assert_eq!(
            current.state().immature_balance_of(&bob.address()),
            INITIAL_BLOCK_REWARD * 2
        );
    }

    #[test]
    fn reorg_can_turn_a_mature_coinbase_back_into_immature_balance() {
        let alice = alice_key();
        let bob = bob_key();
        let mut current = Blockchain::with_difficulty(0);
        current
            .append_block(mine_reward_block(&current, &alice))
            .unwrap();
        mature_first_coinbase(&mut current, &alice);
        let maturity_height = current.next_height();
        let maturity_coinbase = Transaction::coinbase(alice.address(), 0, maturity_height).unwrap();
        current
            .append_block(mine_block_with_transactions(&current, &[maturity_coinbase]))
            .unwrap();
        assert_eq!(
            current.state().spendable_balance_of(&alice.address()),
            INITIAL_BLOCK_REWARD
        );

        let mut alternate = Blockchain::with_difficulty(0);
        alternate
            .append_block(mine_reward_block(&alternate, &bob))
            .unwrap();
        alternate
            .append_block(mine_reward_block(&alternate, &alice))
            .unwrap();
        while alternate.tip().height < COINBASE_MATURITY_BLOCKS {
            let height = alternate.next_height();
            let coinbase = Transaction::coinbase(bob.address(), 0, height).unwrap();
            alternate
                .append_block(mine_block_with_transactions(&alternate, &[coinbase]))
                .unwrap();
        }

        let height = alternate.next_height();
        let coinbase = Transaction::coinbase(bob.address(), 0, height).unwrap();
        let mut nonce = 0;
        loop {
            let candidate = Block::with_transactions_at_difficulty(
                height,
                alternate.tip().hash(),
                1_788_825_600 + height,
                nonce,
                0,
                &[coinbase.clone()],
            )
            .unwrap();
            if candidate.hash() < current.tip().hash() {
                alternate.append_block(candidate).unwrap();
                break;
            }
            nonce += 1;
        }

        assert!(
            current
                .replace_with_stronger_chain(alternate.blocks().to_vec())
                .unwrap()
        );
        assert_eq!(current.tip().height, COINBASE_MATURITY_BLOCKS + 1);
        assert_eq!(current.state().spendable_balance_of(&alice.address()), 0);
        assert_eq!(
            current.state().immature_balance_of(&alice.address()),
            INITIAL_BLOCK_REWARD
        );
    }

    #[test]
    fn fees_are_debited_paid_to_miner_and_do_not_increase_supply() {
        let mut blockchain = Blockchain::with_difficulty(0);
        let alice = alice_key();
        let bob = bob_key();
        let miner = miner_key();

        let reward_block = mine_reward_block(&blockchain, &alice);
        blockchain.append_block(reward_block).unwrap();
        mature_first_coinbase(&mut blockchain, &alice);

        let transfer = Transaction::signed(&alice, bob.address(), 25, 7, 0).unwrap();
        let coinbase = Transaction::coinbase(
            miner.address(),
            INITIAL_BLOCK_REWARD + 7,
            blockchain.next_height(),
        )
        .unwrap();
        blockchain
            .append_block(mine_block_with_transactions(
                &blockchain,
                &[coinbase, transfer],
            ))
            .unwrap();

        assert_eq!(
            blockchain.state().balance_of(&alice.address()),
            INITIAL_BLOCK_REWARD - 32
        );
        assert_eq!(blockchain.state().balance_of(&bob.address()), 25);
        assert_eq!(
            blockchain.state().balance_of(&miner.address()),
            INITIAL_BLOCK_REWARD + 7
        );
        assert_eq!(blockchain.total_supply().unwrap(), INITIAL_BLOCK_REWARD * 2);
        assert!(blockchain.validate().is_ok());
    }

    #[test]
    fn multiple_fees_are_summed_once_for_coinbase() {
        let mut blockchain = Blockchain::with_difficulty(0);
        let alice = alice_key();
        let bob = bob_key();
        let carol = KeyPair::from_secret_bytes([11u8; 32]);
        let miner = miner_key();

        let reward_block = mine_reward_block(&blockchain, &alice);
        blockchain.append_block(reward_block).unwrap();
        mature_first_coinbase(&mut blockchain, &alice);

        let first = Transaction::signed(&alice, bob.address(), 30, 2, 0).unwrap();
        let second = Transaction::signed(&bob, carol.address(), 10, 3, 0).unwrap();
        let coinbase = Transaction::coinbase(
            miner.address(),
            INITIAL_BLOCK_REWARD + 5,
            blockchain.next_height(),
        )
        .unwrap();
        blockchain
            .append_block(mine_block_with_transactions(
                &blockchain,
                &[coinbase, first, second],
            ))
            .unwrap();

        assert_eq!(
            blockchain.state().balance_of(&alice.address()),
            INITIAL_BLOCK_REWARD - 32
        );
        assert_eq!(blockchain.state().balance_of(&bob.address()), 17);
        assert_eq!(blockchain.state().balance_of(&carol.address()), 10);
        assert_eq!(
            blockchain.state().balance_of(&miner.address()),
            INITIAL_BLOCK_REWARD + 5
        );
        assert_eq!(blockchain.total_supply().unwrap(), INITIAL_BLOCK_REWARD * 2);
        assert!(blockchain.validate().is_ok());
    }

    #[test]
    fn coinbase_above_subsidy_plus_fees_is_rejected() {
        let mut blockchain = Blockchain::new();
        let alice = alice_key();
        let miner = miner_key();

        let reward_block = mine_reward_block(&blockchain, &alice);
        blockchain.append_block(reward_block).unwrap();

        let transfer = Transaction::signed(&alice, miner.address(), 25, 7, 0).unwrap();
        let coinbase = Transaction::coinbase(
            miner.address(),
            INITIAL_BLOCK_REWARD + 8,
            blockchain.next_height(),
        )
        .unwrap();
        let block = mine_block_with_transactions(&blockchain, &[coinbase, transfer]);

        assert!(blockchain.append_block(block).is_err());
        assert_eq!(blockchain.tip().height, 1);
    }

    #[test]
    fn zero_subsidy_block_with_fees_is_valid() {
        let alice = alice_key();
        let bob = bob_key();
        let miner = miner_key();
        let genesis = create_genesis_block();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let prior = Block::with_difficulty(
            crate::consensus::LAST_SUBSIDIZED_BLOCK,
            genesis.hash(),
            now.saturating_sub(2),
            0,
            0,
            Vec::new(),
        )
        .unwrap();
        let mut blockchain = Blockchain {
            blocks: vec![genesis, prior.clone()],
            difficulty_bits: 0,
            state: State::new(),
        };
        blockchain
            .state_mut_for_lab()
            .set_balance_for_lab(alice.address(), 100);

        let transfer = Transaction::signed(&alice, bob.address(), 10, 3, 0).unwrap();
        let coinbase = Transaction::coinbase(
            miner.address(),
            3,
            crate::consensus::FIRST_ZERO_SUBSIDY_BLOCK,
        )
        .unwrap();
        let block = Block::with_transactions_at_difficulty(
            crate::consensus::FIRST_ZERO_SUBSIDY_BLOCK,
            prior.hash(),
            now.saturating_sub(1),
            0,
            0,
            &[coinbase, transfer],
        )
        .unwrap();

        blockchain.append_block(block).unwrap();
        assert_eq!(blockchain.state().balance_of(&miner.address()), 3);
        assert_eq!(blockchain.state().spendable_balance_of(&miner.address()), 0);
        assert_eq!(blockchain.state().immature_balance_of(&miner.address()), 3);
        assert_eq!(blockchain.state().balance_of(&alice.address()), 87);
        assert_eq!(blockchain.state().balance_of(&bob.address()), 10);
        assert_eq!(blockchain.total_supply().unwrap(), 100);
    }

    #[test]
    fn zero_subsidy_block_without_fees_is_valid_without_emission() {
        let miner = miner_key();
        let genesis = create_genesis_block();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let prior = Block::with_difficulty(
            crate::consensus::LAST_SUBSIDIZED_BLOCK,
            genesis.hash(),
            now.saturating_sub(2),
            0,
            0,
            Vec::new(),
        )
        .unwrap();
        let mut blockchain = Blockchain {
            blocks: vec![genesis, prior.clone()],
            difficulty_bits: 0,
            state: State::new(),
        };
        blockchain
            .state_mut_for_lab()
            .set_balance_for_lab(miner.address(), 100);

        let coinbase = Transaction::coinbase(
            miner.address(),
            0,
            crate::consensus::FIRST_ZERO_SUBSIDY_BLOCK,
        )
        .unwrap();
        let block = Block::with_transactions_at_difficulty(
            crate::consensus::FIRST_ZERO_SUBSIDY_BLOCK,
            prior.hash(),
            now.saturating_sub(1),
            0,
            0,
            &[coinbase],
        )
        .unwrap();

        blockchain.append_block(block).unwrap();
        assert_eq!(blockchain.state().balance_of(&miner.address()), 100);
        assert_eq!(blockchain.total_supply().unwrap(), 100);
    }

    #[test]
    fn fee_coinbase_does_not_overflow_hard_cap_before_transfer_debit() {
        let alice = alice_key();
        let miner = miner_key();
        let genesis = create_genesis_block();
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let prior = Block::with_difficulty(
            crate::consensus::LAST_SUBSIDIZED_BLOCK,
            genesis.hash(),
            now.saturating_sub(2),
            0,
            0,
            Vec::new(),
        )
        .unwrap();
        let mut blockchain = Blockchain {
            blocks: vec![genesis, prior.clone()],
            difficulty_bits: 0,
            state: State::new(),
        };
        blockchain
            .state_mut_for_lab()
            .set_balance_for_lab(alice.address(), crate::consensus::MAX_SUPPLY);

        let transfer = Transaction::signed(&alice, miner.address(), 1, 10, 0).unwrap();
        let coinbase = Transaction::coinbase(
            miner.address(),
            10,
            crate::consensus::FIRST_ZERO_SUBSIDY_BLOCK,
        )
        .unwrap();
        let block = Block::with_transactions_at_difficulty(
            crate::consensus::FIRST_ZERO_SUBSIDY_BLOCK,
            prior.hash(),
            now.saturating_sub(1),
            0,
            0,
            &[coinbase, transfer],
        )
        .unwrap();

        blockchain.append_block(block).unwrap();
        assert_eq!(
            blockchain.total_supply().unwrap(),
            crate::consensus::MAX_SUPPLY
        );
    }

    #[test]
    fn block_with_tampered_signature_is_rejected() {
        let mut blockchain = Blockchain::new();

        let alice = alice_key();

        let bob = bob_key();

        let reward = mine_reward_block(&blockchain, &alice);

        blockchain.append_block(reward).unwrap();

        let coinbase = Transaction::coinbase(
            alice.address(),
            INITIAL_BLOCK_REWARD,
            blockchain.next_height(),
        )
        .unwrap();

        let mut transfer = Transaction::signed(&alice, bob.address(), 25, 0, 0).unwrap();

        transfer.amount = 99;

        let data = encode_transactions(&[coinbase, transfer]);

        let candidate = Block::new(
            blockchain.next_height(),
            blockchain.tip().hash(),
            1_788_825_602,
            0,
            data,
        );

        let mined = mine_block(candidate, blockchain.difficulty_bits(), 1_000_000).unwrap();

        assert!(blockchain.append_block(mined).is_err());

        assert_eq!(blockchain.total_supply().unwrap(), INITIAL_BLOCK_REWARD);
    }

    #[test]
    fn validation_detects_corrupted_runtime_state() {
        let mut blockchain = Blockchain::new();

        let miner = miner_key();

        let block = mine_reward_block(&blockchain, &miner);

        blockchain.append_block(block).unwrap();

        assert!(blockchain.validate().is_ok());

        blockchain
            .state_mut_for_lab()
            .set_balance_for_lab(miner.address(), 999_999);

        assert!(blockchain.validate().is_err());
    }

    #[test]
    fn blockchain_can_be_rebuilt_from_blocks() {
        let mut original = Blockchain::new();

        let miner = miner_key();

        for _ in 0..2 {
            let block = mine_reward_block(&original, &miner);

            original.append_block(block).unwrap();
        }

        let rebuilt =
            Blockchain::from_blocks(original.difficulty_bits(), original.blocks().to_vec())
                .unwrap();

        assert_eq!(rebuilt.blocks(), original.blocks());

        assert_eq!(rebuilt.state(), original.state());

        assert_eq!(
            rebuilt.total_supply().unwrap(),
            original.total_supply().unwrap()
        );

        assert!(rebuilt.validate().is_ok());
    }

    #[test]
    fn rebuild_rejects_wrong_genesis() {
        let mut blocks = Blockchain::new().blocks().to_vec();

        blocks[0].nonce = 999;

        assert!(Blockchain::from_blocks(LAB_DIFFICULTY_BITS, blocks,).is_err());
    }
}

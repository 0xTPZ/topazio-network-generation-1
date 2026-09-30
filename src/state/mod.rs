use std::collections::{BTreeMap, HashMap};

use crate::{
    consensus::{AtomicUnits, COINBASE_MATURITY_BLOCKS, MAX_SUPPLY},
    transaction::{Transaction, TransactionKind},
};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct AccountState {
    /// Confirmed balance that can fund a transfer.
    pub balance: AtomicUnits,
    /// Confirmed coinbase balance that is not yet spendable.
    pub immature_balance: AtomicUnits,
    pub nonce: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct State {
    accounts: HashMap<String, AccountState>,
    current_height: u64,
    immature_coinbases: BTreeMap<u64, Vec<(String, AtomicUnits)>>,
}

impl State {
    pub fn new() -> Self {
        Self {
            accounts: HashMap::new(),
            current_height: 0,
            immature_coinbases: BTreeMap::new(),
        }
    }

    pub fn balance_of(&self, address: &str) -> AtomicUnits {
        self.total_balance_of(address)
    }

    pub fn total_balance_of(&self, address: &str) -> AtomicUnits {
        self.try_total_balance_of(address)
            .expect("account total balance overflow")
    }

    pub fn try_total_balance_of(&self, address: &str) -> Result<AtomicUnits, String> {
        self.accounts
            .get(address)
            .map(|account| {
                account
                    .balance
                    .checked_add(account.immature_balance)
                    .ok_or_else(|| "account total balance overflow".to_string())
            })
            .unwrap_or(Ok(0))
    }

    pub fn spendable_balance_of(&self, address: &str) -> AtomicUnits {
        self.accounts
            .get(address)
            .map(|account| account.balance)
            .unwrap_or(0)
    }

    pub fn immature_balance_of(&self, address: &str) -> AtomicUnits {
        self.accounts
            .get(address)
            .map(|account| account.immature_balance)
            .unwrap_or(0)
    }

    pub fn current_height(&self) -> u64 {
        self.current_height
    }

    pub fn nonce_of(&self, address: &str) -> u64 {
        self.accounts
            .get(address)
            .map(|account| account.nonce)
            .unwrap_or(0)
    }

    pub fn account(&self, address: &str) -> AccountState {
        self.accounts.get(address).cloned().unwrap_or_default()
    }

    pub fn total_supply(&self) -> Result<AtomicUnits, String> {
        let mut total = 0 as AtomicUnits;

        for account in self.accounts.values() {
            total = total
                .checked_add(account.balance)
                .and_then(|value| value.checked_add(account.immature_balance))
                .ok_or_else(|| "total supply overflow".to_string())?;
        }

        if total > MAX_SUPPLY {
            return Err("total supply exceeds hard cap".to_string());
        }

        Ok(total)
    }

    pub fn set_balance_for_lab(&mut self, address: impl Into<String>, balance: AtomicUnits) {
        let address = address.into();

        let account = self.accounts.entry(address).or_default();

        account.balance = balance;
        account.immature_balance = 0;
    }

    pub fn apply_transaction(&mut self, transaction: &Transaction) -> Result<(), String> {
        if transaction.is_coinbase() {
            // This convenience API is used by isolated state/mempool tests.
            // A block application supplies its exact height explicitly below.
            let execution_height = self.current_height.max(transaction.nonce);
            self.apply_transaction_at_height(transaction, execution_height)
        } else {
            transaction.validate()?;
            self.total_supply()?;
            self.apply_transfer(transaction)
        }
    }

    pub(crate) fn apply_transaction_at_height(
        &mut self,
        transaction: &Transaction,
        height: u64,
    ) -> Result<(), String> {
        self.advance_to_height(height)?;
        transaction.validate()?;

        // Reject an already-invalid runtime state before any transaction can
        // make progress from it.
        self.total_supply()?;

        match transaction.kind {
            TransactionKind::Transfer => self.apply_transfer(transaction),
            TransactionKind::Coinbase => self.apply_coinbase(transaction),
        }
    }

    pub fn advance_to_height(&mut self, height: u64) -> Result<(), String> {
        if height < self.current_height {
            return Err(format!(
                "state height cannot move backwards from {} to {}",
                self.current_height, height
            ));
        }

        // Apply promotions to a clone so a corrupted state can never be left
        // partially modified when an underflow/overflow is detected.
        let mut next_state = self.clone();
        let maturity_heights: Vec<u64> = next_state
            .immature_coinbases
            .range(..=height)
            .map(|(maturity_height, _)| *maturity_height)
            .collect();

        for maturity_height in maturity_heights {
            if let Some(outputs) = next_state.immature_coinbases.remove(&maturity_height) {
                for (address, amount) in outputs {
                    let account = next_state.accounts.entry(address).or_default();
                    account.immature_balance = account
                        .immature_balance
                        .checked_sub(amount)
                        .ok_or_else(|| "immature balance underflow".to_string())?;
                    account.balance = account
                        .balance
                        .checked_add(amount)
                        .ok_or_else(|| "spendable balance overflow".to_string())?;
                }
            }
        }

        next_state.current_height = height;
        *self = next_state;
        Ok(())
    }

    fn apply_transfer(&mut self, transaction: &Transaction) -> Result<(), String> {
        let sender = self.account(&transaction.from);

        let recipient = self.account(&transaction.to);

        if transaction.nonce != sender.nonce {
            return Err(format!(
                "invalid nonce for {}: expected {}, received {}",
                transaction.from, sender.nonce, transaction.nonce
            ));
        }

        let total_debit = transaction.total_debit()?;

        let new_sender_balance = sender.balance.checked_sub(total_debit).ok_or_else(|| {
            format!(
                "insufficient balance for {}: available {}, required {}",
                transaction.from, sender.balance, total_debit
            )
        })?;

        let new_sender_nonce = sender
            .nonce
            .checked_add(1)
            .ok_or_else(|| "sender nonce overflow".to_string())?;

        let new_recipient_balance = recipient
            .balance
            .checked_add(transaction.amount)
            .ok_or_else(|| "recipient balance overflow".to_string())?;

        self.accounts.insert(
            transaction.from.clone(),
            AccountState {
                balance: new_sender_balance,
                immature_balance: sender.immature_balance,
                nonce: new_sender_nonce,
            },
        );

        self.accounts.insert(
            transaction.to.clone(),
            AccountState {
                balance: new_recipient_balance,
                immature_balance: recipient.immature_balance,
                nonce: recipient.nonce,
            },
        );

        Ok(())
    }

    fn apply_coinbase(&mut self, transaction: &Transaction) -> Result<(), String> {
        self.apply_coinbase_with_fee_offset(transaction, 0)
    }

    pub(crate) fn apply_coinbase_with_fee_offset(
        &mut self,
        transaction: &Transaction,
        fee_offset: AtomicUnits,
    ) -> Result<(), String> {
        transaction.validate()?;

        let recipient = self.account(&transaction.to);

        let current_supply = self.total_supply()?;

        // Fees already exist in the supply and are transferred through the
        // block. Only the subsidy portion is new issuance for the hard-cap
        // check. Underclaiming fees burns the unclaimed remainder.
        let issued_amount = transaction.amount.checked_sub(fee_offset).unwrap_or(0);
        let new_supply = current_supply
            .checked_add(issued_amount)
            .ok_or_else(|| "coinbase supply overflow".to_string())?;

        if new_supply > MAX_SUPPLY {
            return Err("coinbase would exceed hard cap".to_string());
        }

        let new_balance = recipient
            .immature_balance
            .checked_add(transaction.amount)
            .ok_or_else(|| "coinbase balance overflow".to_string())?;

        let maturity_height = transaction
            .nonce
            .checked_add(COINBASE_MATURITY_BLOCKS)
            .ok_or_else(|| "coinbase maturity height overflow".to_string())?;

        self.accounts.insert(
            transaction.to.clone(),
            AccountState {
                balance: recipient.balance,
                immature_balance: new_balance,
                nonce: recipient.nonce,
            },
        );

        self.immature_coinbases
            .entry(maturity_height)
            .or_default()
            .push((transaction.to.clone(), transaction.amount));

        Ok(())
    }

    pub(crate) fn apply_transfer_for_block(
        &mut self,
        transaction: &Transaction,
    ) -> Result<(), String> {
        transaction.validate()?;

        if transaction.kind != TransactionKind::Transfer {
            return Err("block transfer application received non-transfer".to_string());
        }

        // A block may temporarily hold its fee-bearing coinbase before the
        // corresponding debits are applied. The final block state is checked
        // by the caller; skipping only this intermediate supply check avoids
        // treating fees as new issuance at the hard-cap boundary.
        self.apply_transfer(transaction)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        consensus::{COINBASE_MATURITY_BLOCKS, MAX_SUPPLY},
        identity::KeyPair,
    };

    fn alice_key() -> KeyPair {
        KeyPair::from_secret_bytes([7u8; 32])
    }

    fn bob_key() -> KeyPair {
        KeyPair::from_secret_bytes([9u8; 32])
    }

    #[test]
    fn unknown_account_has_zero_balance_and_nonce() {
        let state = State::new();

        let alice = alice_key();

        assert_eq!(state.balance_of(&alice.address()), 0);

        assert_eq!(state.nonce_of(&alice.address()), 0);
    }

    #[test]
    fn lab_balance_can_be_initialized() {
        let mut state = State::new();

        let alice = alice_key();

        state.set_balance_for_lab(alice.address(), 100);

        assert_eq!(state.balance_of(&alice.address()), 100);
    }

    #[test]
    fn empty_state_has_zero_supply() {
        let state = State::new();

        assert_eq!(state.total_supply().unwrap(), 0);
    }

    #[test]
    fn coinbase_creates_balance_and_supply() {
        let mut state = State::new();

        let alice = alice_key();

        let coinbase = Transaction::coinbase(alice.address(), 50, 1).unwrap();

        state.apply_transaction(&coinbase).unwrap();

        assert_eq!(state.balance_of(&alice.address()), 50);

        assert_eq!(state.total_supply().unwrap(), 50);
    }

    #[test]
    fn coinbase_does_not_change_nonce() {
        let mut state = State::new();

        let alice = alice_key();

        let coinbase = Transaction::coinbase(alice.address(), 50, 1).unwrap();

        state.apply_transaction(&coinbase).unwrap();

        assert_eq!(state.nonce_of(&alice.address()), 0);
    }

    #[test]
    fn coinbase_is_created_as_immature_and_total_is_partitioned() {
        let mut state = State::new();
        let alice = alice_key();
        let coinbase = Transaction::coinbase(alice.address(), 50, 1).unwrap();

        state.apply_transaction(&coinbase).unwrap();

        assert_eq!(state.total_balance_of(&alice.address()), 50);
        assert_eq!(state.spendable_balance_of(&alice.address()), 0);
        assert_eq!(state.immature_balance_of(&alice.address()), 50);
        assert_eq!(state.total_supply().unwrap(), 50);
    }

    #[test]
    fn coinbase_maturity_boundary_is_exact() {
        let mut state = State::new();
        let alice = alice_key();
        let coinbase = Transaction::coinbase(alice.address(), 50, 1).unwrap();
        state.apply_transaction(&coinbase).unwrap();

        state
            .advance_to_height(1 + COINBASE_MATURITY_BLOCKS - 2)
            .unwrap();
        assert_eq!(state.spendable_balance_of(&alice.address()), 0); // H + 98

        state
            .advance_to_height(1 + COINBASE_MATURITY_BLOCKS - 1)
            .unwrap();
        assert_eq!(state.spendable_balance_of(&alice.address()), 0); // H + 99

        state
            .advance_to_height(1 + COINBASE_MATURITY_BLOCKS)
            .unwrap();
        assert_eq!(state.spendable_balance_of(&alice.address()), 50); // H + 100
        assert_eq!(state.immature_balance_of(&alice.address()), 0);
    }

    #[test]
    fn immature_coinbase_cannot_fund_transfer() {
        let mut state = State::new();
        let alice = alice_key();
        let bob = bob_key();
        state
            .apply_transaction(&Transaction::coinbase(alice.address(), 50, 1).unwrap())
            .unwrap();

        let transfer = Transaction::signed(&alice, bob.address(), 1, 0, 0).unwrap();
        assert!(state.apply_transaction(&transfer).is_err());
        assert_eq!(state.total_balance_of(&alice.address()), 50);
    }

    #[test]
    fn zero_coinbase_preserves_partition_and_supply() {
        let mut state = State::new();
        let alice = alice_key();
        state
            .apply_transaction(&Transaction::coinbase(alice.address(), 0, 1).unwrap())
            .unwrap();

        assert_eq!(state.total_supply().unwrap(), 0);
        assert_eq!(state.total_balance_of(&alice.address()), 0);
    }

    #[test]
    fn coinbase_cannot_exceed_hard_cap() {
        let mut state = State::new();
        let alice = alice_key();

        state.set_balance_for_lab(alice.address(), MAX_SUPPLY - 1);

        let coinbase = Transaction::coinbase(alice.address(), 2, 1).unwrap();

        assert!(state.apply_transaction(&coinbase).is_err());
        assert_eq!(state.balance_of(&alice.address()), MAX_SUPPLY - 1);
    }

    #[test]
    fn valid_transaction_moves_balance() {
        let mut state = State::new();

        let alice = alice_key();

        let bob = bob_key();

        state.set_balance_for_lab(alice.address(), 100);

        let transaction = Transaction::signed(&alice, bob.address(), 25, 0, 0).unwrap();

        state.apply_transaction(&transaction).unwrap();

        assert_eq!(state.balance_of(&alice.address()), 75);

        assert_eq!(state.balance_of(&bob.address()), 25);
    }

    #[test]
    fn fee_is_debited_but_recipient_receives_only_amount() {
        let mut state = State::new();
        let alice = alice_key();
        let bob = bob_key();
        state.set_balance_for_lab(alice.address(), 100);

        let transaction = Transaction::signed(&alice, bob.address(), 25, 7, 0).unwrap();
        state.apply_transaction(&transaction).unwrap();

        assert_eq!(state.balance_of(&alice.address()), 68);
        assert_eq!(state.balance_of(&bob.address()), 25);
    }

    #[test]
    fn transfer_preserves_total_supply() {
        let mut state = State::new();

        let alice = alice_key();

        let bob = bob_key();

        state.set_balance_for_lab(alice.address(), 100);

        let before = state.total_supply().unwrap();

        let transaction = Transaction::signed(&alice, bob.address(), 25, 0, 0).unwrap();

        state.apply_transaction(&transaction).unwrap();

        let after = state.total_supply().unwrap();

        assert_eq!(before, after);

        assert_eq!(after, 100);
    }

    #[test]
    fn valid_transaction_increments_sender_nonce() {
        let mut state = State::new();

        let alice = alice_key();

        let bob = bob_key();

        state.set_balance_for_lab(alice.address(), 100);

        let transaction = Transaction::signed(&alice, bob.address(), 25, 0, 0).unwrap();

        state.apply_transaction(&transaction).unwrap();

        assert_eq!(state.nonce_of(&alice.address()), 1);
    }

    #[test]
    fn replayed_transaction_is_rejected() {
        let mut state = State::new();

        let alice = alice_key();

        let bob = bob_key();

        state.set_balance_for_lab(alice.address(), 100);

        let transaction = Transaction::signed(&alice, bob.address(), 25, 0, 0).unwrap();

        state.apply_transaction(&transaction).unwrap();

        assert!(state.apply_transaction(&transaction,).is_err());
    }

    #[test]
    fn transaction_with_future_nonce_is_rejected() {
        let mut state = State::new();

        let alice = alice_key();

        let bob = bob_key();

        state.set_balance_for_lab(alice.address(), 100);

        let transaction = Transaction::signed(&alice, bob.address(), 25, 0, 1).unwrap();

        assert!(state.apply_transaction(&transaction,).is_err());
    }

    #[test]
    fn insufficient_balance_is_rejected() {
        let mut state = State::new();

        let alice = alice_key();

        let bob = bob_key();

        state.set_balance_for_lab(alice.address(), 10);

        let transaction = Transaction::signed(&alice, bob.address(), 25, 0, 0).unwrap();

        assert!(state.apply_transaction(&transaction,).is_err());
    }

    #[test]
    fn insufficient_balance_including_fee_is_rejected() {
        let mut state = State::new();
        let alice = alice_key();
        let bob = bob_key();
        state.set_balance_for_lab(alice.address(), 25);

        let transaction = Transaction::signed(&alice, bob.address(), 25, 1, 0).unwrap();

        assert!(state.apply_transaction(&transaction).is_err());
        assert_eq!(state.balance_of(&alice.address()), 25);
        assert_eq!(state.balance_of(&bob.address()), 0);
    }

    #[test]
    fn sequential_transactions_require_sequential_nonces() {
        let mut state = State::new();

        let alice = alice_key();

        let bob = bob_key();

        let carol = KeyPair::from_secret_bytes([11u8; 32]);

        state.set_balance_for_lab(alice.address(), 100);

        let first = Transaction::signed(&alice, bob.address(), 25, 0, 0).unwrap();

        let second = Transaction::signed(&alice, carol.address(), 10, 0, 1).unwrap();

        state.apply_transaction(&first).unwrap();

        state.apply_transaction(&second).unwrap();

        assert_eq!(state.balance_of(&alice.address()), 65);

        assert_eq!(state.nonce_of(&alice.address()), 2);
    }

    #[test]
    fn failed_overflow_does_not_modify_state() {
        let mut state = State::new();

        let alice = alice_key();

        let bob = bob_key();

        state.set_balance_for_lab(alice.address(), 100);

        state.set_balance_for_lab(bob.address(), u64::MAX);

        let transaction = Transaction::signed(&alice, bob.address(), 1, 0, 0).unwrap();

        assert!(state.apply_transaction(&transaction,).is_err());

        assert_eq!(state.balance_of(&alice.address()), 100);
    }

    #[test]
    fn tampered_transaction_does_not_modify_state() {
        let mut state = State::new();

        let alice = alice_key();

        let bob = bob_key();

        state.set_balance_for_lab(alice.address(), 100);

        let mut transaction = Transaction::signed(&alice, bob.address(), 25, 0, 0).unwrap();

        transaction.amount = 90;

        assert!(state.apply_transaction(&transaction,).is_err());

        assert_eq!(state.balance_of(&alice.address()), 100);
    }
}

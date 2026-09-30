use std::{
    collections::HashSet,
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::{
    block::MAX_BLOCK_DATA_SIZE,
    state::State,
    transaction::{
        MAX_TRANSACTIONS_PER_BLOCK, Transaction, decode_transactions, encode_transactions,
    },
};

const MEMPOOL_MAGIC: &[u8; 8] = b"TPZMEM01";

const MEMPOOL_VERSION: u16 = 2;

pub const MAX_MEMPOOL_TRANSACTIONS: usize = 10_000;
pub const MAX_MEMPOOL_FILE_SIZE: usize = 64 * 1024 * 1024;

#[derive(Debug, Clone)]
pub struct Mempool {
    transactions: Vec<Transaction>,
    max_transactions: usize,
}

impl Mempool {
    pub fn new() -> Self {
        Self::new_with_limit(MAX_MEMPOOL_TRANSACTIONS).expect("default mempool limit is valid")
    }

    pub fn new_with_limit(max_transactions: usize) -> Result<Self, String> {
        if max_transactions == 0 || max_transactions > MAX_MEMPOOL_TRANSACTIONS {
            return Err("mempool transaction limit is outside the network range".to_string());
        }

        Ok(Self {
            transactions: Vec::new(),
            max_transactions,
        })
    }

    pub fn len(&self) -> usize {
        self.transactions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.transactions.is_empty()
    }

    pub fn transactions(&self) -> &[Transaction] {
        &self.transactions
    }

    pub fn contains_hash(&self, hash: &[u8; 32]) -> bool {
        self.transactions
            .iter()
            .any(|transaction| &transaction.hash() == hash)
    }

    pub fn transaction_by_hash(&self, hash: &[u8; 32]) -> Option<&Transaction> {
        self.transactions
            .iter()
            .find(|transaction| &transaction.hash() == hash)
    }

    pub fn add_transaction(
        &mut self,
        transaction: Transaction,
        confirmed_state: &State,
    ) -> Result<(), String> {
        if self.transactions.len() >= self.max_transactions {
            return Err("mempool transaction limit reached".to_string());
        }

        /*
         * Coinbase nunca entra no mempool.
         */
        if transaction.is_coinbase() {
            return Err("coinbase transaction cannot enter mempool".to_string());
        }

        transaction
            .validate()
            .map_err(|error| format!("invalid transaction: {}", error))?;

        let transaction_hash = transaction.hash();

        /*
         * A mesma transação não pode
         * aparecer duas vezes.
         */
        if self
            .transactions
            .iter()
            .any(|pending| pending.hash() == transaction_hash)
        {
            return Err("transaction already exists in mempool".to_string());
        }

        /*
         * Projetamos o estado confirmado
         * junto das transações que já estão
         * aguardando confirmação.
         */
        let mut projected_state = confirmed_state.clone();

        for pending in &self.transactions {
            projected_state
                .apply_transaction(pending)
                .map_err(|error| {
                    format!("existing mempool transaction became invalid: {}", error)
                })?;
        }

        projected_state
            .apply_transaction(&transaction)
            .map_err(|error| format!("transaction rejected by mempool state: {}", error))?;

        self.transactions.push(transaction);

        Ok(())
    }

    pub fn transactions_for_block(
        &self,
        coinbase: Transaction,
        confirmed_state: &State,
    ) -> Result<Vec<Transaction>, String> {
        if !coinbase.is_coinbase() {
            return Err("first block transaction must be coinbase".to_string());
        }

        let coinbase_size = coinbase.encode().len();

        let mut encoded_size = 8usize
            .checked_add(8)
            .and_then(|size| size.checked_add(coinbase_size))
            .ok_or_else(|| "block transaction data size overflow".to_string())?;

        if encoded_size > MAX_BLOCK_DATA_SIZE {
            return Err("coinbase transaction exceeds the block data limit".to_string());
        }

        let capacity = self
            .transactions
            .len()
            .saturating_add(1)
            .min(MAX_TRANSACTIONS_PER_BLOCK);

        let mut transactions = Vec::with_capacity(capacity);

        let mut projected_state = confirmed_state.clone();

        projected_state
            .apply_transaction(&coinbase)
            .map_err(|error| format!("invalid coinbase for block selection: {}", error))?;

        transactions.push(coinbase);

        let mut ordered_indices: Vec<usize> = (0..self.transactions.len()).collect();
        ordered_indices.sort_by(|left, right| {
            self.transactions[*right]
                .fee
                .cmp(&self.transactions[*left].fee)
                .then_with(|| {
                    self.transactions[*left]
                        .hash()
                        .cmp(&self.transactions[*right].hash())
                })
        });

        let mut seen_hashes = HashSet::new();

        // Scan repeatedly so a low-fee nonce predecessor can unlock a
        // higher-fee dependent transaction while preserving fee priority.
        loop {
            let mut added_in_pass = false;

            for index in &ordered_indices {
                if transactions.len() >= MAX_TRANSACTIONS_PER_BLOCK {
                    break;
                }

                let pending = &self.transactions[*index];

                if pending.is_coinbase() {
                    continue;
                }

                let pending_hash = pending.hash();

                if seen_hashes.contains(&pending_hash) {
                    continue;
                }

                // Avoid repeatedly verifying signatures for transactions
                // that are not yet eligible because an earlier nonce is
                // still pending.
                if projected_state.nonce_of(&pending.from) != pending.nonce {
                    continue;
                }

                let pending_size = pending.encode().len();

                let next_encoded_size = encoded_size
                    .checked_add(8)
                    .and_then(|size| size.checked_add(pending_size))
                    .ok_or_else(|| "block transaction data size overflow".to_string())?;

                if next_encoded_size > MAX_BLOCK_DATA_SIZE {
                    continue;
                }

                let mut candidate_state = projected_state.clone();

                if candidate_state.apply_transaction(pending).is_err() {
                    continue;
                }

                projected_state = candidate_state;
                encoded_size = next_encoded_size;
                seen_hashes.insert(pending_hash);
                transactions.push(pending.clone());
                added_in_pass = true;
            }

            if !added_in_pass {
                break;
            }
        }

        Ok(transactions)
    }

    pub fn remove_confirmed(&mut self, confirmed: &[Transaction]) {
        self.transactions.retain(|pending| {
            !confirmed
                .iter()
                .filter(|transaction| !transaction.is_coinbase())
                .any(|transaction| transaction.hash() == pending.hash())
        });
    }

    pub fn revalidated(&self, confirmed_state: &State) -> Result<Self, String> {
        let mut mempool = Self::new_with_limit(self.max_transactions)?;

        for transaction in &self.transactions {
            let _ = mempool.add_transaction(transaction.clone(), confirmed_state);
        }

        Ok(mempool)
    }

    pub fn clear(&mut self) {
        self.transactions.clear();
    }

    /*
     * Salva somente transações pendentes.
     *
     * O estado econômico continua sendo
     * responsabilidade exclusiva da
     * blockchain.
     */
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), String> {
        let path = path.as_ref();

        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).map_err(|error| {
                    format!("failed to create mempool storage directory: {}", error)
                })?;
            }
        }

        let encoded_transactions = encode_transactions(&self.transactions);

        let mut bytes = Vec::new();

        bytes.extend_from_slice(MEMPOOL_MAGIC);

        bytes.extend_from_slice(&MEMPOOL_VERSION.to_le_bytes());

        bytes.extend_from_slice(&encoded_transactions);

        if bytes.len() > MAX_MEMPOOL_FILE_SIZE {
            return Err("mempool storage exceeds the network limit".to_string());
        }

        let temporary_path = temporary_path(path)?;

        write_and_sync(&temporary_path, &bytes)?;

        /*
         * Antes de promover o arquivo
         * temporário, verificamos se ele
         * pode ser decodificado.
         *
         * A validação econômica completa
         * ocorre durante load().
         */
        decode_file(
            &fs::read(&temporary_path)
                .map_err(|error| format!("failed to verify temporary mempool file: {}", error))?,
        )?;

        replace_file(&temporary_path, path)?;

        Ok(())
    }

    /*
     * Carrega as transações e reexecuta
     * uma a uma contra o estado confirmado
     * atual da blockchain.
     *
     * Isso impede restaurar uma pendência
     * que já não seja economicamente válida.
     */
    pub fn load(path: impl AsRef<Path>, confirmed_state: &State) -> Result<Self, String> {
        Self::load_with_limit(path, confirmed_state, MAX_MEMPOOL_TRANSACTIONS)
    }

    pub fn load_with_limit(
        path: impl AsRef<Path>,
        confirmed_state: &State,
        max_transactions: usize,
    ) -> Result<Self, String> {
        let bytes = fs::read(path.as_ref())
            .map_err(|error| format!("failed to read mempool storage: {}", error))?;

        let transactions = decode_file(&bytes)?;

        let mut mempool = Self::new_with_limit(max_transactions)?;

        for transaction in transactions {
            mempool
                .add_transaction(transaction, confirmed_state)
                .map_err(|error| {
                    format!("stored mempool transaction is no longer valid: {}", error)
                })?;
        }

        Ok(mempool)
    }
}

impl Default for Mempool {
    fn default() -> Self {
        Self::new()
    }
}

fn decode_file(bytes: &[u8]) -> Result<Vec<Transaction>, String> {
    if bytes.len() > MAX_MEMPOOL_FILE_SIZE {
        return Err("mempool storage exceeds the network limit".to_string());
    }

    let minimum_length = MEMPOOL_MAGIC.len() + 2;

    if bytes.len() < minimum_length {
        return Err("mempool storage is truncated".to_string());
    }

    if &bytes[..MEMPOOL_MAGIC.len()] != MEMPOOL_MAGIC {
        return Err("invalid Topazio mempool storage magic".to_string());
    }

    let version_start = MEMPOOL_MAGIC.len();

    let version_end = version_start + 2;

    let mut version_bytes = [0u8; 2];

    version_bytes.copy_from_slice(&bytes[version_start..version_end]);

    let version = u16::from_le_bytes(version_bytes);

    if version != MEMPOOL_VERSION {
        return Err(format!("unsupported mempool storage version: {}", version));
    }

    decode_transactions(&bytes[version_end..])
        .map_err(|error| format!("invalid mempool transaction data: {}", error))
}

fn write_and_sync(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(path)
        .map_err(|error| format!("failed to create temporary mempool file: {}", error))?;

    if let Err(error) = file.write_all(bytes) {
        let _ = fs::remove_file(path);

        return Err(format!("failed to write temporary mempool file: {}", error));
    }

    if let Err(error) = file.flush() {
        let _ = fs::remove_file(path);

        return Err(format!("failed to flush temporary mempool file: {}", error));
    }

    if let Err(error) = file.sync_all() {
        let _ = fs::remove_file(path);

        return Err(format!(
            "failed to synchronize temporary mempool file: {}",
            error
        ));
    }

    drop(file);

    Ok(())
}

fn replace_file(temporary_path: &Path, destination_path: &Path) -> Result<(), String> {
    if !destination_path.exists() {
        fs::rename(temporary_path, destination_path)
            .map_err(|error| format!("failed to promote temporary mempool file: {}", error))?;

        return Ok(());
    }

    let backup_path = backup_path(destination_path);

    if backup_path.exists() {
        fs::remove_file(&backup_path)
            .map_err(|error| format!("failed to remove stale mempool backup: {}", error))?;
    }

    fs::rename(destination_path, &backup_path)
        .map_err(|error| format!("failed to preserve previous mempool file: {}", error))?;

    if let Err(error) = fs::rename(temporary_path, destination_path) {
        let restore_result = fs::rename(&backup_path, destination_path);

        return match restore_result {
            Ok(_) => Err(format!(
                "failed to replace mempool file; previous file was restored: {}",
                error
            )),

            Err(restore_error) => Err(format!(
                "failed to replace mempool file and failed to restore backup: replace error: {}; restore error: {}",
                error, restore_error
            )),
        };
    }

    if let Err(error) = fs::remove_file(&backup_path) {
        eprintln!(
            "warning: failed to remove mempool backup {}: {}",
            backup_path.display(),
            error
        );
    }

    Ok(())
}

fn temporary_path(destination_path: &Path) -> Result<PathBuf, String> {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|error| {
            format!(
                "system clock error while creating mempool temporary file: {}",
                error
            )
        })?
        .as_nanos();

    let file_name = destination_path
        .file_name()
        .ok_or_else(|| "mempool storage path has no file name".to_string())?
        .to_string_lossy();

    Ok(destination_path.with_file_name(format!(
        "{}.tmp-{}-{}",
        file_name,
        std::process::id(),
        unique
    )))
}

fn backup_path(destination_path: &Path) -> PathBuf {
    let file_name = destination_path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "topazio-mempool".to_string());

    destination_path.with_file_name(format!("{}.bak", file_name))
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::{
        consensus::{COINBASE_MATURITY_BLOCKS, INITIAL_BLOCK_REWARD},
        identity::KeyPair,
        transaction::Transaction,
    };

    fn alice_key() -> KeyPair {
        KeyPair::from_secret_bytes([7u8; 32])
    }

    fn bob_key() -> KeyPair {
        KeyPair::from_secret_bytes([9u8; 32])
    }

    fn carol_key() -> KeyPair {
        KeyPair::from_secret_bytes([11u8; 32])
    }

    fn funded_state() -> State {
        let alice = alice_key();

        let mut state = State::new();

        state.set_balance_for_lab(alice.address(), 100);

        state
    }

    fn immature_reward_state() -> State {
        let alice = alice_key();
        let mut state = State::new();
        state
            .apply_transaction(&Transaction::coinbase(alice.address(), 100, 1).unwrap())
            .unwrap();
        state
    }

    fn temporary_test_path(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();

        std::env::temp_dir().join(format!(
            "topazio-mempool-{}-{}-{}.dat",
            name,
            std::process::id(),
            unique
        ))
    }

    #[test]
    fn new_mempool_is_empty() {
        let mempool = Mempool::new();

        assert!(mempool.is_empty());

        assert_eq!(mempool.len(), 0);
    }

    #[test]
    fn valid_transaction_can_enter_mempool() {
        let alice = alice_key();

        let bob = bob_key();

        let state = funded_state();

        let transaction = Transaction::signed(&alice, bob.address(), 25, 0, 0).unwrap();

        let mut mempool = Mempool::new();

        mempool
            .add_transaction(transaction.clone(), &state)
            .unwrap();

        assert_eq!(mempool.len(), 1);

        assert_eq!(mempool.transactions(), &[transaction]);
    }

    #[test]
    fn mempool_rejects_transaction_funded_only_by_immature_coinbase() {
        let alice = alice_key();
        let bob = bob_key();
        let state = immature_reward_state();
        let transaction = Transaction::signed(&alice, bob.address(), 25, 0, 0).unwrap();
        let mut mempool = Mempool::new();

        assert!(mempool.add_transaction(transaction, &state).is_err());
        assert!(mempool.is_empty());
    }

    #[test]
    fn mempool_accepts_transaction_after_coinbase_maturity() {
        let alice = alice_key();
        let bob = bob_key();
        let mut state = immature_reward_state();
        state
            .advance_to_height(1 + COINBASE_MATURITY_BLOCKS)
            .unwrap();
        let transaction = Transaction::signed(&alice, bob.address(), 25, 0, 0).unwrap();
        let mut mempool = Mempool::new();

        mempool.add_transaction(transaction, &state).unwrap();
        assert_eq!(mempool.len(), 1);
    }

    #[test]
    fn fee_is_validated_and_preserved_in_mempool() {
        let alice = alice_key();
        let bob = bob_key();
        let state = funded_state();
        let transaction = Transaction::signed(&alice, bob.address(), 25, 7, 0).unwrap();
        let mut mempool = Mempool::new();

        mempool
            .add_transaction(transaction.clone(), &state)
            .unwrap();

        assert_eq!(mempool.transactions()[0].fee, 7);
        assert_eq!(mempool.transactions()[0], transaction);
    }

    #[test]
    fn mempool_rejects_balance_insufficient_for_amount_plus_fee() {
        let alice = alice_key();
        let bob = bob_key();
        let mut state = State::new();
        state.set_balance_for_lab(alice.address(), 25);
        let transaction = Transaction::signed(&alice, bob.address(), 25, 1, 0).unwrap();
        let mut mempool = Mempool::new();

        assert!(mempool.add_transaction(transaction, &state).is_err());
        assert!(mempool.is_empty());
    }

    #[test]
    fn block_selection_prioritizes_absolute_fee_deterministically() {
        let alice = alice_key();
        let bob = bob_key();
        let carol = carol_key();
        let mut state = funded_state();
        state.set_balance_for_lab(bob.address(), 100);

        let low_fee = Transaction::signed(&alice, carol.address(), 10, 1, 0).unwrap();
        let high_fee = Transaction::signed(&bob, carol.address(), 10, 9, 0).unwrap();
        let mut mempool = Mempool::new();
        mempool.add_transaction(low_fee, &state).unwrap();
        mempool.add_transaction(high_fee.clone(), &state).unwrap();

        let coinbase = Transaction::coinbase(alice.address(), INITIAL_BLOCK_REWARD, 1).unwrap();
        let selected = mempool.transactions_for_block(coinbase, &state).unwrap();

        assert_eq!(selected[1], high_fee);
        assert_eq!(selected[1].fee, 9);
    }

    #[test]
    fn duplicate_transaction_is_rejected() {
        let alice = alice_key();

        let bob = bob_key();

        let state = funded_state();

        let transaction = Transaction::signed(&alice, bob.address(), 25, 0, 0).unwrap();

        let mut mempool = Mempool::new();

        mempool
            .add_transaction(transaction.clone(), &state)
            .unwrap();

        assert!(mempool.add_transaction(transaction, &state,).is_err());

        assert_eq!(mempool.len(), 1);
    }

    #[test]
    fn coinbase_cannot_enter_mempool() {
        let alice = alice_key();

        let state = State::new();

        let coinbase = Transaction::coinbase(alice.address(), INITIAL_BLOCK_REWARD, 1).unwrap();

        let mut mempool = Mempool::new();

        assert!(mempool.add_transaction(coinbase, &state,).is_err());

        assert!(mempool.is_empty());
    }

    #[test]
    fn future_nonce_is_rejected() {
        let alice = alice_key();

        let bob = bob_key();

        let state = funded_state();

        let transaction = Transaction::signed(&alice, bob.address(), 25, 0, 1).unwrap();

        let mut mempool = Mempool::new();

        assert!(mempool.add_transaction(transaction, &state,).is_err());

        assert!(mempool.is_empty());
    }

    #[test]
    fn sequential_pending_transactions_are_accepted() {
        let alice = alice_key();

        let bob = bob_key();

        let carol = carol_key();

        let state = funded_state();

        let transaction_1 = Transaction::signed(&alice, bob.address(), 25, 0, 0).unwrap();

        let transaction_2 = Transaction::signed(&alice, carol.address(), 30, 0, 1).unwrap();

        let mut mempool = Mempool::new();

        mempool.add_transaction(transaction_1, &state).unwrap();

        mempool.add_transaction(transaction_2, &state).unwrap();

        assert_eq!(mempool.len(), 2);
    }

    #[test]
    fn pending_transactions_cannot_overspend_balance() {
        let alice = alice_key();

        let bob = bob_key();

        let carol = carol_key();

        let state = funded_state();

        let transaction_1 = Transaction::signed(&alice, bob.address(), 80, 0, 0).unwrap();

        let transaction_2 = Transaction::signed(&alice, carol.address(), 30, 0, 1).unwrap();

        let mut mempool = Mempool::new();

        mempool.add_transaction(transaction_1, &state).unwrap();

        assert!(mempool.add_transaction(transaction_2, &state,).is_err());

        assert_eq!(mempool.len(), 1);
    }

    #[test]
    fn mempool_can_be_cleared() {
        let alice = alice_key();

        let bob = bob_key();

        let state = funded_state();

        let transaction = Transaction::signed(&alice, bob.address(), 25, 0, 0).unwrap();

        let mut mempool = Mempool::new();

        mempool.add_transaction(transaction, &state).unwrap();

        assert_eq!(mempool.len(), 1);

        mempool.clear();

        assert!(mempool.is_empty());
    }

    #[test]
    fn block_transactions_place_coinbase_first() {
        let alice = alice_key();

        let bob = bob_key();

        let state = funded_state();

        let transfer = Transaction::signed(&alice, bob.address(), 25, 0, 0).unwrap();

        let mut mempool = Mempool::new();

        mempool.add_transaction(transfer.clone(), &state).unwrap();

        let coinbase = Transaction::coinbase(alice.address(), INITIAL_BLOCK_REWARD, 1).unwrap();

        let block_transactions = mempool.transactions_for_block(coinbase, &state).unwrap();

        assert_eq!(block_transactions.len(), 2);

        assert!(block_transactions[0].is_coinbase());

        assert_eq!(block_transactions[1], transfer);
    }

    #[test]
    fn block_transactions_include_sequential_dependencies() {
        let alice = alice_key();

        let bob = bob_key();

        let carol = carol_key();

        let state = funded_state();

        let first = Transaction::signed(&alice, bob.address(), 25, 0, 0).unwrap();

        let second = Transaction::signed(&alice, carol.address(), 25, 0, 1).unwrap();

        let mut mempool = Mempool::new();

        mempool.add_transaction(first.clone(), &state).unwrap();

        mempool.add_transaction(second.clone(), &state).unwrap();

        let coinbase = Transaction::coinbase(alice.address(), INITIAL_BLOCK_REWARD, 1).unwrap();

        let block_transactions = mempool.transactions_for_block(coinbase, &state).unwrap();

        assert_eq!(block_transactions.len(), 3);

        assert_eq!(block_transactions[1], first);

        assert_eq!(block_transactions[2], second);
    }

    #[test]
    fn block_transactions_skip_invalid_pending_transactions() {
        let alice = alice_key();

        let bob = bob_key();

        let carol = carol_key();

        let state = funded_state();

        let mut tampered = Transaction::signed(&alice, bob.address(), 25, 0, 0).unwrap();

        tampered.amount = 30;

        let valid = Transaction::signed(&alice, carol.address(), 10, 0, 0).unwrap();

        let mempool = Mempool {
            transactions: vec![tampered, valid.clone()],
            max_transactions: MAX_MEMPOOL_TRANSACTIONS,
        };

        let coinbase = Transaction::coinbase(alice.address(), INITIAL_BLOCK_REWARD, 1).unwrap();

        let block_transactions = mempool.transactions_for_block(coinbase, &state).unwrap();

        assert_eq!(block_transactions.len(), 2);

        assert_eq!(block_transactions[1], valid);
    }

    #[test]
    fn block_transactions_prevent_pending_double_spends() {
        let alice = alice_key();

        let bob = bob_key();

        let carol = carol_key();

        let state = funded_state();

        let first = Transaction::signed(&alice, bob.address(), 60, 0, 0).unwrap();

        let double_spend = Transaction::signed(&alice, carol.address(), 50, 0, 0).unwrap();

        let mempool = Mempool {
            transactions: vec![first.clone(), double_spend],
            max_transactions: MAX_MEMPOOL_TRANSACTIONS,
        };

        let coinbase = Transaction::coinbase(alice.address(), INITIAL_BLOCK_REWARD, 1).unwrap();

        let block_transactions = mempool.transactions_for_block(coinbase, &state).unwrap();

        assert_eq!(block_transactions.len(), 2);

        assert_eq!(block_transactions[1], first);
    }

    #[test]
    fn block_transactions_respect_transaction_count_limit() {
        let alice = alice_key();

        let bob = bob_key();

        let mut state = State::new();

        state.set_balance_for_lab(alice.address(), 1_000);

        let mut pending = Vec::new();

        for nonce in 0..crate::transaction::MAX_TRANSACTIONS_PER_BLOCK as u64 {
            pending.push(Transaction::signed(&alice, bob.address(), 1, 0, nonce).unwrap());
        }

        let mempool = Mempool {
            transactions: pending,
            max_transactions: MAX_MEMPOOL_TRANSACTIONS,
        };

        let coinbase = Transaction::coinbase(alice.address(), INITIAL_BLOCK_REWARD, 1).unwrap();

        let block_transactions = mempool.transactions_for_block(coinbase, &state).unwrap();

        assert_eq!(
            block_transactions.len(),
            crate::transaction::MAX_TRANSACTIONS_PER_BLOCK
        );

        assert_eq!(
            block_transactions.last().unwrap().nonce,
            crate::transaction::MAX_TRANSACTIONS_PER_BLOCK as u64 - 2
        );
    }

    #[test]
    fn confirmed_transactions_are_removed_from_mempool() {
        let alice = alice_key();

        let bob = bob_key();

        let state = funded_state();

        let transfer = Transaction::signed(&alice, bob.address(), 25, 0, 0).unwrap();

        let mut mempool = Mempool::new();

        mempool.add_transaction(transfer.clone(), &state).unwrap();

        let coinbase = Transaction::coinbase(alice.address(), INITIAL_BLOCK_REWARD, 1).unwrap();

        let confirmed = vec![coinbase, transfer];

        mempool.remove_confirmed(&confirmed);

        assert!(mempool.is_empty());
    }

    #[test]
    fn revalidated_mempool_preserves_only_transactions_valid_for_current_state() {
        let alice = alice_key();

        let bob = bob_key();

        let carol = carol_key();

        let state = funded_state();

        let stale = Transaction::signed(&alice, bob.address(), 70, 0, 0).unwrap();

        let valid = Transaction::signed(&alice, carol.address(), 10, 0, 0).unwrap();

        let mempool = Mempool {
            transactions: vec![stale, valid.clone()],
            max_transactions: MAX_MEMPOOL_TRANSACTIONS,
        };

        let mut changed_state = State::new();

        changed_state.set_balance_for_lab(alice.address(), 50);

        let revalidated = mempool.revalidated(&changed_state).unwrap();

        assert_eq!(revalidated.transactions(), &[valid]);

        assert!(mempool.transactions()[0].validate().is_ok());

        assert_eq!(state.balance_of(&alice.address()), 100);
    }

    #[test]
    fn mempool_can_be_saved_and_loaded() {
        let path = temporary_test_path("save-load");

        let alice = alice_key();

        let bob = bob_key();

        let state = funded_state();

        let transaction = Transaction::signed(&alice, bob.address(), 25, 0, 0).unwrap();

        let mut mempool = Mempool::new();

        mempool
            .add_transaction(transaction.clone(), &state)
            .unwrap();

        mempool.save(&path).unwrap();

        let loaded = Mempool::load(&path, &state).unwrap();

        assert_eq!(loaded.len(), 1);

        assert_eq!(loaded.transactions(), &[transaction]);

        let _ = fs::remove_file(path);
    }

    #[test]
    fn empty_mempool_can_be_saved_and_loaded() {
        let path = temporary_test_path("empty");

        let state = State::new();

        let mempool = Mempool::new();

        mempool.save(&path).unwrap();

        let loaded = Mempool::load(&path, &state).unwrap();

        assert!(loaded.is_empty());

        let _ = fs::remove_file(path);
    }

    #[test]
    fn invalid_mempool_magic_is_rejected() {
        let path = temporary_test_path("bad-magic");

        let mut bytes = Vec::new();

        bytes.extend_from_slice(b"BADMEM01");

        bytes.extend_from_slice(&MEMPOOL_VERSION.to_le_bytes());

        bytes.extend_from_slice(&encode_transactions(&[]));

        fs::write(&path, bytes).unwrap();

        assert!(Mempool::load(&path, &State::new(),).is_err());

        let _ = fs::remove_file(path);
    }

    #[test]
    fn legacy_mempool_version_is_rejected() {
        let path = temporary_test_path("legacy-version");
        let mut bytes = Vec::new();
        bytes.extend_from_slice(MEMPOOL_MAGIC);
        bytes.extend_from_slice(&1u16.to_le_bytes());
        bytes.extend_from_slice(&encode_transactions(&[]));
        fs::write(&path, bytes).unwrap();

        assert!(Mempool::load(&path, &State::new()).is_err());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn stored_transaction_is_revalidated_against_current_state() {
        let path = temporary_test_path("revalidate");

        let alice = alice_key();

        let bob = bob_key();

        let original_state = funded_state();

        let transaction = Transaction::signed(&alice, bob.address(), 75, 0, 0).unwrap();

        let mut mempool = Mempool::new();

        mempool
            .add_transaction(transaction, &original_state)
            .unwrap();

        mempool.save(&path).unwrap();

        /*
         * Agora simulamos uma blockchain
         * confirmada diferente, onde Alice
         * possui somente a recompensa inicial em unidades atômicas.
         */
        let mut changed_state = State::new();

        changed_state.set_balance_for_lab(alice.address(), 50);

        assert!(Mempool::load(&path, &changed_state,).is_err());

        let _ = fs::remove_file(path);
    }
}

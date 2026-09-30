use std::{
    io::{BufRead, BufReader, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
    time::Duration,
};

use serde_json::{Value, json};

use crate::{
    NETWORK_ID, NETWORK_NAME, NETWORK_SYMBOL, NETWORK_VERSION,
    block::Block,
    blockchain::Blockchain,
    consensus::block_reward,
    crypto::hash_to_hex,
    mempool::Mempool,
    transaction::{Transaction, sum_fees},
};

pub const DEFAULT_RPC_MAX_REQUEST_BYTES: usize = 1024 * 1024;

#[derive(Clone)]
pub struct RpcState {
    inner: Arc<Mutex<RpcData>>,
    peers: Arc<AtomicUsize>,
}

struct RpcData {
    blockchain: Blockchain,
    mempool: Mempool,
    chain_path: PathBuf,
    mempool_path: PathBuf,
}

impl RpcState {
    pub fn new(
        blockchain: Blockchain,
        mempool: Mempool,
        chain_path: impl Into<PathBuf>,
        mempool_path: impl Into<PathBuf>,
    ) -> Self {
        Self {
            inner: Arc::new(Mutex::new(RpcData {
                blockchain,
                mempool,
                chain_path: chain_path.into(),
                mempool_path: mempool_path.into(),
            })),
            peers: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub fn blockchain_snapshot(&self) -> Blockchain {
        self.inner
            .lock()
            .expect("RPC state mutex poisoned")
            .blockchain
            .clone()
    }

    pub fn mempool_snapshot(&self) -> Mempool {
        self.inner
            .lock()
            .expect("RPC state mutex poisoned")
            .mempool
            .clone()
    }

    pub fn set_peer_count(&self, count: usize) {
        self.peers.store(count, Ordering::Relaxed);
    }

    pub fn peer_count(&self) -> usize {
        self.peers.load(Ordering::Relaxed)
    }

    pub fn import_blocks(&self, blocks: Vec<Block>) -> Result<usize, String> {
        if blocks.is_empty() {
            return Ok(0);
        }

        let mut data = self
            .inner
            .lock()
            .map_err(|_| "RPC state is unavailable".to_string())?;
        validate_contiguous_blocks(&blocks)?;

        let current = data.blockchain.clone();

        if blocks_are_already_known(&current, &blocks) {
            return Ok(0);
        }

        let first_height = blocks[0].height;
        let received_count = blocks.len();

        if first_height > current.next_height() {
            return Err(format!(
                "received block batch starts beyond the next local height: {}",
                first_height
            ));
        }

        let fork_start = usize::try_from(first_height)
            .map_err(|_| "received block height is too large".to_string())?;
        let mut orphaned = Vec::new();
        let mut candidate = current.clone();

        if first_height == current.next_height() {
            for block in blocks {
                candidate.append_block(block)?;
            }
        } else {
            if fork_start > current.blocks().len() {
                return Err("received fork starts beyond local chain".to_string());
            }

            if first_height > 0 {
                let previous_index = fork_start
                    .checked_sub(1)
                    .ok_or_else(|| "fork start underflow".to_string())?;
                let previous = current
                    .blocks()
                    .get(previous_index)
                    .ok_or_else(|| "received fork has no local anchor".to_string())?;

                if blocks[0].previous_hash != previous.hash() {
                    return Err(format!(
                        "received fork does not connect at height {}",
                        first_height
                    ));
                }
            }

            let mut candidate_blocks = current.blocks()[..fork_start].to_vec();
            candidate_blocks.extend(blocks);
            let rebuilt = Blockchain::from_blocks(current.difficulty_bits(), candidate_blocks)
                .map_err(|error| {
                    format!("received fork failed blockchain validation: {}", error)
                })?;

            if !rebuilt.is_better_chain_than(&current) {
                return Ok(0);
            }

            let orphan_start = fork_start.max(1);
            orphaned = non_coinbase_transactions(&current.blocks()[orphan_start..])?;
            candidate = rebuilt;
        }

        let confirmed = non_coinbase_transactions(&candidate.blocks()[1..])?;
        crate::storage::save_blockchain(&candidate, &data.chain_path)?;
        let mut mempool = data.mempool.clone();
        mempool.remove_confirmed(&confirmed);
        let pending = mempool.transactions().to_vec();
        mempool.clear();

        // Reinsert orphaned transactions first so pending dependants remain
        // valid when their predecessor is restored by the reorg.
        for transaction in orphaned.into_iter().chain(pending) {
            if !mempool.contains_hash(&transaction.hash()) {
                let _ = mempool.add_transaction(transaction, candidate.state());
            }
        }
        mempool.save(&data.mempool_path)?;
        let imported = if candidate.len() > current.len() {
            candidate.len() - current.len()
        } else {
            // A same-height (or shorter, higher-work) reorg still changed
            // the active tip and must trigger peer convergence.
            received_count
        };
        data.blockchain = candidate;
        data.mempool = mempool;
        Ok(imported)
    }

    pub fn handle_json(&self, request: Value) -> Value {
        let method = request.get("method").and_then(Value::as_str).unwrap_or("");

        match self.handle_method(method, &request) {
            Ok(result) => json!({"ok": true, "result": result}),
            Err(error) => json!({"ok": false, "error": error}),
        }
    }

    fn handle_method(&self, method: &str, request: &Value) -> Result<Value, String> {
        match method {
            "status" => {
                let data = self
                    .inner
                    .lock()
                    .map_err(|_| "RPC state is unavailable".to_string())?;
                Ok(json!({
                    "network": NETWORK_ID,
                    "version": NETWORK_VERSION,
                    "height": data.blockchain.tip().height,
                    "tip": hash_to_hex(&data.blockchain.tip().hash()),
                    "supply": data.blockchain.total_supply()?,
                    "mempool": data.mempool.len(),
                    "peers": self.peer_count(),
                }))
            }
            "network-info" => Ok(json!({
                "name": NETWORK_NAME,
                "symbol": NETWORK_SYMBOL,
                "network": NETWORK_ID,
                "version": NETWORK_VERSION,
                "genesis": hash_to_hex(&crate::genesis::genesis_hash()),
            })),
            "height" => {
                let data = self
                    .inner
                    .lock()
                    .map_err(|_| "RPC state is unavailable".to_string())?;
                Ok(json!({"height": data.blockchain.tip().height}))
            }
            "tip" => {
                let data = self
                    .inner
                    .lock()
                    .map_err(|_| "RPC state is unavailable".to_string())?;
                Ok(json!({
                    "height": data.blockchain.tip().height,
                    "hash": hash_to_hex(&data.blockchain.tip().hash()),
                    "timestamp": data.blockchain.tip().timestamp,
                    "work": data.blockchain.cumulative_work().to_string(),
                }))
            }
            "balance" | "nonce" => {
                let address = required_string(request, "address")?;
                let data = self
                    .inner
                    .lock()
                    .map_err(|_| "RPC state is unavailable".to_string())?;
                if method == "balance" {
                    let state = data.blockchain.state();
                    let total_balance = state.try_total_balance_of(&address)?;
                    Ok(json!({
                        "address": address,
                        "balance": total_balance,
                        "total_balance": total_balance,
                        "spendable_balance": state.spendable_balance_of(&address),
                        "immature_balance": state.immature_balance_of(&address),
                        "current_height": state.current_height(),
                    }))
                } else {
                    Ok(
                        json!({"address": address, "nonce": data.blockchain.state().nonce_of(&address)}),
                    )
                }
            }
            "peers" => Ok(json!({"count": self.peer_count()})),
            "mempool" => {
                let data = self
                    .inner
                    .lock()
                    .map_err(|_| "RPC state is unavailable".to_string())?;
                let hashes: Vec<String> = data
                    .mempool
                    .transactions()
                    .iter()
                    .map(|tx| hash_to_hex(&tx.hash()))
                    .collect();
                let details: Vec<Value> = data
                    .mempool
                    .transactions()
                    .iter()
                    .map(|transaction| {
                        json!({
                            "hash": hash_to_hex(&transaction.hash()),
                            "from": transaction.from,
                            "to": transaction.to,
                            "amount": transaction.amount,
                            "fee": transaction.fee,
                            "nonce": transaction.nonce,
                        })
                    })
                    .collect();
                Ok(json!({"count": hashes.len(), "transactions": hashes, "details": details}))
            }
            "mining-info" => {
                let data = self
                    .inner
                    .lock()
                    .map_err(|_| "RPC state is unavailable".to_string())?;
                let height = data.blockchain.next_height();
                let subsidy = block_reward(height);
                let pending_transactions =
                    if let Some(address) = request.get("address").and_then(Value::as_str) {
                        // Selection is made against the subsidy-only coinbase.
                        // The final coinbase value is subsidy plus the fees of
                        // the transactions that were actually selected.
                        let coinbase = Transaction::coinbase(address.to_string(), subsidy, height)?;
                        data.mempool
                            .transactions_for_block(coinbase, data.blockchain.state())?
                            .into_iter()
                            .skip(1)
                            .map(|transaction| hex::encode(transaction.encode()))
                            .collect::<Vec<String>>()
                    } else {
                        Vec::new()
                    };
                let transaction_count = pending_transactions.len();
                let selected_transactions: Vec<Transaction> = pending_transactions
                    .iter()
                    .map(|encoded| {
                        Transaction::decode(
                            &hex::decode(encoded)
                                .map_err(|_| "mining transaction is not valid hex".to_string())?,
                        )
                    })
                    .collect::<Result<_, _>>()?;
                let fees = sum_fees(&selected_transactions)?;
                let coinbase_value = subsidy
                    .checked_add(fees)
                    .ok_or_else(|| "subsidy plus fees overflow".to_string())?;

                Ok(json!({
                    "height": height,
                    "previous_hash": hash_to_hex(&data.blockchain.tip().hash()),
                    "previous_timestamp": data.blockchain.tip().timestamp,
                    "difficulty_bits": data.blockchain.next_difficulty_bits(),
                    "reward": coinbase_value,
                    "subsidy": subsidy,
                    "fees": fees,
                    "coinbase_value": coinbase_value,
                    "transactions": pending_transactions,
                    "transaction_count": transaction_count,
                }))
            }
            "transaction" => self.transaction_info(required_string(request, "hash")?),
            "submit_transaction" => {
                self.submit_transaction(required_string(request, "transaction")?)
            }
            "submit_block" => self.submit_block(required_string(request, "block")?),
            _ => Err(format!("unknown RPC method: {}", method)),
        }
    }

    fn transaction_info(&self, hash_text: String) -> Result<Value, String> {
        let wanted = decode_hash(&hash_text)?;
        let data = self
            .inner
            .lock()
            .map_err(|_| "RPC state is unavailable".to_string())?;

        if let Some(transaction) = data.mempool.transaction_by_hash(&wanted) {
            return Ok(json!({
                "hash": hash_text,
                "status": "mempool",
                "transaction": hex::encode(transaction.encode()),
                "from": transaction.from,
                "to": transaction.to,
                "amount": transaction.amount,
                "fee": transaction.fee,
                "nonce": transaction.nonce,
            }));
        }

        for block in data.blockchain.blocks().iter().skip(1) {
            let transactions = block.transactions()?;
            for transaction in transactions {
                if transaction.hash() == wanted {
                    return Ok(json!({
                        "hash": hash_text,
                        "status": "confirmed",
                        "height": block.height,
                        "block": hash_to_hex(&block.hash()),
                        "transaction": hex::encode(transaction.encode()),
                        "from": transaction.from,
                        "to": transaction.to,
                        "amount": transaction.amount,
                        "fee": transaction.fee,
                        "nonce": transaction.nonce,
                    }));
                }
            }
        }

        Ok(json!({"hash": hash_text, "status": "unknown"}))
    }

    fn submit_transaction(&self, encoded: String) -> Result<Value, String> {
        let bytes =
            hex::decode(&encoded).map_err(|_| "transaction is not valid hex".to_string())?;
        let transaction = Transaction::decode(&bytes)?;
        let hash = transaction.hash();
        let hash_text = hash_to_hex(&hash);
        let amount = transaction.amount;
        let fee = transaction.fee;

        let mut data = self
            .inner
            .lock()
            .map_err(|_| "RPC state is unavailable".to_string())?;

        if data.mempool.contains_hash(&hash) {
            return Ok(
                json!({"accepted": true, "transaction_hash": hash_text, "reason": "already in mempool", "amount": amount, "fee": fee}),
            );
        }

        let mut candidate = data.mempool.clone();
        candidate.add_transaction(transaction, data.blockchain.state())?;
        candidate.save(&data.mempool_path)?;
        data.mempool = candidate;

        Ok(json!({
            "accepted": true,
            "transaction_hash": hash_text,
            "reason": "accepted into mempool",
            "mempool_size": data.mempool.len(),
            "amount": amount,
            "fee": fee,
        }))
    }

    fn submit_block(&self, encoded: String) -> Result<Value, String> {
        let bytes = hex::decode(&encoded).map_err(|_| "block is not valid hex".to_string())?;
        let block = Block::decode(&bytes)?;
        let height = block.height;
        let hash = hash_to_hex(&block.hash());
        let transactions = block.transactions()?;

        let mut data = self
            .inner
            .lock()
            .map_err(|_| "RPC state is unavailable".to_string())?;
        let mut candidate = data.blockchain.clone();
        candidate.append_block(block)?;
        crate::storage::save_blockchain(&candidate, &data.chain_path)?;
        let mut mempool = data.mempool.clone();
        mempool.remove_confirmed(&transactions);
        mempool = mempool.revalidated(candidate.state())?;
        mempool.save(&data.mempool_path)?;
        data.blockchain = candidate;
        data.mempool = mempool;

        Ok(json!({"accepted": true, "height": height, "block_hash": hash}))
    }
}

pub fn serve(bind: &str, state: RpcState, max_request_bytes: usize) -> Result<(), String> {
    let listener = TcpListener::bind(bind)
        .map_err(|error| format!("failed to bind RPC listener {}: {}", bind, error))?;

    for incoming in listener.incoming() {
        match incoming {
            Ok(mut stream) => {
                let state = state.clone();
                std::thread::spawn(move || {
                    if let Err(error) = handle_connection(&mut stream, &state, max_request_bytes) {
                        eprintln!("RPC connection closed: {}", error);
                    }
                });
            }
            Err(error) => eprintln!("RPC accept error: {}", error),
        }
    }

    Ok(())
}

pub fn request(bind: &str, request: Value, max_response_bytes: usize) -> Result<Value, String> {
    let mut stream = TcpStream::connect(bind)
        .map_err(|error| format!("failed to connect to RPC {}: {}", bind, error))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(|error| format!("failed to configure RPC timeout: {}", error))?;
    stream
        .set_write_timeout(Some(Duration::from_secs(10)))
        .map_err(|error| format!("failed to configure RPC timeout: {}", error))?;

    let encoded = serde_json::to_vec(&request)
        .map_err(|error| format!("failed to encode RPC request: {}", error))?;
    stream
        .write_all(&encoded)
        .map_err(|error| format!("failed to write RPC request: {}", error))?;
    stream
        .write_all(b"\n")
        .map_err(|error| format!("failed to finish RPC request: {}", error))?;
    stream
        .flush()
        .map_err(|error| format!("failed to flush RPC request: {}", error))?;

    let mut response = Vec::new();
    BufReader::new(stream)
        .read_until(b'\n', &mut response)
        .map_err(|error| format!("failed to read RPC response: {}", error))?;

    if response.len() > max_response_bytes {
        return Err("RPC response exceeds the configured limit".to_string());
    }

    serde_json::from_slice(&response).map_err(|error| format!("invalid RPC response: {}", error))
}

fn handle_connection(
    stream: &mut TcpStream,
    state: &RpcState,
    max_request_bytes: usize,
) -> Result<(), String> {
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .map_err(|error| format!("failed to configure RPC timeout: {}", error))?;
    stream
        .set_write_timeout(Some(Duration::from_secs(10)))
        .map_err(|error| format!("failed to configure RPC timeout: {}", error))?;

    let mut request = Vec::new();
    BufReader::new(
        stream
            .try_clone()
            .map_err(|error| format!("failed to clone RPC stream: {}", error))?,
    )
    .read_until(b'\n', &mut request)
    .map_err(|error| format!("failed to read RPC request: {}", error))?;

    if request.len() > max_request_bytes || request.len() > DEFAULT_RPC_MAX_REQUEST_BYTES * 16 {
        return Err("RPC request exceeds the configured limit".to_string());
    }

    let parsed: Value = serde_json::from_slice(&request)
        .map_err(|error| format!("invalid RPC JSON request: {}", error))?;
    let response = state.handle_json(parsed);
    let mut bytes = serde_json::to_vec(&response)
        .map_err(|error| format!("failed to encode RPC response: {}", error))?;
    bytes.push(b'\n');
    stream
        .write_all(&bytes)
        .map_err(|error| format!("failed to write RPC response: {}", error))?;
    stream
        .flush()
        .map_err(|error| format!("failed to flush RPC response: {}", error))?;
    Ok(())
}

fn required_string(request: &Value, field: &str) -> Result<String, String> {
    request
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| format!("RPC field '{}' is required", field))
}

fn decode_hash(value: &str) -> Result<[u8; 32], String> {
    let bytes = hex::decode(value).map_err(|_| "hash is not valid hex".to_string())?;
    if bytes.len() != 32 {
        return Err("hash must contain exactly 32 bytes".to_string());
    }
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&bytes);
    Ok(hash)
}

fn validate_contiguous_blocks(blocks: &[Block]) -> Result<(), String> {
    for window in blocks.windows(2) {
        let previous = &window[0];
        let current = &window[1];
        let expected = previous
            .height
            .checked_add(1)
            .ok_or_else(|| "received block height overflow".to_string())?;

        if current.height != expected {
            return Err(format!(
                "received block batch is not contiguous: expected {}, received {}",
                expected, current.height
            ));
        }

        if current.previous_hash != previous.hash() {
            return Err(format!(
                "received block batch has a broken link at height {}",
                current.height
            ));
        }
    }

    Ok(())
}

fn blocks_are_already_known(blockchain: &Blockchain, blocks: &[Block]) -> bool {
    blocks.iter().all(|block| {
        usize::try_from(block.height)
            .ok()
            .and_then(|index| blockchain.blocks().get(index))
            .map(|known| known.hash() == block.hash())
            .unwrap_or(false)
    })
}

fn non_coinbase_transactions(blocks: &[Block]) -> Result<Vec<Transaction>, String> {
    let mut transactions = Vec::new();

    for block in blocks {
        transactions.extend(
            block
                .transactions()?
                .into_iter()
                .filter(|transaction| !transaction.is_coinbase()),
        );
    }

    Ok(transactions)
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    use crate::{
        block::Block,
        consensus::{COINBASE_MATURITY_BLOCKS, INITIAL_BLOCK_REWARD},
        identity::KeyPair,
        mempool::Mempool,
        mining::mine_block,
        storage::load_blockchain,
        transaction::Transaction,
    };

    fn temporary_path(name: &str) -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();

        std::env::temp_dir().join(format!(
            "topazio-rpc-{}-{}-{}",
            name,
            std::process::id(),
            unique
        ))
    }

    fn mine_next_block(blockchain: &Blockchain, transactions: &[Transaction]) -> Block {
        let difficulty = blockchain.next_difficulty_bits();

        let candidate = Block::with_transactions_at_difficulty(
            blockchain.next_height(),
            blockchain.tip().hash(),
            blockchain.tip().timestamp + 1,
            0,
            difficulty,
            transactions,
        )
        .unwrap();

        mine_block(candidate, difficulty, 2_000_000).unwrap()
    }

    fn mature_first_coinbase(chain: &mut Blockchain, miner: &KeyPair) {
        while chain.tip().height <= COINBASE_MATURITY_BLOCKS {
            let height = chain.next_height();
            let coinbase = Transaction::coinbase(miner.address(), 0, height).unwrap();
            let block = mine_next_block(chain, &[coinbase]);
            chain.append_block(block).unwrap();
        }
    }

    #[test]
    fn mining_info_templates_pending_transactions_and_submit_block_confirms_them() {
        let base = temporary_path("mining-template");
        let chain_path = base.join("chain.dat");
        let mempool_path = base.join("mempool.dat");
        fs::create_dir_all(&base).unwrap();

        let alice = KeyPair::from_secret_bytes([52u8; 32]);
        let bob = KeyPair::from_secret_bytes([53u8; 32]);

        let mut blockchain = Blockchain::with_difficulty(0);
        let reward = Transaction::coinbase(
            alice.address(),
            INITIAL_BLOCK_REWARD,
            blockchain.next_height(),
        )
        .unwrap();
        let reward_block = mine_next_block(&blockchain, &[reward]);
        blockchain.append_block(reward_block).unwrap();
        mature_first_coinbase(&mut blockchain, &alice);

        let transfer = Transaction::signed(&alice, bob.address(), 7, 3, 0).unwrap();
        let transfer_hash = hash_to_hex(&transfer.hash());
        let mut mempool = Mempool::new();
        mempool
            .add_transaction(transfer.clone(), blockchain.state())
            .unwrap();

        let state = RpcState::new(blockchain, mempool, &chain_path, &mempool_path);
        let template = state.handle_json(json!({
            "method": "mining-info",
            "address": alice.address(),
        }));

        assert_eq!(template.get("ok").and_then(Value::as_bool), Some(true));

        let result = template.get("result").unwrap();
        assert_eq!(
            result.get("subsidy").and_then(Value::as_u64),
            Some(INITIAL_BLOCK_REWARD)
        );
        assert_eq!(result.get("fees").and_then(Value::as_u64), Some(3));
        assert_eq!(
            result.get("coinbase_value").and_then(Value::as_u64),
            Some(INITIAL_BLOCK_REWARD + 3)
        );
        assert_eq!(
            result.get("previous_timestamp").and_then(Value::as_u64),
            Some(state.blockchain_snapshot().tip().timestamp)
        );
        let pending = result
            .get("transactions")
            .and_then(Value::as_array)
            .unwrap();

        assert_eq!(pending.len(), 1);

        let encoded = pending[0].as_str().unwrap();
        let templated_transaction = Transaction::decode(&hex::decode(encoded).unwrap()).unwrap();

        assert_eq!(templated_transaction.hash(), transfer.hash());

        let height = result.get("height").and_then(Value::as_u64).unwrap();
        let previous_hash =
            decode_hash(result.get("previous_hash").and_then(Value::as_str).unwrap()).unwrap();
        let difficulty = result
            .get("difficulty_bits")
            .and_then(Value::as_u64)
            .unwrap() as u16;
        let reward = result.get("reward").and_then(Value::as_u64).unwrap();
        let coinbase = Transaction::coinbase(alice.address(), reward, height).unwrap();
        let transactions = vec![coinbase, templated_transaction];
        let snapshot = state.blockchain_snapshot();
        let candidate = Block::with_transactions_at_difficulty(
            height,
            previous_hash,
            snapshot.tip().timestamp + 1,
            0,
            difficulty,
            &transactions,
        )
        .unwrap();
        let mined = mine_block(candidate, difficulty, 2_000_000).unwrap();

        let submit = state.handle_json(json!({
            "method": "submit_block",
            "block": hex::encode(mined.encode()),
        }));

        assert_eq!(submit.get("ok").and_then(Value::as_bool), Some(true));

        let lookup = state.handle_json(json!({
            "method": "transaction",
            "hash": transfer_hash,
        }));

        assert_eq!(
            lookup
                .get("result")
                .and_then(|value| value.get("status"))
                .and_then(Value::as_str),
            Some("confirmed")
        );

        let mempool_status = state.handle_json(json!({"method": "mempool"}));

        assert_eq!(
            mempool_status
                .get("result")
                .and_then(|value| value.get("count"))
                .and_then(Value::as_u64),
            Some(0)
        );

        let balance = state.handle_json(json!({
            "method": "balance",
            "address": alice.address(),
        }));
        let balance_result = balance.get("result").unwrap();
        assert_eq!(
            balance_result.get("total_balance").and_then(Value::as_u64),
            Some((INITIAL_BLOCK_REWARD * 2) - 7)
        );
        assert_eq!(
            balance_result
                .get("spendable_balance")
                .and_then(Value::as_u64),
            Some(INITIAL_BLOCK_REWARD - 10)
        );
        assert_eq!(
            balance_result
                .get("immature_balance")
                .and_then(Value::as_u64),
            Some(INITIAL_BLOCK_REWARD + 3)
        );

        let restored = load_blockchain(&chain_path).unwrap();

        assert_eq!(restored.tip().height, 102);
        assert_eq!(
            restored.state().balance_of(&alice.address()),
            (INITIAL_BLOCK_REWARD * 2) - 7
        );
        assert_eq!(restored.state().balance_of(&bob.address()), 7);

        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn import_blocks_reorganizes_to_stronger_chain_and_restores_orphaned_transfers() {
        let base = temporary_path("reorg");
        let chain_path = base.join("chain.dat");
        let mempool_path = base.join("mempool.dat");
        fs::create_dir_all(&base).unwrap();

        let alice = KeyPair::from_secret_bytes([62u8; 32]);
        let bob = KeyPair::from_secret_bytes([63u8; 32]);
        let carol = KeyPair::from_secret_bytes([64u8; 32]);

        let mut common = Blockchain::with_difficulty(0);
        let common_reward =
            Transaction::coinbase(alice.address(), INITIAL_BLOCK_REWARD, common.next_height())
                .unwrap();
        let common_block = mine_next_block(&common, &[common_reward]);
        common.append_block(common_block).unwrap();
        mature_first_coinbase(&mut common, &alice);

        let mut current = common.clone();
        let orphaned_transfer = Transaction::signed(&alice, carol.address(), 5, 0, 0).unwrap();
        let current_reward =
            Transaction::coinbase(alice.address(), INITIAL_BLOCK_REWARD, current.next_height())
                .unwrap();
        let current_block = mine_next_block(&current, &[current_reward, orphaned_transfer.clone()]);
        current.append_block(current_block).unwrap();

        let mut stronger = common.clone();
        for _ in 0..2 {
            let reward =
                Transaction::coinbase(bob.address(), INITIAL_BLOCK_REWARD, stronger.next_height())
                    .unwrap();
            let block = mine_next_block(&stronger, &[reward]);
            stronger.append_block(block).unwrap();
        }

        let state = RpcState::new(current, Mempool::new(), &chain_path, &mempool_path);
        let imported = state
            .import_blocks(stronger.blocks()[102..].to_vec())
            .unwrap();

        assert_eq!(imported, 1);

        let snapshot = state.blockchain_snapshot();

        assert_eq!(snapshot.tip().height, 103);
        assert_eq!(snapshot.tip().hash(), stronger.tip().hash());
        assert_eq!(
            snapshot.state().balance_of(&alice.address()),
            INITIAL_BLOCK_REWARD
        );
        assert_eq!(
            snapshot.state().balance_of(&bob.address()),
            INITIAL_BLOCK_REWARD * 2
        );

        let mempool = state.mempool_snapshot();

        assert_eq!(mempool.len(), 1);
        assert!(mempool.contains_hash(&orphaned_transfer.hash()));

        let restored = load_blockchain(&chain_path).unwrap();

        assert_eq!(restored.tip().hash(), stronger.tip().hash());

        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn equal_work_forks_converge_to_the_same_deterministic_tip() {
        let base = temporary_path("equal-work-fork");
        fs::create_dir_all(&base).unwrap();

        let common_miner = KeyPair::from_secret_bytes([71u8; 32]);
        let miner_a = KeyPair::from_secret_bytes([72u8; 32]);
        let miner_b = KeyPair::from_secret_bytes([73u8; 32]);

        let mut common = Blockchain::with_difficulty(0);
        let common_reward = Transaction::coinbase(
            common_miner.address(),
            INITIAL_BLOCK_REWARD,
            common.next_height(),
        )
        .unwrap();
        common
            .append_block(mine_next_block(&common, &[common_reward]))
            .unwrap();

        let mut chain_a = common.clone();
        let reward_a = Transaction::coinbase(
            miner_a.address(),
            INITIAL_BLOCK_REWARD,
            chain_a.next_height(),
        )
        .unwrap();
        chain_a
            .append_block(mine_next_block(&chain_a, &[reward_a]))
            .unwrap();

        let mut chain_b = common.clone();
        let reward_b = Transaction::coinbase(
            miner_b.address(),
            INITIAL_BLOCK_REWARD,
            chain_b.next_height(),
        )
        .unwrap();
        chain_b
            .append_block(mine_next_block(&chain_b, &[reward_b]))
            .unwrap();

        assert_eq!(chain_a.cumulative_work(), chain_b.cumulative_work());
        let expected_tip = chain_a.tip().hash().min(chain_b.tip().hash());

        let state_a = RpcState::new(
            chain_a.clone(),
            Mempool::new(),
            base.join("a-chain.dat"),
            base.join("a-mempool.dat"),
        );
        let state_b = RpcState::new(
            chain_b.clone(),
            Mempool::new(),
            base.join("b-chain.dat"),
            base.join("b-mempool.dat"),
        );

        let imported_by_a = state_a
            .import_blocks(chain_b.blocks()[2..].to_vec())
            .unwrap();
        let imported_by_b = state_b
            .import_blocks(chain_a.blocks()[2..].to_vec())
            .unwrap();

        assert_eq!(imported_by_a + imported_by_b, 1);
        assert_eq!(state_a.blockchain_snapshot().tip().hash(), expected_tip);
        assert_eq!(state_b.blockchain_snapshot().tip().hash(), expected_tip);
        assert_eq!(
            state_a.blockchain_snapshot().cumulative_work(),
            state_b.blockchain_snapshot().cumulative_work()
        );

        let _ = fs::remove_dir_all(base);
    }

    #[test]
    fn stronger_reorg_requeues_dependencies_and_drops_conflicting_orphans() {
        let base = temporary_path("dependency-reorg");
        let a_chain_path = base.join("a-chain.dat");
        let a_mempool_path = base.join("a-mempool.dat");
        let b_chain_path = base.join("b-chain.dat");
        let b_mempool_path = base.join("b-mempool.dat");
        fs::create_dir_all(&base).unwrap();

        let alice = KeyPair::from_secret_bytes([81u8; 32]);
        let bob = KeyPair::from_secret_bytes([82u8; 32]);
        let carol = KeyPair::from_secret_bytes([83u8; 32]);

        let mut common = Blockchain::with_difficulty(0);
        let common_reward =
            Transaction::coinbase(alice.address(), INITIAL_BLOCK_REWARD, common.next_height())
                .unwrap();
        common
            .append_block(mine_next_block(&common, &[common_reward]))
            .unwrap();
        mature_first_coinbase(&mut common, &alice);

        let orphaned_transfer = Transaction::signed(&alice, carol.address(), 5, 2, 0).unwrap();
        let mut chain_a = common.clone();
        let a_reward = Transaction::coinbase(
            alice.address(),
            INITIAL_BLOCK_REWARD + 2,
            chain_a.next_height(),
        )
        .unwrap();
        chain_a
            .append_block(mine_next_block(
                &chain_a,
                &[a_reward, orphaned_transfer.clone()],
            ))
            .unwrap();
        let a_reward =
            Transaction::coinbase(alice.address(), INITIAL_BLOCK_REWARD, chain_a.next_height())
                .unwrap();
        chain_a
            .append_block(mine_next_block(&chain_a, &[a_reward]))
            .unwrap();

        let pending_dependent = Transaction::signed(&alice, bob.address(), 6, 0, 1).unwrap();
        let mut a_mempool = Mempool::new();
        a_mempool
            .add_transaction(pending_dependent.clone(), chain_a.state())
            .unwrap();

        let alternate_transfer = Transaction::signed(&alice, bob.address(), 3, 4, 0).unwrap();
        let mut chain_b = common.clone();
        let b_reward = Transaction::coinbase(
            bob.address(),
            INITIAL_BLOCK_REWARD + 4,
            chain_b.next_height(),
        )
        .unwrap();
        chain_b
            .append_block(mine_next_block(&chain_b, &[b_reward, alternate_transfer]))
            .unwrap();
        for _ in 0..2 {
            let reward =
                Transaction::coinbase(bob.address(), INITIAL_BLOCK_REWARD, chain_b.next_height())
                    .unwrap();
            chain_b
                .append_block(mine_next_block(&chain_b, &[reward]))
                .unwrap();
        }

        assert!(chain_b.cumulative_work() > chain_a.cumulative_work());

        let state_a = RpcState::new(chain_a, a_mempool, &a_chain_path, &a_mempool_path);
        let state_b = RpcState::new(
            chain_b.clone(),
            Mempool::new(),
            &b_chain_path,
            &b_mempool_path,
        );

        let imported = state_a
            .import_blocks(chain_b.blocks()[102..].to_vec())
            .unwrap();
        assert_eq!(imported, 1);

        let ignored = state_b
            .import_blocks(state_a.blockchain_snapshot().blocks()[102..].to_vec())
            .unwrap();
        assert_eq!(ignored, 0);

        let snapshot = state_a.blockchain_snapshot();
        assert_eq!(snapshot.tip().height, chain_b.tip().height);
        assert_eq!(snapshot.tip().hash(), chain_b.tip().hash());
        assert_eq!(snapshot.cumulative_work(), chain_b.cumulative_work());
        assert_eq!(
            snapshot.state().balance_of(&alice.address()),
            INITIAL_BLOCK_REWARD - 7
        );
        assert_eq!(
            snapshot.state().balance_of(&bob.address()),
            (INITIAL_BLOCK_REWARD * 3) + 4 + 3
        );
        assert_eq!(snapshot.state().balance_of(&carol.address()), 0);

        let mempool = state_a.mempool_snapshot();
        assert_eq!(mempool.len(), 1);
        assert!(mempool.contains_hash(&pending_dependent.hash()));
        assert!(!mempool.contains_hash(&orphaned_transfer.hash()));
        assert_eq!(mempool.transactions()[0].nonce, 1);

        let restored_chain = load_blockchain(&a_chain_path).unwrap();
        let restored_mempool = Mempool::load(&a_mempool_path, restored_chain.state()).unwrap();
        assert_eq!(restored_chain.tip().hash(), chain_b.tip().hash());
        assert!(restored_mempool.contains_hash(&pending_dependent.hash()));
        assert!(!restored_mempool.contains_hash(&orphaned_transfer.hash()));

        let _ = fs::remove_dir_all(base);
    }
}

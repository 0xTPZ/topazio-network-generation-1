use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::json;

use topazio_network::{
    block::Block, crypto::Hash, mining::mine_block, rpc::request, transaction::Transaction,
};

fn main() {
    if let Err(error) = run() {
        eprintln!("topazio-miner: {}", error);
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rpc = option(&args, "--rpc").unwrap_or_else(|| "127.0.0.1:18541".to_string());
    let address = option(&args, "--address").ok_or_else(|| "--address is required".to_string())?;
    let max_nonce: u64 = option(&args, "--max-nonce")
        .unwrap_or_else(|| "10000000".to_string())
        .parse()
        .map_err(|_| "--max-nonce must be an unsigned integer".to_string())?;

    let info = request(
        &rpc,
        json!({"method": "mining-info", "address": address.clone()}),
        16 * 1024 * 1024,
    )?;
    let result = info
        .get("result")
        .ok_or_else(|| format!("mining-info failed: {}", info))?;
    let height = result
        .get("height")
        .and_then(|value| value.as_u64())
        .ok_or_else(|| "invalid mining height".to_string())?;
    let previous_hash = decode_hash(
        result
            .get("previous_hash")
            .and_then(|value| value.as_str())
            .ok_or_else(|| "invalid previous hash".to_string())?,
    )?;
    let difficulty = result
        .get("difficulty_bits")
        .and_then(|value| value.as_u64())
        .ok_or_else(|| "invalid mining difficulty".to_string())? as u16;
    let reward = result
        .get("reward")
        .and_then(|value| value.as_u64())
        .ok_or_else(|| "invalid mining reward".to_string())?;
    let previous_timestamp = result
        .get("previous_timestamp")
        .and_then(|value| value.as_u64())
        .unwrap_or(0);
    let coinbase = Transaction::coinbase(address, reward, height)?;
    let mut transactions = vec![coinbase];

    if let Some(values) = result.get("transactions") {
        let values = values
            .as_array()
            .ok_or_else(|| "mining transactions must be an array".to_string())?;

        for value in values {
            let encoded = value
                .as_str()
                .ok_or_else(|| "mining transaction must be hex text".to_string())?;
            let bytes = hex::decode(encoded)
                .map_err(|_| "mining transaction is not valid hex".to_string())?;
            let transaction = Transaction::decode(&bytes)?;

            if transaction.is_coinbase() {
                return Err("mining template cannot include a coinbase transaction".to_string());
            }

            transactions.push(transaction);
        }
    }

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| "system clock is before Unix epoch".to_string())?
        .as_secs();
    let timestamp = now.max(previous_timestamp.saturating_add(1));
    let candidate = Block::with_transactions_at_difficulty(
        height,
        previous_hash,
        timestamp,
        0,
        difficulty,
        &transactions,
    )?;
    let mined = mine_block(candidate, difficulty, max_nonce)?;
    let response = request(
        &rpc,
        json!({"method": "submit_block", "block": hex::encode(mined.encode())}),
        16 * 1024 * 1024,
    )?;
    println!(
        "{}",
        serde_json::to_string_pretty(&response).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn option(args: &[String], name: &str) -> Option<String> {
    let position = args.iter().position(|value| value == name)?;
    args.get(position + 1).cloned()
}

fn decode_hash(value: &str) -> Result<Hash, String> {
    let bytes = hex::decode(value).map_err(|_| "previous hash is not valid hex".to_string())?;
    if bytes.len() != 32 {
        return Err("previous hash must contain 32 bytes".to_string());
    }
    let mut hash = [0u8; 32];
    hash.copy_from_slice(&bytes);
    Ok(hash)
}

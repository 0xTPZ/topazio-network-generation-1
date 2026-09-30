use serde_json::{Value, json};

use topazio_network::{rpc::request as rpc_request, wallet::Wallet};

fn main() {
    if let Err(error) = run() {
        eprintln!("topazio-cli: {}", error);
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let rpc = take_option(&mut args, "--rpc").unwrap_or_else(|| "127.0.0.1:18541".to_string());
    let command = args
        .first()
        .cloned()
        .ok_or_else(|| "RPC command is required".to_string())?;

    let request = match command.as_str() {
        "send" => build_send_request(&mut args, &rpc)?,
        "balance" | "nonce" => {
            let address = args
                .get(1)
                .ok_or_else(|| "address is required".to_string())?;
            json!({"method": command, "address": address})
        }
        "transaction" => {
            let hash = args
                .get(1)
                .ok_or_else(|| "transaction hash is required".to_string())?;
            json!({"method": "transaction", "hash": hash})
        }
        "status" | "network-info" | "height" | "tip" | "mempool" | "peers" | "mining-info" => {
            json!({"method": command})
        }
        _ => return Err("unknown CLI command".to_string()),
    };

    let response = rpc_request(&rpc, request, 16 * 1024 * 1024)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&response).map_err(|error| error.to_string())?
    );
    Ok(())
}

fn build_send_request(args: &mut [String], rpc: &str) -> Result<Value, String> {
    let wallet_path =
        option_from(args, "--wallet").ok_or_else(|| "--wallet is required".to_string())?;
    let password =
        option_from(args, "--password").ok_or_else(|| "--password is required".to_string())?;
    let to = option_from(args, "--to").ok_or_else(|| "--to is required".to_string())?;
    let amount: u64 = option_from(args, "--amount")
        .ok_or_else(|| "--amount is required".to_string())?
        .parse()
        .map_err(|_| "amount must be an unsigned integer".to_string())?;
    let fee: u64 = option_from(args, "--fee")
        .unwrap_or_else(|| "0".to_string())
        .parse()
        .map_err(|_| "fee must be an unsigned integer".to_string())?;
    let nonce = rpc_request(rpc, json!({"method": "nonce", "address": Wallet::open(wallet_path.clone(), &password)?.address()}), 16 * 1024 * 1024)?
        .get("result").and_then(|result| result.get("nonce")).and_then(Value::as_u64)
        .ok_or_else(|| "RPC nonce response was invalid".to_string())?;
    let wallet = Wallet::open(wallet_path, &password)?;
    let transaction = wallet.sign_transfer(to, amount, fee, nonce)?;

    Ok(json!({
        "method": "submit_transaction",
        "transaction": hex::encode(transaction.encode()),
        "amount": transaction.amount,
        "fee": transaction.fee,
    }))
}

fn take_option(args: &mut Vec<String>, name: &str) -> Option<String> {
    let position = args.iter().position(|value| value == name)?;
    args.remove(position);
    if position < args.len() {
        Some(args.remove(position))
    } else {
        None
    }
}

fn option_from(args: &[String], name: &str) -> Option<String> {
    let position = args.iter().position(|value| value == name)?;
    args.get(position + 1).cloned()
}

use std::path::PathBuf;

use topazio_network::wallet::Wallet;

fn main() {
    if let Err(error) = run() {
        eprintln!("topazio-wallet: {}", error);
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let mut args = std::env::args().skip(1);
    let command = args
        .next()
        .ok_or_else(|| "usage: topazio-wallet <create|address> <file> <password>".to_string())?;
    let path = PathBuf::from(
        args.next()
            .ok_or_else(|| "wallet file is required".to_string())?,
    );
    let password = args
        .next()
        .ok_or_else(|| "password is required".to_string())?;

    match command.as_str() {
        "create" => {
            let wallet = Wallet::create(path, &password)?;
            println!("{}", wallet.address());
        }
        "address" => {
            let wallet = Wallet::open(path, &password)?;
            println!("{}", wallet.address());
        }
        _ => return Err("unknown wallet command".to_string()),
    }

    Ok(())
}

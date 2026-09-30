use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

use topazio_network::{
    NETWORK_ID,
    block::Block,
    blockchain::Blockchain,
    config::Config,
    consensus::{COINBASE_MATURITY_BLOCKS, INITIAL_BLOCK_REWARD, LAB_DIFFICULTY_BITS},
    genesis::{EXPECTED_GENESIS_HASH, genesis_hash_hex},
    identity::KeyPair,
    mempool::Mempool,
    mining::mine_block,
    storage::{load_blockchain, save_blockchain},
    transaction::{Transaction, encode_transactions},
    wallet::Wallet,
};

fn temporary_path(name: &str) -> std::path::PathBuf {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    std::env::temp_dir().join(format!(
        "topazio-testnet-{}-{}-{}",
        name,
        std::process::id(),
        unique
    ))
}

fn mine_next_block(blockchain: &Blockchain, transactions: &[Transaction]) -> Block {
    let difficulty = blockchain.next_difficulty_bits();
    let candidate = Block::with_difficulty(
        blockchain.next_height(),
        blockchain.tip().hash(),
        blockchain.tip().timestamp + 1,
        0,
        difficulty,
        encode_transactions(transactions),
    )
    .unwrap();

    mine_block(candidate, difficulty, 2_000_000).unwrap()
}

fn mature_first_coinbase(blockchain: &mut Blockchain, miner: &KeyPair) {
    while blockchain.tip().height <= COINBASE_MATURITY_BLOCKS {
        let height = blockchain.next_height();
        let coinbase = Transaction::coinbase(miner.address(), 0, height).unwrap();
        let block = mine_next_block(blockchain, &[coinbase]);
        blockchain.append_block(block).unwrap();
    }
}

#[test]
fn testnet_identity_and_default_config_are_stable() {
    assert_eq!(NETWORK_ID, "topazio-v1-testnet");
    assert_eq!(genesis_hash_hex(), EXPECTED_GENESIS_HASH);
    assert!(Config::default().validate().is_ok());
}

#[test]
fn mined_coinbase_updates_state_and_work() {
    let miner = KeyPair::from_secret_bytes([41u8; 32]);
    let mut blockchain = Blockchain::new();
    let coinbase = Transaction::coinbase(miner.address(), INITIAL_BLOCK_REWARD, 1).unwrap();
    let candidate = Block::with_difficulty(
        1,
        blockchain.tip().hash(),
        blockchain.tip().timestamp + 1,
        0,
        LAB_DIFFICULTY_BITS,
        encode_transactions(&[coinbase]),
    )
    .unwrap();
    let mined = mine_block(candidate, LAB_DIFFICULTY_BITS, 2_000_000).unwrap();
    blockchain.append_block(mined).unwrap();
    assert_eq!(
        blockchain.state().balance_of(&miner.address()),
        INITIAL_BLOCK_REWARD
    );
    assert_eq!(blockchain.state().spendable_balance_of(&miner.address()), 0);
    assert_eq!(
        blockchain.state().immature_balance_of(&miner.address()),
        INITIAL_BLOCK_REWARD
    );
    assert!(blockchain.cumulative_work() > 0);
    assert!(blockchain.validate().is_ok());
}

#[test]
fn encrypted_wallet_does_not_accept_wrong_password() {
    let path = temporary_path("wallet");
    let wallet = Wallet::create(&path, "testnet-password").unwrap();
    assert!(Wallet::open(&path, "wrong-password").is_err());
    assert_eq!(
        Wallet::open(&path, "testnet-password").unwrap().address(),
        wallet.address()
    );
    let _ = fs::remove_file(path);
}

#[test]
fn pending_transaction_is_mined_confirmed_and_recovered_after_restart() {
    let base = temporary_path("mempool-confirmation");
    let chain_path = base.join("chain.dat");
    let mempool_path = base.join("mempool.dat");
    fs::create_dir_all(&base).unwrap();

    let alice = KeyPair::from_secret_bytes([42u8; 32]);
    let bob = KeyPair::from_secret_bytes([43u8; 32]);

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

    assert_eq!(
        blockchain.state().balance_of(&alice.address()),
        INITIAL_BLOCK_REWARD
    );

    let transfer = Transaction::signed(&alice, bob.address(), 5, 0, 0).unwrap();
    let transfer_hash = transfer.hash();
    let mut mempool = Mempool::new();
    mempool
        .add_transaction(transfer.clone(), blockchain.state())
        .unwrap();
    assert_eq!(mempool.len(), 1);

    let confirmation_reward = Transaction::coinbase(
        alice.address(),
        INITIAL_BLOCK_REWARD,
        blockchain.next_height(),
    )
    .unwrap();
    let block_transactions = mempool
        .transactions_for_block(confirmation_reward, blockchain.state())
        .unwrap();

    assert_eq!(block_transactions.len(), 2);
    assert_eq!(block_transactions[1].hash(), transfer_hash);

    let confirmation_block = mine_next_block(&blockchain, &block_transactions);
    let confirmation_height = confirmation_block.height;
    blockchain.append_block(confirmation_block.clone()).unwrap();
    mempool.remove_confirmed(&block_transactions);

    assert_eq!(confirmation_height, COINBASE_MATURITY_BLOCKS + 2);
    assert!(mempool.is_empty());
    assert_eq!(
        blockchain.state().balance_of(&alice.address()),
        (INITIAL_BLOCK_REWARD * 2) - 5
    );
    assert_eq!(blockchain.state().balance_of(&bob.address()), 5);
    assert_eq!(blockchain.state().nonce_of(&alice.address()), 1);
    assert!(blockchain.validate().is_ok());

    save_blockchain(&blockchain, &chain_path).unwrap();
    mempool.save(&mempool_path).unwrap();

    let restored = load_blockchain(&chain_path).unwrap();
    let restored_mempool = Mempool::load(&mempool_path, restored.state()).unwrap();

    assert_eq!(restored.tip().height, COINBASE_MATURITY_BLOCKS + 2);
    assert_eq!(restored.tip().hash(), confirmation_block.hash());
    assert!(restored_mempool.is_empty());
    assert_eq!(
        restored.state().balance_of(&alice.address()),
        (INITIAL_BLOCK_REWARD * 2) - 5
    );
    assert_eq!(restored.state().balance_of(&bob.address()), 5);

    let _ = fs::remove_dir_all(base);
}

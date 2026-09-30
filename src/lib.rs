pub mod block;
pub mod blockchain;
pub mod config;
pub mod consensus;
pub mod crypto;
pub mod genesis;
pub mod identity;
pub mod mempool;
pub mod mining;
pub mod network;
pub mod node;
pub mod rpc;
pub mod state;
pub mod storage;
pub mod sync;
pub mod transaction;
pub mod wallet;

pub const NETWORK_NAME: &str = "Topazio Network";

pub const NETWORK_SYMBOL: &str = "TPZ";

pub const NETWORK_VERSION: &str = "1.0.0-testnet";

pub const NETWORK_ID: &str = "topazio-v1-testnet";

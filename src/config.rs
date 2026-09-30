use std::{
    fs,
    path::{Path, PathBuf},
};

use serde::{Deserialize, Serialize};

use crate::{NETWORK_ID, NETWORK_VERSION};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub network: String,
    pub data_dir: PathBuf,
    pub p2p_bind: String,
    pub p2p_port: u16,
    pub rpc_bind: String,
    pub rpc_port: u16,
    pub bootnodes: Vec<String>,
    pub max_peers: usize,
    pub peer_timeout_secs: u64,
    pub rpc_max_request_bytes: usize,
    pub mempool_max_transactions: usize,
    pub mining_enabled: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            network: NETWORK_ID.to_string(),
            data_dir: PathBuf::from("data/testnet"),
            p2p_bind: "0.0.0.0".to_string(),
            p2p_port: 18_540,
            rpc_bind: "127.0.0.1".to_string(),
            rpc_port: 18_541,
            bootnodes: Vec::new(),
            max_peers: 32,
            peer_timeout_secs: 10,
            rpc_max_request_bytes: 1024 * 1024,
            mempool_max_transactions: 10_000,
            mining_enabled: false,
        }
    }
}

impl Config {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref();
        let source = fs::read_to_string(path)
            .map_err(|error| format!("failed to read config {}: {}", path.display(), error))?;

        let config: Self = toml::from_str(&source)
            .map_err(|error| format!("invalid Topazio config: {}", error))?;

        config.validate()?;
        Ok(config)
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.network != NETWORK_ID {
            return Err(format!(
                "config network must be {}, received {}",
                NETWORK_ID, self.network
            ));
        }

        if self.max_peers == 0 || self.max_peers > 1_000 {
            return Err("max_peers must be between 1 and 1000".to_string());
        }

        if self.peer_timeout_secs == 0 || self.peer_timeout_secs > 300 {
            return Err("peer_timeout_secs must be between 1 and 300".to_string());
        }

        if self.rpc_max_request_bytes < 1024 || self.rpc_max_request_bytes > 16 * 1024 * 1024 {
            return Err("rpc_max_request_bytes is outside the supported range".to_string());
        }

        if self.mempool_max_transactions == 0 || self.mempool_max_transactions > 10_000 {
            return Err("mempool_max_transactions is outside the supported range".to_string());
        }

        Ok(())
    }

    pub fn version(&self) -> &'static str {
        NETWORK_VERSION
    }
}

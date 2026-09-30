use std::{
    collections::HashSet,
    fs,
    net::{TcpListener, TcpStream},
    path::PathBuf,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
        mpsc::{self, SyncSender, TrySendError},
    },
    thread,
    time::{Duration, Instant},
};

use serde_json::json;

use topazio_network::{
    NETWORK_ID,
    blockchain::Blockchain,
    config::Config,
    mempool::Mempool,
    network::{Handshake, receive_handshake, send_handshake},
    node::NodeIdentity,
    rpc::{RpcState, serve},
    storage::{load_blockchain, save_blockchain},
    sync::{
        MAX_BLOCKS_PER_REQUEST, SyncMessage, blocks_for_request, receive_sync_message,
        send_sync_message,
    },
};

#[derive(Clone)]
struct PeerRegistry {
    inner: Arc<Mutex<Vec<PeerConnection>>>,
    next_id: Arc<AtomicU64>,
}

struct PeerConnection {
    id: u64,
    node_id: String,
    sender: SyncSender<SyncMessage>,
}

const PEER_OUTBOX_CAPACITY: usize = 256;

impl PeerRegistry {
    fn new() -> Self {
        Self {
            inner: Arc::new(Mutex::new(Vec::new())),
            next_id: Arc::new(AtomicU64::new(1)),
        }
    }

    fn register(
        &self,
        node_id: String,
        sender: SyncSender<SyncMessage>,
        max_peers: usize,
    ) -> Result<u64, String> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let mut peers = self
            .inner
            .lock()
            .map_err(|_| "peer registry is unavailable".to_string())?;
        if peers.len() >= max_peers {
            return Err("maximum peer count reached".to_string());
        }
        if peers.iter().any(|peer| peer.node_id == node_id) {
            return Err("peer with this node id is already connected".to_string());
        }
        peers.push(PeerConnection {
            id,
            node_id,
            sender,
        });
        Ok(id)
    }

    fn remove(&self, id: u64) {
        if let Ok(mut peers) = self.inner.lock() {
            peers.retain(|peer| peer.id != id);
        }
    }

    fn count(&self) -> usize {
        self.inner.lock().map(|peers| peers.len()).unwrap_or(0)
    }

    fn broadcast(&self, message: SyncMessage) {
        if let Ok(mut peers) = self.inner.lock() {
            peers.retain(|peer| match peer.sender.try_send(message.clone()) {
                Ok(()) | Err(TrySendError::Full(_)) => true,
                Err(TrySendError::Disconnected(_)) => false,
            });
        }
    }
}

fn main() {
    if let Err(error) = run() {
        eprintln!("topazio-node: {}", error);
        std::process::exit(1);
    }
}

fn run() -> Result<(), String> {
    let config_path = argument_value("--config").unwrap_or_else(|| "topazio.toml".to_string());
    let config = if PathBuf::from(&config_path).exists() {
        Config::load(&config_path)?
    } else {
        eprintln!(
            "config {} not found; using built-in testnet defaults",
            config_path
        );
        Config::default()
    };

    fs::create_dir_all(&config.data_dir)
        .map_err(|error| format!("failed to create data directory: {}", error))?;

    let chain_path = config.data_dir.join("chain.dat");
    let mempool_path = config.data_dir.join("mempool.dat");
    let identity_path = config.data_dir.join("node.identity");

    let blockchain = if chain_path.exists() {
        load_blockchain(&chain_path)?
    } else {
        let blockchain = Blockchain::new();
        save_blockchain(&blockchain, &chain_path)?;
        blockchain
    };

    let mempool = if mempool_path.exists() {
        Mempool::load_with_limit(
            &mempool_path,
            blockchain.state(),
            config.mempool_max_transactions,
        )?
    } else {
        let mempool = Mempool::new_with_limit(config.mempool_max_transactions)?;
        mempool.save(&mempool_path)?;
        mempool
    };

    let identity = if identity_path.exists() {
        NodeIdentity::load(&identity_path)?
    } else {
        let identity = NodeIdentity::generate();
        identity.save(&identity_path)?;
        identity
    };

    let state = RpcState::new(blockchain, mempool, chain_path, mempool_path);
    let rpc_state = state.clone();
    let rpc_bind = format!("{}:{}", config.rpc_bind, config.rpc_port);
    let rpc_bind_for_thread = rpc_bind.clone();
    let rpc_limit = config.rpc_max_request_bytes;
    thread::spawn(move || {
        if let Err(error) = serve(&rpc_bind_for_thread, rpc_state, rpc_limit) {
            eprintln!("RPC server stopped: {}", error);
        }
    });

    let p2p_bind = format!("{}:{}", config.p2p_bind, config.p2p_port);
    let listener = TcpListener::bind(&p2p_bind)
        .map_err(|error| format!("failed to bind P2P listener {}: {}", p2p_bind, error))?;

    println!("Topazio Network {}", topazio_network::NETWORK_VERSION);
    println!("Network: {}", NETWORK_ID);
    println!("Node ID: {}", identity.address());
    println!("P2P: {}", p2p_bind);
    println!("RPC: {}", rpc_bind);

    let peers = PeerRegistry::new();
    let broadcast_state = state.clone();
    let broadcast_peers = peers.clone();
    let peer_timeout_secs = config.peer_timeout_secs;
    thread::spawn(move || {
        broadcast_state_changes(broadcast_state, broadcast_peers, peer_timeout_secs)
    });

    for bootnode in &config.bootnodes {
        let state = state.clone();
        let identity = identity.clone();
        let peers = peers.clone();
        let address = bootnode.clone();
        let max_peers = config.max_peers;
        thread::spawn(move || {
            maintain_bootnode_connection(
                address,
                identity,
                state,
                peers,
                max_peers,
                config.peer_timeout_secs,
            )
        });
    }

    for incoming in listener.incoming() {
        match incoming {
            Ok(stream) => {
                if peers.count() >= config.max_peers {
                    continue;
                }
                let state = state.clone();
                let identity = identity.clone();
                let peers = peers.clone();
                let max_peers = config.max_peers;
                let peer_timeout_secs = config.peer_timeout_secs;
                thread::spawn(move || {
                    if let Err(error) = serve_peer(
                        stream,
                        &identity,
                        &state,
                        &peers,
                        max_peers,
                        peer_timeout_secs,
                    ) {
                        eprintln!("P2P peer closed: {}", error);
                    }
                });
            }
            Err(error) => eprintln!("P2P accept error: {}", error),
        }
    }

    Ok(())
}

fn serve_peer(
    mut stream: TcpStream,
    identity: &NodeIdentity,
    state: &RpcState,
    peers: &PeerRegistry,
    max_peers: usize,
    peer_timeout_secs: u64,
) -> Result<(), String> {
    configure_peer_stream(&stream, peer_timeout_secs)?;

    let blockchain = state.blockchain_snapshot();
    let local = Handshake::new(
        identity.address(),
        blockchain.tip().height,
        blockchain.tip().hash(),
    )?;
    let peer = receive_handshake(&mut stream)?;
    local.validate_peer(&peer)?;
    send_handshake(&mut stream, &local)?;

    run_peer_session(stream, peer, state, peers, max_peers, peer_timeout_secs)
}

fn run_peer_session(
    mut stream: TcpStream,
    peer: Handshake,
    state: &RpcState,
    peers: &PeerRegistry,
    max_peers: usize,
    peer_timeout_secs: u64,
) -> Result<(), String> {
    configure_persistent_session(&stream, peer_timeout_secs)?;

    let (sender, receiver) = mpsc::sync_channel::<SyncMessage>(PEER_OUTBOX_CAPACITY);
    let peer_id = peers.register(peer.node_id.clone(), sender.clone(), max_peers)?;
    state.set_peer_count(peers.count());

    let mut writer = stream
        .try_clone()
        .map_err(|error| format!("failed to clone peer stream: {}", error))?;
    let writer_thread = thread::spawn(move || {
        while let Ok(message) = receiver.recv() {
            if let Err(error) = send_sync_message(&mut writer, &message) {
                eprintln!("P2P write failed: {}", error);
                break;
            }
        }
    });

    let session_result = peer_message_loop(&mut stream, peer, state, &sender);

    peers.remove(peer_id);
    state.set_peer_count(peers.count());
    drop(sender);
    let _ = writer_thread.join();

    session_result
}

fn peer_message_loop(
    stream: &mut TcpStream,
    peer: Handshake,
    state: &RpcState,
    sender: &SyncSender<SyncMessage>,
) -> Result<(), String> {
    request_initial_sync(&peer, state, sender)?;
    let mut reorg_retry_requested = false;

    loop {
        match receive_sync_message(stream) {
            Ok(SyncMessage::GetBlocks {
                start_height,
                limit,
            }) => {
                let blockchain = state.blockchain_snapshot();
                let blocks = blocks_for_request(&blockchain, start_height, limit)?;
                send_to_peer(sender, SyncMessage::blocks(blocks)?)?;
            }
            Ok(SyncMessage::Transaction { transaction }) => {
                let response = state.handle_json(json!({
                    "method": "submit_transaction",
                    "transaction": hex::encode(transaction.encode()),
                }));
                let accepted = response
                    .get("ok")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false)
                    && response
                        .get("result")
                        .and_then(|value| value.get("accepted"))
                        .and_then(|value| value.as_bool())
                        .unwrap_or(false);
                let reason = transaction_ack_reason(&response, accepted);
                let ack = SyncMessage::transaction_ack(transaction.hash(), accepted, reason)?;
                send_to_peer(sender, ack)?;
            }
            Ok(SyncMessage::Blocks { blocks }) => match state.import_blocks(blocks) {
                Ok(imported) => {
                    reorg_retry_requested = false;
                    if imported > 0 {
                        let blockchain = state.blockchain_snapshot();
                        send_to_peer(
                            sender,
                            SyncMessage::announce_tip(
                                blockchain.tip().height,
                                blockchain.tip().hash(),
                            ),
                        )?;
                    }
                }
                Err(error) => {
                    if reorg_retry_requested {
                        eprintln!(
                            "P2P invalid block batch while full-chain retry is pending: {}; ignoring batch",
                            error
                        );
                        continue;
                    }

                    reorg_retry_requested = true;
                    let request = SyncMessage::get_blocks(1, MAX_BLOCKS_PER_REQUEST)?;
                    send_to_peer(sender, request)?;
                }
            },
            Ok(SyncMessage::AnnounceTip { height, tip_hash }) => {
                let local = state.blockchain_snapshot();
                if height > local.tip().height {
                    let request = SyncMessage::get_blocks(local.next_height(), 128)?;
                    send_to_peer(sender, request)?;
                } else if height == local.tip().height && tip_hash != local.tip().hash() {
                    let request = SyncMessage::get_blocks(1, MAX_BLOCKS_PER_REQUEST)?;
                    send_to_peer(sender, request)?;
                }
            }
            Ok(SyncMessage::TransactionAck { .. }) => {}
            Err(error) => return Err(error),
        }
    }
}

fn maintain_bootnode_connection(
    address: String,
    identity: NodeIdentity,
    state: RpcState,
    peers: PeerRegistry,
    max_peers: usize,
    peer_timeout_secs: u64,
) {
    let mut backoff_secs = 1u64;

    loop {
        if peers.count() < max_peers {
            match connect_to_bootnode_once(
                &address,
                &identity,
                &state,
                &peers,
                max_peers,
                peer_timeout_secs,
            ) {
                Ok(()) => backoff_secs = 1,
                Err(error) => eprintln!("bootnode {} disconnected: {}", address, error),
            }
        }

        thread::sleep(Duration::from_secs(backoff_secs));
        backoff_secs = backoff_secs.saturating_mul(2).min(30);
    }
}

fn connect_to_bootnode_once(
    address: &str,
    identity: &NodeIdentity,
    state: &RpcState,
    peers: &PeerRegistry,
    max_peers: usize,
    peer_timeout_secs: u64,
) -> Result<(), String> {
    let mut stream = TcpStream::connect(address)
        .map_err(|error| format!("failed to connect to bootnode {}: {}", address, error))?;
    configure_peer_stream(&stream, peer_timeout_secs)?;

    let blockchain = state.blockchain_snapshot();
    let local = Handshake::new(
        identity.address(),
        blockchain.tip().height,
        blockchain.tip().hash(),
    )?;
    send_handshake(&mut stream, &local)?;
    let peer = receive_handshake(&mut stream)?;
    local.validate_peer(&peer)?;

    run_peer_session(stream, peer, state, peers, max_peers, peer_timeout_secs)
}

fn configure_peer_stream(stream: &TcpStream, timeout_secs: u64) -> Result<(), String> {
    stream
        .set_read_timeout(Some(Duration::from_secs(timeout_secs)))
        .map_err(|error| format!("failed to configure peer read timeout: {}", error))?;
    stream
        .set_write_timeout(Some(Duration::from_secs(10)))
        .map_err(|error| format!("failed to configure peer write timeout: {}", error))?;
    stream
        .set_nodelay(true)
        .map_err(|error| format!("failed to configure TCP_NODELAY: {}", error))?;
    Ok(())
}

fn configure_persistent_session(stream: &TcpStream, timeout_secs: u64) -> Result<(), String> {
    stream
        .set_read_timeout(Some(Duration::from_secs(timeout_secs)))
        .map_err(|error| {
            format!(
                "failed to configure persistent peer read timeout: {}",
                error
            )
        })?;
    Ok(())
}

fn request_initial_sync(
    peer: &Handshake,
    state: &RpcState,
    sender: &SyncSender<SyncMessage>,
) -> Result<(), String> {
    let local = state.blockchain_snapshot();

    if peer.height > local.tip().height {
        let request = SyncMessage::get_blocks(local.next_height(), MAX_BLOCKS_PER_REQUEST)?;
        send_to_peer(sender, request)?;
    } else if local.tip().height > peer.height || local.tip().hash() != peer.tip_hash {
        send_to_peer(
            sender,
            SyncMessage::announce_tip(local.tip().height, local.tip().hash()),
        )?;
    }

    Ok(())
}

fn send_to_peer(sender: &SyncSender<SyncMessage>, message: SyncMessage) -> Result<(), String> {
    sender
        .send(message)
        .map_err(|_| "peer writer is unavailable".to_string())
}

fn transaction_ack_reason(response: &serde_json::Value, accepted: bool) -> String {
    if accepted {
        return response
            .get("result")
            .and_then(|value| value.get("reason"))
            .and_then(|value| value.as_str())
            .unwrap_or("accepted into mempool")
            .to_string();
    }

    response
        .get("error")
        .and_then(|value| value.as_str())
        .unwrap_or("transaction rejected")
        .to_string()
}

fn broadcast_state_changes(state: RpcState, peers: PeerRegistry, peer_timeout_secs: u64) {
    let mut known_tip = {
        let blockchain = state.blockchain_snapshot();
        (blockchain.tip().height, blockchain.tip().hash())
    };
    let mut known_mempool = current_mempool_hashes(&state);
    let heartbeat_interval = Duration::from_secs((peer_timeout_secs / 2).max(1));
    let mut last_heartbeat = Instant::now();

    loop {
        thread::sleep(Duration::from_millis(500));

        let blockchain = state.blockchain_snapshot();
        let current_tip = (blockchain.tip().height, blockchain.tip().hash());

        if current_tip != known_tip {
            if current_tip.0 > known_tip.0 {
                let start = known_tip.0.saturating_add(1) as usize;
                let blocks = blockchain.blocks();

                if start < blocks.len() {
                    for chunk in blocks[start..].chunks(MAX_BLOCKS_PER_REQUEST as usize) {
                        if let Ok(message) = SyncMessage::blocks(chunk.to_vec()) {
                            peers.broadcast(message);
                        }
                    }
                }
            }

            peers.broadcast(SyncMessage::announce_tip(current_tip.0, current_tip.1));
            known_tip = current_tip;
        }

        if last_heartbeat.elapsed() >= heartbeat_interval {
            peers.broadcast(SyncMessage::announce_tip(current_tip.0, current_tip.1));
            last_heartbeat = Instant::now();
        }

        let mempool = state.mempool_snapshot();
        let current_hashes: HashSet<_> = mempool
            .transactions()
            .iter()
            .map(|transaction| transaction.hash())
            .collect();

        for transaction in mempool.transactions() {
            let hash = transaction.hash();

            if !known_mempool.contains(&hash) {
                if let Ok(message) = SyncMessage::transaction(transaction.clone()) {
                    peers.broadcast(message);
                }
            }
        }

        known_mempool = current_hashes;
        state.set_peer_count(peers.count());
    }
}

fn current_mempool_hashes(state: &RpcState) -> HashSet<[u8; 32]> {
    state
        .mempool_snapshot()
        .transactions()
        .iter()
        .map(|transaction| transaction.hash())
        .collect()
}

fn argument_value(name: &str) -> Option<String> {
    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        if argument == name {
            return args.next();
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peer_registry_bounds_slow_peer_outbox_without_blocking_broadcast() {
        let peers = PeerRegistry::new();
        let (sender, receiver) = mpsc::sync_channel(1);
        peers.register("slow-peer".to_string(), sender, 1).unwrap();

        let message = SyncMessage::announce_tip(1, [7u8; 32]);
        peers.broadcast(message.clone());
        peers.broadcast(message);

        assert!(receiver.try_recv().is_ok());
        assert!(receiver.try_recv().is_err());
    }

    #[test]
    fn peer_registry_rejects_duplicate_node_ids_and_excess_peers() {
        let peers = PeerRegistry::new();
        let (first_sender, _first_receiver) = mpsc::sync_channel(1);
        peers
            .register("peer-a".to_string(), first_sender, 1)
            .unwrap();

        let (duplicate_sender, _duplicate_receiver) = mpsc::sync_channel(1);
        assert!(
            peers
                .register("peer-a".to_string(), duplicate_sender, 1)
                .is_err()
        );

        let (second_sender, _second_receiver) = mpsc::sync_channel(1);
        assert!(
            peers
                .register("peer-b".to_string(), second_sender, 1)
                .is_err()
        );
    }
}

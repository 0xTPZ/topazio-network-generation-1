use std::{
    net::{Shutdown, TcpListener, TcpStream},
    path::Path,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    thread,
    time::Duration,
};

use topazio_network::{
    NETWORK_NAME,
    blockchain::Blockchain,
    crypto::hash_to_hex,
    mempool::Mempool,
    network::{Handshake, P2P_NETWORK_ID, P2P_PROTOCOL_VERSION, receive_handshake, send_handshake},
    node::NodeIdentity,
    storage::load_blockchain,
    sync::{SyncMessage, blocks_for_request, receive_sync_message, send_sync_message},
};

const LISTEN_ADDRESS: &str = "127.0.0.1:19481";

const STORAGE_PATH: &str = "data/topazio-chain-lab.dat";

const NODE_IDENTITY_PATH: &str = "data/topazio-node-identity-lab.dat";

const MEMPOOL_PATH: &str = "data/topazio-mempool-lab.dat";

const SOCKET_TIMEOUT_SECONDS: u64 = 15;

const TIP_MONITOR_INTERVAL_SECONDS: u64 = 1;

#[derive(Clone)]
struct ConnectedPeer {
    session_number: u64,
    node_id: String,
    writer: Arc<Mutex<TcpStream>>,
}

type PeerRegistry = Arc<Mutex<Vec<ConnectedPeer>>>;

fn main() {
    let identity = load_identity();

    let node_id = identity.address().to_string();

    let initial_blockchain = load_blockchain(Path::new(STORAGE_PATH)).unwrap_or_else(|error| {
        panic!("falha ao carregar blockchain principal: {}", error);
    });

    let initial_tip = initial_blockchain.tip();

    let mempool = load_or_create_mempool(&initial_blockchain);

    println!("========================================");
    println!("   TOPAZIO NODE SERVER MULTI-PEER");
    println!("========================================");
    println!();

    println!("Rede: {}", NETWORK_NAME);

    println!("Network ID: {}", P2P_NETWORK_ID);

    println!("Protocolo P2P: v{}", P2P_PROTOCOL_VERSION);

    println!("Node ID: {}", node_id);

    println!("Identidade: {}", NODE_IDENTITY_PATH);

    println!("Blockchain: {}", STORAGE_PATH);

    println!("Mempool: {}", MEMPOOL_PATH);

    println!("Pendentes iniciais: {}", mempool.len());

    println!("Altura inicial: {}", initial_tip.height);

    println!("Tip inicial: {}", hash_to_hex(&initial_tip.hash()));

    println!();

    let listener = TcpListener::bind(LISTEN_ADDRESS).unwrap_or_else(|error| {
        panic!("n├úo foi poss├¡vel abrir {}: {}", LISTEN_ADDRESS, error);
    });

    let shared_mempool = Arc::new(Mutex::new(mempool));

    let peer_registry: PeerRegistry = Arc::new(Mutex::new(Vec::new()));

    println!("Escutando permanentemente em: {}", LISTEN_ADDRESS);

    println!("Modo: m├║ltiplos peers + sess├Áes persistentes");

    println!("Registro global de peers: ATIVO");

    println!("Broadcast de Transaction: ATIVO");

    println!("AnnounceTip espont├óneo: ATIVO");

    println!("Monitor autom├ítico da blockchain: ATIVO");

    println!("Transaction P2P -> mempool: ATIVO");

    println!("Mempool persistente: ATIVO");

    println!(
        "Intervalo do monitor: {} segundo(s)",
        TIP_MONITOR_INTERVAL_SECONDS
    );

    println!("Pressione Ctrl+C para encerrar o n├│.");

    println!();

    println!("Aguardando peers...");

    let mut session_number: u64 = 0;

    loop {
        let (stream, peer_address) = match listener.accept() {
            Ok(connection) => connection,

            Err(error) => {
                eprintln!("Falha ao aceitar conex├úo: {}", error);

                continue;
            }
        };

        session_number = session_number.saturating_add(1);

        println!();

        println!("========================================");

        println!("   CONEX├âO #{} ACEITA", session_number);

        println!("========================================");

        println!("Origem: {}", peer_address);

        println!("Criando thread persistente para o peer...");

        let session_node_id = node_id.clone();

        let session_mempool = Arc::clone(&shared_mempool);

        let session_registry = Arc::clone(&peer_registry);

        thread::spawn(move || {
            run_peer_session(
                session_number,
                stream,
                &session_node_id,
                session_mempool,
                session_registry,
            );
        });

        println!("Peer #{} entregue ├á thread.", session_number);

        println!("Servidor principal continua livre.");

        println!();

        println!("Aguardando peers...");
    }
}

fn run_peer_session(
    session_number: u64,
    stream: TcpStream,
    node_id: &str,
    mempool: Arc<Mutex<Mempool>>,
    peer_registry: PeerRegistry,
) {
    println!();

    println!("[PEER #{}] Sess├úo iniciada.", session_number);

    let blockchain = match load_blockchain(Path::new(STORAGE_PATH)) {
        Ok(blockchain) => blockchain,

        Err(error) => {
            eprintln!(
                "[PEER #{}] Falha ao carregar blockchain: {}",
                session_number, error
            );

            println!("[PEER #{}] Thread finalizada.", session_number);

            return;
        }
    };

    println!("[PEER #{}] Blockchain recarregada: OK", session_number);

    println!(
        "[PEER #{}] Altura local: {}",
        session_number,
        blockchain.tip().height
    );

    println!(
        "[PEER #{}] Tip local: {}",
        session_number,
        hash_to_hex(&blockchain.tip().hash())
    );

    let session_active = Arc::new(AtomicBool::new(true));

    match handle_peer(
        session_number,
        stream,
        node_id,
        &blockchain,
        mempool,
        Arc::clone(&peer_registry),
        Arc::clone(&session_active),
    ) {
        Ok(()) => {
            println!();

            println!("[PEER #{}] Sess├úo conclu├¡da normalmente.", session_number);
        }

        Err(error) => {
            eprintln!();

            eprintln!("[PEER #{}] Sess├úo encerrada: {}", session_number, error);
        }
    }

    session_active.store(false, Ordering::SeqCst);

    remove_peer_from_registry(session_number, &peer_registry);

    println!("[PEER #{}] Thread finalizada.", session_number);
}

fn handle_peer(
    session_number: u64,
    mut stream: TcpStream,
    node_id: &str,
    blockchain: &Blockchain,
    mempool: Arc<Mutex<Mempool>>,
    peer_registry: PeerRegistry,
    session_active: Arc<AtomicBool>,
) -> Result<(), String> {
    configure_stream(&stream)?;

    let local_tip = blockchain.tip();

    let local_handshake = Handshake::new(node_id.to_string(), local_tip.height, local_tip.hash())?;

    let remote_handshake = receive_handshake(&mut stream)?;

    local_handshake.validate_peer(&remote_handshake)?;

    send_handshake(&mut stream, &local_handshake)?;

    println!();

    println!("[PEER #{}] Handshake: OK", session_number);

    println!(
        "[PEER #{}] Node ID remoto: {}",
        session_number, remote_handshake.node_id
    );

    println!(
        "[PEER #{}] Network ID remoto: {}",
        session_number, remote_handshake.network_id
    );

    println!(
        "[PEER #{}] Protocolo remoto: v{}",
        session_number, remote_handshake.protocol_version
    );

    println!(
        "[PEER #{}] Altura remota: {}",
        session_number, remote_handshake.height
    );

    println!(
        "[PEER #{}] Altura local: {}",
        session_number, local_tip.height
    );

    let peer_already_synced = remote_handshake.height == local_tip.height
        && remote_handshake.tip_hash == local_tip.hash();

    if remote_handshake.height == local_tip.height && remote_handshake.tip_hash != local_tip.hash()
    {
        return Err(format!(
            "peer possui mesma altura {}, mas tip diferente; poss├¡vel fork",
            local_tip.height
        ));
    }

    if remote_handshake.height > local_tip.height {
        println!();

        println!(
            "[PEER #{}] Peer est├í {} bloco(s) ├á frente.",
            session_number,
            remote_handshake.height - local_tip.height
        );

        println!(
            "[PEER #{}] Sincroniza├º├úo reversa ainda n├úo implementada.",
            session_number
        );

        return Ok(());
    }

    if peer_already_synced {
        println!();

        println!("[PEER #{}] Status: j├í sincronizado.", session_number);

        println!("[PEER #{}] Mesma altura e mesmo tip.", session_number);
    } else {
        println!();

        println!(
            "[PEER #{}] Peer est├í {} bloco(s) atr├ís.",
            session_number,
            local_tip.height - remote_handshake.height
        );

        println!(
            "[PEER #{}] Servi├ºo de sincroniza├º├úo dispon├¡vel.",
            session_number
        );
    }

    stream
        .set_read_timeout(None)
        .map_err(|error| format!("falha ao ativar conex├úo persistente: {}", error))?;

    let writer_stream = stream
        .try_clone()
        .map_err(|error| format!("falha ao clonar socket do peer: {}", error))?;

    let writer = Arc::new(Mutex::new(writer_stream));

    register_peer(
        session_number,
        remote_handshake.node_id.clone(),
        Arc::clone(&writer),
        &peer_registry,
    )?;

    println!();

    println!("[PEER #{}] Modo persistente: ATIVO", session_number);

    let mut monitor_started = false;

    if peer_already_synced {
        send_current_tip_announcement(session_number, &writer)?;

        start_tip_monitor(
            session_number,
            Arc::clone(&writer),
            local_tip.height,
            local_tip.hash(),
            Arc::clone(&peer_registry),
            Arc::clone(&session_active),
        );

        monitor_started = true;
    }

    println!("[PEER #{}] Aguardando mensagens...", session_number);

    loop {
        let message = receive_sync_message(&mut stream)?;

        match message {
            SyncMessage::GetBlocks {
                start_height,
                limit,
            } => {
                println!();

                println!("[PEER #{}] Pedido GetBlocks recebido:", session_number);

                println!(
                    "[PEER #{}]   In├¡cio: bloco #{}",
                    session_number, start_height
                );

                println!("[PEER #{}]   Limite: {}", session_number, limit);

                let current_blockchain =
                    load_blockchain(Path::new(STORAGE_PATH)).map_err(|error| {
                        format!("falha ao recarregar blockchain para GetBlocks: {}", error)
                    })?;

                let current_tip = current_blockchain.tip();

                println!(
                    "[PEER #{}]   Altura atual do servidor: {}",
                    session_number, current_tip.height
                );

                let blocks = blocks_for_request(&current_blockchain, start_height, limit)?;

                println!(
                    "[PEER #{}]   Enviando: {} bloco(s)",
                    session_number,
                    blocks.len()
                );

                let last_height = blocks.last().map(|block| block.height);

                if let (Some(first), Some(last)) = (blocks.first(), blocks.last()) {
                    println!(
                        "[PEER #{}]   Faixa: #{} at├® #{}",
                        session_number, first.height, last.height
                    );
                }

                let response = SyncMessage::blocks(blocks)?;

                send_message_with_writer(&writer, &response)?;

                println!("[PEER #{}] Resposta Blocks enviada.", session_number);

                if let Some(height) = last_height {
                    if height >= current_tip.height {
                        send_tip_announcement_with_writer(
                            session_number,
                            &writer,
                            current_tip.height,
                            current_tip.hash(),
                        )?;

                        if !monitor_started {
                            start_tip_monitor(
                                session_number,
                                Arc::clone(&writer),
                                current_tip.height,
                                current_tip.hash(),
                                Arc::clone(&peer_registry),
                                Arc::clone(&session_active),
                            );

                            monitor_started = true;
                        }
                    }
                }

                println!("[PEER #{}] Conex├úo permanece ativa.", session_number);
            }

            SyncMessage::Blocks { .. } => {
                return Err("peer enviou Blocks sem solicita├º├úo".to_string());
            }

            SyncMessage::AnnounceTip { height, tip_hash } => {
                println!();

                println!("[PEER #{}] AnnounceTip recebido.", session_number);

                println!("[PEER #{}] Altura anunciada: {}", session_number, height);

                println!(
                    "[PEER #{}] Tip anunciado: {}",
                    session_number,
                    hash_to_hex(&tip_hash)
                );

                println!(
                    "[PEER #{}] Nenhuma altera├º├úo na blockchain nesta fase.",
                    session_number
                );

                println!("[PEER #{}] Conex├úo permanece ativa.", session_number);
            }

            SyncMessage::Transaction { transaction } => {
                let transaction_hash = transaction.hash();

                println!();

                println!(
                    "[PEER #{}] ========================================",
                    session_number
                );

                println!("[PEER #{}] TRANSACTION P2P RECEBIDA", session_number);

                println!(
                    "[PEER #{}] Hash: {}",
                    session_number,
                    hash_to_hex(&transaction_hash)
                );

                println!("[PEER #{}] De: {}", session_number, transaction.from);

                println!("[PEER #{}] Para: {}", session_number, transaction.to);

                println!(
                    "[PEER #{}] Valor: {} TPZ",
                    session_number, transaction.amount
                );

                println!("[PEER #{}] Nonce: {}", session_number, transaction.nonce);

                /*
                 * Outro processo pode ter
                 * minerado desde que esta
                 * conex├úo foi aberta.
                 */
                let current_blockchain =
                    load_blockchain(Path::new(STORAGE_PATH)).map_err(|error| {
                        format!(
                            "falha ao carregar estado confirmado para transaction: {}",
                            error
                        )
                    })?;

                println!(
                    "[PEER #{}] Estado confirmado recarregado: altura {}",
                    session_number,
                    current_blockchain.tip().height
                );

                /*
                 * Fazemos a mudan├ºa primeiro
                 * em uma c├│pia do mempool.
                 */
                {
                    let mut active_mempool = mempool
                        .lock()
                        .map_err(|_| "mutex do mempool foi corrompido".to_string())?;

                    let mut candidate_mempool = active_mempool.clone();

                    match candidate_mempool
                        .add_transaction(transaction.clone(), current_blockchain.state())
                    {
                        Ok(()) => {}

                        Err(error) => {
                            println!("[PEER #{}] TRANSACTION REJEITADA", session_number);

                            println!("[PEER #{}] Motivo: {}", session_number, error);

                            println!(
                                "[PEER #{}] Mempool permanece com {} pendente(s).",
                                session_number,
                                active_mempool.len()
                            );

                            println!("[PEER #{}] Conex├úo permanece ativa.", session_number);

                            continue;
                        }
                    }

                    candidate_mempool.save(MEMPOOL_PATH).map_err(|error| {
                        format!(
                            "transaction v├ílida, mas falhou ao persistir mempool: {}",
                            error
                        )
                    })?;

                    *active_mempool = candidate_mempool;

                    println!("[PEER #{}] Valida├º├úo criptogr├ífica: OK", session_number);

                    println!("[PEER #{}] Valida├º├úo econ├┤mica: OK", session_number);

                    println!("[PEER #{}] Entrada no mempool: OK", session_number);

                    println!("[PEER #{}] Persist├¬ncia: OK", session_number);

                    println!("[PEER #{}] Arquivo: {}", session_number, MEMPOOL_PATH);

                    println!(
                        "[PEER #{}] Pendentes agora: {}",
                        session_number,
                        active_mempool.len()
                    );
                }

                println!(
                    "[PEER #{}] TRANSACTION ACEITA: {}",
                    session_number,
                    hash_to_hex(&transaction_hash)
                );

                /*
                 * Somente depois de:
                 *
                 * - validar;
                 * - inserir no mempool;
                 * - persistir;
                 *
                 * fazemos broadcast.
                 */
                let broadcast_message = SyncMessage::transaction(transaction)?;

                let broadcast_result =
                    broadcast_to_other_peers(session_number, &broadcast_message, &peer_registry)?;

                println!("[PEER #{}] Broadcast conclu├¡do.", session_number);

                println!(
                    "[PEER #{}] Peers alcan├ºados: {}",
                    session_number, broadcast_result.sent
                );

                println!(
                    "[PEER #{}] Peers removidos por falha: {}",
                    session_number, broadcast_result.removed
                );

                println!(
                    "[PEER #{}] ========================================",
                    session_number
                );

                println!("[PEER #{}] Conex├úo permanece ativa.", session_number);
            }

            SyncMessage::TransactionAck { .. } => {
                println!(
                    "[PEER #{}] TransactionAck inesperado; ignorado.",
                    session_number
                );
            }
        }
    }
}

#[derive(Debug)]
struct BroadcastResult {
    sent: usize,
    removed: usize,
}

fn register_peer(
    session_number: u64,
    node_id: String,
    writer: Arc<Mutex<TcpStream>>,
    peer_registry: &PeerRegistry,
) -> Result<(), String> {
    let mut registry = peer_registry
        .lock()
        .map_err(|_| "mutex do registro de peers foi corrompido".to_string())?;

    registry.retain(|peer| peer.session_number != session_number);

    registry.push(ConnectedPeer {
        session_number,
        node_id: node_id.clone(),
        writer,
    });

    println!();

    println!("[PEER #{}] REGISTRO GLOBAL: OK", session_number);

    println!("[PEER #{}] Node ID registrado: {}", session_number, node_id);

    println!(
        "[PEER #{}] Peers conectados agora: {}",
        session_number,
        registry.len()
    );

    Ok(())
}

fn remove_peer_from_registry(session_number: u64, peer_registry: &PeerRegistry) {
    let mut registry = match peer_registry.lock() {
        Ok(registry) => registry,

        Err(_) => {
            eprintln!(
                "[PEER #{}] Falha ao acessar registro global durante remo├º├úo.",
                session_number
            );

            return;
        }
    };

    let before = registry.len();

    registry.retain(|peer| peer.session_number != session_number);

    if registry.len() != before {
        println!("[PEER #{}] Removido do registro global.", session_number);

        println!(
            "[PEER #{}] Peers conectados restantes: {}",
            session_number,
            registry.len()
        );
    }
}

fn broadcast_to_other_peers(
    origin_session_number: u64,
    message: &SyncMessage,
    peer_registry: &PeerRegistry,
) -> Result<BroadcastResult, String> {
    /*
     * Copiamos a lista para n├úo manter
     * o mutex global bloqueado durante
     * opera├º├Áes TCP.
     */
    let peers = {
        let registry = peer_registry
            .lock()
            .map_err(|_| "mutex do registro de peers foi corrompido".to_string())?;

        registry.clone()
    };

    println!();

    println!("[PEER #{}] Iniciando broadcast...", origin_session_number);

    println!(
        "[PEER #{}] Peers registrados: {}",
        origin_session_number,
        peers.len()
    );

    let mut sent = 0usize;

    let mut failed_sessions = Vec::new();

    for peer in peers {
        if peer.session_number == origin_session_number {
            continue;
        }

        println!(
            "[PEER #{}] >>> enviando Transaction para PEER #{} ({})",
            origin_session_number, peer.session_number, peer.node_id
        );

        match send_message_with_writer(&peer.writer, message) {
            Ok(()) => {
                sent = sent.saturating_add(1);

                println!(
                    "[PEER #{}] >>> PEER #{} recebeu broadcast: OK",
                    origin_session_number, peer.session_number
                );
            }

            Err(error) => {
                eprintln!(
                    "[PEER #{}] >>> falha ao enviar para PEER #{}: {}",
                    origin_session_number, peer.session_number, error
                );

                failed_sessions.push(peer.session_number);
            }
        }
    }

    let removed = remove_failed_peers(&failed_sessions, peer_registry)?;

    Ok(BroadcastResult { sent, removed })
}

fn remove_failed_peers(
    failed_sessions: &[u64],
    peer_registry: &PeerRegistry,
) -> Result<usize, String> {
    if failed_sessions.is_empty() {
        return Ok(0);
    }

    let mut registry = peer_registry
        .lock()
        .map_err(|_| "mutex do registro de peers foi corrompido".to_string())?;

    let before = registry.len();

    registry.retain(|peer| !failed_sessions.contains(&peer.session_number));

    Ok(before.saturating_sub(registry.len()))
}

fn is_peer_registered(session_number: u64, peer_registry: &PeerRegistry) -> bool {
    let registry = match peer_registry.lock() {
        Ok(registry) => registry,

        Err(_) => {
            return false;
        }
    };

    registry
        .iter()
        .any(|peer| peer.session_number == session_number)
}

fn shutdown_peer_writer(writer: &Arc<Mutex<TcpStream>>) {
    let stream = match writer.lock() {
        Ok(stream) => stream,

        Err(_) => {
            return;
        }
    };

    let _ = stream.shutdown(Shutdown::Both);
}

fn start_tip_monitor(
    session_number: u64,
    writer: Arc<Mutex<TcpStream>>,
    initial_height: u64,
    initial_hash: [u8; 32],
    peer_registry: PeerRegistry,
    session_active: Arc<AtomicBool>,
) {
    println!(
        "[PEER #{}] Monitor automático do tip: ATIVO",
        session_number
    );

    println!(
        "[PEER #{}] Monitorando crescimento a partir da altura {}",
        session_number, initial_height
    );

    thread::spawn(move || {
        let mut last_height = initial_height;

        let mut last_hash = initial_hash;

        loop {
            /*
             * O monitor pertence à mesma sessão P2P.
             *
             * Se a thread principal da sessão terminou
             * ou se o peer já foi removido do registro
             * global, este monitor também deve terminar.
             */
            if !session_active.load(Ordering::SeqCst)
                || !is_peer_registered(session_number, &peer_registry)
            {
                println!(
                    "[PEER #{}] Monitor do tip encerrado: sessão não está mais ativa.",
                    session_number
                );

                break;
            }

            thread::sleep(Duration::from_secs(TIP_MONITOR_INTERVAL_SECONDS));

            /*
             * Repetimos a checagem depois do sleep.
             *
             * Isso evita carregar a blockchain e tentar
             * escrever em um socket de uma sessão que
             * morreu durante o intervalo do monitor.
             */
            if !session_active.load(Ordering::SeqCst)
                || !is_peer_registered(session_number, &peer_registry)
            {
                println!(
                    "[PEER #{}] Monitor do tip encerrado: peer desconectado.",
                    session_number
                );

                break;
            }

            let blockchain = match load_blockchain(Path::new(STORAGE_PATH)) {
                Ok(blockchain) => blockchain,

                Err(error) => {
                    eprintln!(
                        "[PEER #{}] Monitor: falha ao carregar blockchain: {}",
                        session_number, error
                    );

                    continue;
                }
            };

            let tip = blockchain.tip();

            let current_height = tip.height;

            let current_hash = tip.hash();

            if current_height == last_height && current_hash == last_hash {
                continue;
            }

            println!();

            println!(
                "[PEER #{}] ========================================",
                session_number
            );

            println!(
                "[PEER #{}] NOVO TIP DETECTADO AUTOMATICAMENTE",
                session_number
            );

            println!(
                "[PEER #{}] Altura anterior: {}",
                session_number, last_height
            );

            println!("[PEER #{}] Nova altura: {}", session_number, current_height);

            println!(
                "[PEER #{}] Novo tip: {}",
                session_number,
                hash_to_hex(&current_hash)
            );

            println!(
                "[PEER #{}] ========================================",
                session_number
            );

            match send_tip_announcement_with_writer(
                session_number,
                &writer,
                current_height,
                current_hash,
            ) {
                Ok(()) => {
                    last_height = current_height;

                    last_hash = current_hash;
                }

                Err(error) => {
                    /*
                     * Uma falha de escrita é também um
                     * sinal de que este peer não é mais
                     * utilizável.
                     *
                     * Marcamos a sessão como inativa,
                     * removemos imediatamente o peer do
                     * registro e fechamos o socket para
                     * liberar a leitura bloqueada na
                     * thread principal da sessão.
                     */
                    session_active.store(false, Ordering::SeqCst);

                    remove_peer_from_registry(session_number, &peer_registry);

                    shutdown_peer_writer(&writer);

                    eprintln!(
                        "[PEER #{}] Monitor do tip detectou desconexão e encerrou a sessão: {}",
                        session_number, error
                    );

                    break;
                }
            }
        }
    });
}

fn send_current_tip_announcement(
    session_number: u64,
    writer: &Arc<Mutex<TcpStream>>,
) -> Result<(), String> {
    let blockchain = load_blockchain(Path::new(STORAGE_PATH))
        .map_err(|error| format!("falha ao carregar blockchain para AnnounceTip: {}", error))?;

    let tip = blockchain.tip();

    send_tip_announcement_with_writer(session_number, writer, tip.height, tip.hash())
}

fn send_tip_announcement_with_writer(
    session_number: u64,
    writer: &Arc<Mutex<TcpStream>>,
    height: u64,
    tip_hash: [u8; 32],
) -> Result<(), String> {
    let announcement = SyncMessage::announce_tip(height, tip_hash);

    send_message_with_writer(writer, &announcement)?;

    println!();

    println!("[PEER #{}] >>> AnnounceTip ENVIADO", session_number);

    println!("[PEER #{}]     Altura: {}", session_number, height);

    println!(
        "[PEER #{}]     Tip: {}",
        session_number,
        hash_to_hex(&tip_hash)
    );

    println!(
        "[PEER #{}]     Mensagem espont├ónea servidor -> peer: OK",
        session_number
    );

    Ok(())
}

fn send_message_with_writer(
    writer: &Arc<Mutex<TcpStream>>,
    message: &SyncMessage,
) -> Result<(), String> {
    let mut stream = writer
        .lock()
        .map_err(|_| "mutex de escrita do peer foi corrompido".to_string())?;

    send_sync_message(&mut stream, message)
}

fn load_or_create_mempool(blockchain: &Blockchain) -> Mempool {
    let path = Path::new(MEMPOOL_PATH);

    if path.exists() {
        println!("Mempool persistente encontrado.");

        let mempool = Mempool::load(path, blockchain.state()).unwrap_or_else(|error| {
            panic!(
                "arquivo do mempool existe, mas ├® inv├ílido para o estado atual: {}",
                error
            );
        });

        println!("Mempool carregado: OK");

        return mempool;
    }

    println!("Nenhum mempool persistente encontrado.");

    println!("Criando mempool vazio.");

    let mempool = Mempool::new();

    mempool.save(path).unwrap_or_else(|error| {
        panic!("falha ao criar mempool persistente: {}", error);
    });

    println!("Mempool persistente criado: OK");

    mempool
}

fn load_identity() -> NodeIdentity {
    let path = Path::new(NODE_IDENTITY_PATH);

    if path.exists() {
        return NodeIdentity::load(path).unwrap_or_else(|error| {
            panic!("falha ao carregar identidade do n├│: {}", error);
        });
    }

    let identity = NodeIdentity::generate();

    identity.save(path).unwrap_or_else(|error| {
        panic!("falha ao salvar identidade do n├│: {}", error);
    });

    identity
}

fn configure_stream(stream: &TcpStream) -> Result<(), String> {
    let timeout = Some(Duration::from_secs(SOCKET_TIMEOUT_SECONDS));

    stream
        .set_read_timeout(timeout)
        .map_err(|error| format!("falha ao configurar timeout de leitura: {}", error))?;

    stream
        .set_write_timeout(timeout)
        .map_err(|error| format!("falha ao configurar timeout de escrita: {}", error))?;

    stream
        .set_nodelay(true)
        .map_err(|error| format!("falha ao configurar TCP_NODELAY: {}", error))?;

    Ok(())
}

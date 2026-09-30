use std::{net::TcpStream, path::Path, thread, time::Duration};

use topazio_network::{
    NETWORK_NAME,
    blockchain::Blockchain,
    crypto::hash_to_hex,
    mempool::Mempool,
    network::{Handshake, P2P_NETWORK_ID, P2P_PROTOCOL_VERSION, receive_handshake, send_handshake},
    node::NodeIdentity,
    storage::{load_blockchain, save_blockchain},
    sync::{
        MAX_BLOCKS_PER_REQUEST, SyncMessage, apply_block_batch, receive_sync_message,
        send_sync_message,
    },
    transaction::Transaction,
};

const SERVER_ADDRESS: &str = "127.0.0.1:19481";

const NODE_STORAGE_PATH: &str = "data/node-b/topazio-chain-lab.dat";

const NODE_IDENTITY_PATH: &str = "data/topazio-node-client-lab.dat";

const NODE_MEMPOOL_PATH: &str = "data/node-b/topazio-mempool-lab.dat";

const SOCKET_TIMEOUT_SECONDS: u64 = 15;

const RECONNECT_DELAY_SECONDS: u64 = 5;

fn main() {
    println!("========================================");
    println!("       TOPAZIO NODE B");
    println!("========================================");
    println!();

    let mut blockchain = load_or_create_blockchain();

    println!();

    let mut mempool = load_or_create_mempool(&blockchain);

    println!();

    let identity = load_or_create_identity();

    let node_id = identity.address();

    let initial_tip = blockchain.tip();

    println!();

    println!("Rede: {}", NETWORK_NAME);
    println!("Network ID: {}", P2P_NETWORK_ID);
    println!("Protocolo P2P: v{}", P2P_PROTOCOL_VERSION);
    println!("Node ID: {}", node_id);
    println!("Identidade: {}", NODE_IDENTITY_PATH);
    println!("Blockchain independente: {}", NODE_STORAGE_PATH);
    println!("Mempool independente: {}", NODE_MEMPOOL_PATH);
    println!("Pendentes iniciais: {}", mempool.len());
    println!("Altura inicial: {}", initial_tip.height);
    println!("Tip inicial: {}", hash_to_hex(&initial_tip.hash()));

    println!();
    println!("Reconex├úo autom├ítica: ATIVA");
    println!(
        "Intervalo entre tentativas: {} segundo(s)",
        RECONNECT_DELAY_SECONDS
    );
    println!("Pressione Ctrl+C para encerrar o N├│ B.");

    let mut connection_attempt: u64 = 0;

    loop {
        connection_attempt = connection_attempt.saturating_add(1);

        println!();
        println!("========================================");
        println!("       CONEX├âO P2P");
        println!("========================================");
        println!("Tentativa de conex├úo: #{}", connection_attempt);
        println!("Peer: {}", SERVER_ADDRESS);

        let mut stream = match TcpStream::connect(SERVER_ADDRESS) {
            Ok(stream) => {
                println!("TCP conectado: OK");
                stream
            }

            Err(error) => {
                println!("Conex├úo indispon├¡vel.");
                println!("Motivo: {}", error);
                println!("Blockchain local preservada.");
                println!("Mempool local preservado.");
                println!(
                    "Nova tentativa em {} segundo(s)...",
                    RECONNECT_DELAY_SECONDS
                );

                thread::sleep(Duration::from_secs(RECONNECT_DELAY_SECONDS));

                continue;
            }
        };

        if let Err(error) = configure_stream(&stream) {
            println!();
            println!("Falha ao configurar a conex├úo TCP.");
            println!("Motivo: {}", error);
            println!(
                "Nova tentativa em {} segundo(s)...",
                RECONNECT_DELAY_SECONDS
            );

            thread::sleep(Duration::from_secs(RECONNECT_DELAY_SECONDS));

            continue;
        }

        println!("Configura├º├úo TCP: OK");

        match establish_peer_session(&mut stream, &node_id, &mut blockchain, &mut mempool) {
            Ok(()) => {
                println!();
                println!("Sess├úo P2P encerrada.");
            }

            Err(error) => {
                println!();
                println!("========================================");
                println!("       CONEX├âO P2P ENCERRADA");
                println!("========================================");
                println!("N├│ B perdeu a conex├úo com o peer.");
                println!("Motivo: {}", error);
                println!("Blockchain local preservada.");
                println!("Mempool local preservado.");
            }
        }

        println!();
        println!(
            "Tentando reconectar em {} segundo(s)...",
            RECONNECT_DELAY_SECONDS
        );

        thread::sleep(Duration::from_secs(RECONNECT_DELAY_SECONDS));
    }
}

fn establish_peer_session(
    stream: &mut TcpStream,
    node_id: &str,
    blockchain: &mut Blockchain,
    mempool: &mut Mempool,
) -> Result<(), String> {
    let local_tip = blockchain.tip();

    let local_handshake = Handshake::new(node_id.to_string(), local_tip.height, local_tip.hash())
        .map_err(|error| format!("falha ao criar handshake local: {}", error))?;

    send_handshake(stream, &local_handshake)
        .map_err(|error| format!("falha ao enviar handshake: {}", error))?;

    let remote_handshake = receive_handshake(stream)
        .map_err(|error| format!("falha ao receber handshake: {}", error))?;

    local_handshake
        .validate_peer(&remote_handshake)
        .map_err(|error| format!("peer rejeitado: {}", error))?;

    println!();
    println!("========================================");
    println!("         PEER ACEITO");
    println!("========================================");
    println!("Handshake: OK");
    println!("Node ID remoto: {}", remote_handshake.node_id);
    println!("Altura remota: {}", remote_handshake.height);
    println!("Tip remoto: {}", hash_to_hex(&remote_handshake.tip_hash));

    let local_tip = blockchain.tip();

    if local_tip.height == remote_handshake.height && local_tip.hash() == remote_handshake.tip_hash
    {
        println!();
        println!("Blockchain do N├│ B j├í est├í sincronizada.");
        println!("Nenhum bloco precisa ser baixado.");
    } else if local_tip.height == remote_handshake.height
        && local_tip.hash() != remote_handshake.tip_hash
    {
        return Err(format!(
            "mesma altura com tip diferente; poss├¡vel fork na altura {}",
            local_tip.height
        ));
    } else if local_tip.height > remote_handshake.height {
        return Err(format!(
            "N├│ B est├í ├á frente do servidor: local {} / remoto {}; sincroniza├º├úo reversa ainda n├úo implementada",
            local_tip.height, remote_handshake.height
        ));
    } else {
        println!();
        println!("========================================");
        println!("     SINCRONIZA├ç├âO INICIAL DO N├ô B");
        println!("========================================");

        synchronize_to_tip(
            stream,
            blockchain,
            mempool,
            NODE_STORAGE_PATH,
            NODE_MEMPOOL_PATH,
            remote_handshake.height,
            remote_handshake.tip_hash,
        )?;

        println!();
        println!("Sincroniza├º├úo inicial do N├│ B conclu├¡da: OK");
    }

    blockchain
        .validate()
        .map_err(|error| format!("valida├º├úo final da blockchain falhou: {}", error))?;

    enter_persistent_mode(stream, blockchain, mempool)
}

fn enter_persistent_mode(
    stream: &mut TcpStream,
    blockchain: &mut Blockchain,
    mempool: &mut Mempool,
) -> Result<(), String> {
    stream.set_read_timeout(None).map_err(|error| {
        format!(
            "falha ao remover timeout da conex├úo persistente: {}",
            error
        )
    })?;

    println!();
    println!("========================================");
    println!("       CONEX├âO P2P PERSISTENTE");
    println!("========================================");
    println!("N├│ B permanece conectado.");
    println!("Altura local: {}", blockchain.tip().height);
    println!("Tip local: {}", hash_to_hex(&blockchain.tip().hash()));
    println!("Mempool local: {} pendente(s)", mempool.len());

    println!();
    println!("Sincroniza├º├úo autom├ítica por AnnounceTip: ATIVA");
    println!("Reconex├úo autom├ítica: ATIVA");
    println!("Mempool P2P independente: ATIVO");
    println!("Persist├¬ncia do mempool: ATIVA");
    println!("Mensagens intercaladas durante sync: SUPORTADAS");
    println!("Aguardando mensagens da rede...");
    println!("Pressione Ctrl+C para encerrar o N├│ B.");

    loop {
        let message = receive_sync_message(stream)?;

        match message {
            SyncMessage::AnnounceTip { height, tip_hash } => {
                println!();
                println!("----------------------------------------");
                println!("        AnnounceTip RECEBIDO");
                println!("----------------------------------------");
                println!("Altura anunciada: {}", height);
                println!("Tip anunciado: {}", hash_to_hex(&tip_hash));

                let local_tip = blockchain.tip();

                println!("Altura local atual: {}", local_tip.height);

                if height < local_tip.height {
                    println!("An├║ncio antigo ignorado.");
                    println!("Conex├úo continua ativa.");
                    continue;
                }

                if height == local_tip.height {
                    if tip_hash == local_tip.hash() {
                        println!("Blockchain j├í possui esse tip.");
                        println!("Nenhuma sincroniza├º├úo necess├íria.");
                        println!("Conex├úo continua ativa.");
                        continue;
                    }

                    return Err(format!(
                        "AnnounceTip indica poss├¡vel fork na altura {}: tip local {} / tip remoto {}",
                        height,
                        hash_to_hex(&local_tip.hash()),
                        hash_to_hex(&tip_hash)
                    ));
                }

                println!();
                println!("Novo tip detectado.");
                println!("Faltam {} bloco(s).", height - local_tip.height);
                println!("Iniciando sincroniza├º├úo autom├ítica...");

                synchronize_to_tip(
                    stream,
                    blockchain,
                    mempool,
                    NODE_STORAGE_PATH,
                    NODE_MEMPOOL_PATH,
                    height,
                    tip_hash,
                )?;

                println!();
                println!("========================================");
                println!("    SINCRONIZA├ç├âO AUTOM├üTICA OK");
                println!("========================================");
                println!("Nova altura local: {}", blockchain.tip().height);
                println!("Novo tip local: {}", hash_to_hex(&blockchain.tip().hash()));
                println!("Mempool pendente: {}", mempool.len());
                println!("Persist├¬ncia do N├│ B: OK");
                println!("Conex├úo continua ativa.");

                println!();
                println!("Aguardando mensagens da rede...");
            }

            SyncMessage::Blocks { .. } => {
                return Err(
                    "servidor enviou Blocks sem solicita├º├úo durante modo persistente".to_string(),
                );
            }

            SyncMessage::GetBlocks { .. } => {
                return Err("servidor enviou GetBlocks inesperado ao N├│ B".to_string());
            }

            SyncMessage::Transaction { transaction } => {
                process_received_transaction(
                    transaction,
                    blockchain,
                    mempool,
                    NODE_MEMPOOL_PATH,
                    false,
                )?;
            }

            SyncMessage::TransactionAck {
                transaction_hash,
                accepted,
                reason,
            } => {
                println!();
                println!("----------------------------------------");
                println!("      TransactionAck RECEBIDO");
                println!("----------------------------------------");
                println!("Hash: {}", hash_to_hex(&transaction_hash));
                println!("Status: {}", if accepted { "ACEITA" } else { "REJEITADA" });
                println!("Motivo: {}", reason);
                println!("ACK informativo recebido pelo Nó B.");
                println!("Conexão continua ativa.");
            }
        }
    }
}

fn synchronize_to_tip(
    stream: &mut TcpStream,
    blockchain: &mut Blockchain,
    mempool: &mut Mempool,
    storage_path: &str,
    mempool_path: &str,
    target_height: u64,
    target_hash: [u8; 32],
) -> Result<(), String> {
    let mut current_target_height = target_height;
    let mut current_target_hash = target_hash;

    while blockchain.tip().height < current_target_height {
        let start_height = blockchain.next_height();

        println!();
        println!("Solicitando blocos a partir de #{}...", start_height);

        let request = SyncMessage::get_blocks(start_height, MAX_BLOCKS_PER_REQUEST)?;

        send_sync_message(stream, &request)?;

        println!("GetBlocks enviado: OK");
        println!("Aguardando resposta Blocks...");
        println!("Mensagens intercaladas s├úo permitidas durante a espera.");

        let blocks = loop {
            let response = receive_sync_message(stream)?;

            match response {
                SyncMessage::Blocks { blocks } => {
                    break blocks;
                }

                SyncMessage::GetBlocks { .. } => {
                    return Err(
                        "servidor enviou GetBlocks inesperado enquanto aguard├ívamos Blocks"
                            .to_string(),
                    );
                }

                SyncMessage::AnnounceTip { height, tip_hash } => {
                    println!();
                    println!("----------------------------------------");
                    println!(" AnnounceTip INTERCALADO DURANTE SYNC");
                    println!("----------------------------------------");
                    println!("Altura anunciada: {}", height);
                    println!("Tip anunciado: {}", hash_to_hex(&tip_hash));
                    println!("Alvo atual: {}", current_target_height);

                    let local_tip = blockchain.tip();

                    if height < local_tip.height {
                        println!("AnnounceTip antigo ignorado.");
                        println!("Continuamos aguardando Blocks...");
                        continue;
                    }

                    if height == local_tip.height {
                        if tip_hash != local_tip.hash() {
                            return Err(format!(
                                "AnnounceTip intercalado indica poss├¡vel fork na altura {}: tip local {} / tip remoto {}",
                                height,
                                hash_to_hex(&local_tip.hash()),
                                hash_to_hex(&tip_hash)
                            ));
                        }

                        println!("AnnounceTip corresponde ao tip local.");
                        println!("Continuamos aguardando Blocks...");
                        continue;
                    }

                    if height < current_target_height {
                        println!("AnnounceTip anterior ao alvo atual ignorado.");
                        println!("Continuamos aguardando Blocks...");
                        continue;
                    }

                    if height == current_target_height {
                        if tip_hash != current_target_hash {
                            return Err(format!(
                                "AnnounceTip conflitante para a altura {}: alvo atual {} / novo an├║ncio {}",
                                height,
                                hash_to_hex(&current_target_hash),
                                hash_to_hex(&tip_hash)
                            ));
                        }

                        println!("AnnounceTip confirma o alvo atual.");
                        println!("Continuamos aguardando Blocks...");
                        continue;
                    }

                    println!(
                        "Blockchain remota cresceu durante a sincroniza├º├úo: {} -> {}",
                        current_target_height, height
                    );

                    current_target_height = height;
                    current_target_hash = tip_hash;

                    println!("Novo alvo de sincroniza├º├úo registrado.");
                    println!("Nova altura alvo: {}", current_target_height);
                    println!("Novo tip alvo: {}", hash_to_hex(&current_target_hash));
                    println!("Continuamos aguardando o lote Blocks solicitado...");
                }

                SyncMessage::Transaction { transaction } => {
                    println!();
                    println!("----------------------------------------");
                    println!(" Transaction INTERCALADA DURANTE SYNC");
                    println!("----------------------------------------");

                    process_received_transaction(
                        transaction,
                        blockchain,
                        mempool,
                        mempool_path,
                        true,
                    )?;

                    println!("Continuamos aguardando Blocks...");
                }

                SyncMessage::TransactionAck {
                    transaction_hash,
                    accepted,
                    reason,
                } => {
                    println!();
                    println!("----------------------------------------");
                    println!(" TransactionAck INTERCALADO DURANTE SYNC");
                    println!("----------------------------------------");
                    println!("Hash: {}", hash_to_hex(&transaction_hash));
                    println!("Status: {}", if accepted { "ACEITA" } else { "REJEITADA" });
                    println!("Motivo: {}", reason);
                    println!("ACK informativo ignorado pelo fluxo de sync.");
                    println!("Continuamos aguardando Blocks...");
                }
            }
        };

        if blocks.is_empty() {
            return Err(format!(
                "servidor respondeu lote vazio antes de alcan├ºar a altura {}",
                current_target_height
            ));
        }

        println!("Recebidos: {} bloco(s)", blocks.len());

        if let (Some(first), Some(last)) = (blocks.first(), blocks.last()) {
            println!("Faixa recebida: #{} at├® #{}", first.height, last.height);
        }

        let mut confirmed_transactions = Vec::new();

        for block in &blocks {
            let transactions = block.transactions()?;

            for transaction in transactions {
                if !transaction.is_coinbase() {
                    confirmed_transactions.push(transaction);
                }
            }
        }

        let received_count = blocks.len();

        println!("Validando lote...");

        apply_block_batch(blockchain, blocks)?;

        println!("Lote validado: {} bloco(s)", received_count);
        println!("Altura ap├│s valida├º├úo: {}", blockchain.tip().height);

        save_blockchain(blockchain, Path::new(storage_path))?;

        println!("Blockchain persistida: OK");

        if !confirmed_transactions.is_empty() {
            let before = mempool.len();

            mempool.remove_confirmed(&confirmed_transactions);

            let after = mempool.len();

            mempool.save(mempool_path)?;

            println!(
                "Mempool reconciliado com blocos confirmados: {} -> {}",
                before, after
            );

            println!("Mempool persistido: OK");
        }
    }

    let final_tip = blockchain.tip();

    if final_tip.height != current_target_height {
        return Err(format!(
            "altura ap├│s sincroniza├º├úo ├® {}, mas alvo atual esperava {}",
            final_tip.height, current_target_height
        ));
    }

    if final_tip.hash() != current_target_hash {
        return Err(format!(
            "tip ap├│s sincroniza├º├úo n├úo corresponde ao alvo atual: local {} / anunciado {}",
            hash_to_hex(&final_tip.hash()),
            hash_to_hex(&current_target_hash)
        ));
    }

    blockchain.validate().map_err(|error| {
        format!(
            "valida├º├úo completa ap├│s sincroniza├º├úo falhou: {}",
            error
        )
    })?;

    Ok(())
}

fn process_received_transaction(
    transaction: Transaction,
    blockchain: &Blockchain,
    mempool: &mut Mempool,
    mempool_path: &str,
    during_sync: bool,
) -> Result<(), String> {
    let transaction_hash = transaction.hash();

    if !during_sync {
        println!();
        println!("----------------------------------------");
        println!("       Transaction RECEBIDA");
        println!("----------------------------------------");
    }

    println!("Hash: {}", hash_to_hex(&transaction_hash));
    println!("De: {}", transaction.from);
    println!("Para: {}", transaction.to);
    println!("Valor: {} TPZ", transaction.amount);
    println!("Nonce: {}", transaction.nonce);

    let mut candidate_mempool = mempool.clone();

    match candidate_mempool.add_transaction(transaction, blockchain.state()) {
        Ok(()) => {}

        Err(error) => {
            if during_sync {
                println!("TRANSACTION INTERCALADA REJEITADA NO N├ô B");
            } else {
                println!("TRANSACTION REJEITADA NO N├ô B");
            }

            println!("Motivo: {}", error);
            println!("Mempool permanece com {} pendente(s).", mempool.len());
            println!("Conex├úo continua ativa.");

            return Ok(());
        }
    }

    candidate_mempool.save(mempool_path)?;

    *mempool = candidate_mempool;

    println!("Valida├º├úo criptogr├ífica: OK");
    println!("Valida├º├úo econ├┤mica: OK");
    println!("Entrada no mempool do N├│ B: OK");
    println!("Persist├¬ncia: OK");
    println!("Arquivo: {}", mempool_path);
    println!("Pendentes agora: {}", mempool.len());

    if during_sync {
        println!(
            "TRANSACTION INTERCALADA ACEITA NO N├ô B: {}",
            hash_to_hex(&transaction_hash)
        );
    } else {
        println!(
            "TRANSACTION ACEITA NO N├ô B: {}",
            hash_to_hex(&transaction_hash)
        );
    }

    println!("Conex├úo continua ativa.");

    Ok(())
}

fn load_or_create_blockchain() -> Blockchain {
    println!("Blockchain do N├│ B:");

    let path = Path::new(NODE_STORAGE_PATH);

    if path.exists() {
        println!("  Arquivo encontrado");

        let blockchain = load_blockchain(path).unwrap_or_else(|error| {
            panic!("falha ao carregar blockchain do N├│ B: {}", error);
        });

        println!("  Carregamento: OK");
        println!("  Altura: {}", blockchain.tip().height);

        return blockchain;
    }

    println!("  Nenhum arquivo encontrado");
    println!("  Criando blockchain independente");

    let blockchain = Blockchain::new();

    println!("  Estado inicial: somente genesis");

    save_blockchain(&blockchain, path).unwrap_or_else(|error| {
        panic!("falha ao salvar genesis do N├│ B: {}", error);
    });

    println!("  Genesis salvo: OK");

    blockchain
}

fn load_or_create_mempool(blockchain: &Blockchain) -> Mempool {
    println!("Mempool do N├│ B:");

    let path = Path::new(NODE_MEMPOOL_PATH);

    if path.exists() {
        println!("  Arquivo encontrado");

        let mempool = Mempool::load(path, blockchain.state()).unwrap_or_else(|error| {
            panic!("falha ao carregar mempool do N├│ B: {}", error);
        });

        println!("  Carregamento: OK");
        println!("  Pendentes: {}", mempool.len());

        return mempool;
    }

    println!("  Nenhum arquivo encontrado");
    println!("  Criando mempool independente vazio");

    let mempool = Mempool::new();

    mempool.save(path).unwrap_or_else(|error| {
        panic!("falha ao salvar mempool do N├│ B: {}", error);
    });

    println!("  Mempool criado: OK");

    mempool
}

fn load_or_create_identity() -> NodeIdentity {
    println!("Identidade do N├│ B:");

    let path = Path::new(NODE_IDENTITY_PATH);

    if path.exists() {
        println!("  Arquivo encontrado");

        let identity = NodeIdentity::load(path).unwrap_or_else(|error| {
            panic!("falha ao carregar identidade do N├│ B: {}", error);
        });

        println!("  Carregamento: OK");

        return identity;
    }

    println!("  Nenhum arquivo encontrado");
    println!("  Gerando nova identidade");

    let identity = NodeIdentity::generate();

    identity.save(path).unwrap_or_else(|error| {
        panic!("falha ao salvar identidade do N├│ B: {}", error);
    });

    println!("  Identidade criada: OK");

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

#[cfg(test)]
mod tests {
    use super::*;

    use std::{
        fs,
        net::{TcpListener, TcpStream},
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use topazio_network::{
        block::Block,
        consensus::INITIAL_BLOCK_REWARD,
        genesis::GENESIS_TIMESTAMP,
        identity::KeyPair,
        mining::mine_block,
        sync::{SyncMessage, receive_sync_message, send_sync_message},
    };

    fn miner_key() -> KeyPair {
        KeyPair::from_secret_bytes([5u8; 32])
    }

    fn bob_key() -> KeyPair {
        KeyPair::from_secret_bytes([9u8; 32])
    }

    fn temporary_path(label: &str) -> PathBuf {
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();

        std::env::temp_dir().join(format!(
            "topazio-node-b-test-{}-{}-{}.dat",
            label,
            std::process::id(),
            nanos
        ))
    }

    fn mine_next_coinbase_block(blockchain: &Blockchain, miner: &KeyPair) -> Block {
        let height = blockchain.next_height();

        let coinbase =
            Transaction::coinbase(miner.address(), INITIAL_BLOCK_REWARD, height).unwrap();

        let transactions = vec![coinbase];

        let timestamp = GENESIS_TIMESTAMP
            .checked_add(height)
            .expect("test block timestamp overflow");

        let candidate =
            Block::with_transactions(height, blockchain.tip().hash(), timestamp, 0, &transactions)
                .unwrap();

        mine_block(candidate, blockchain.difficulty_bits(), 1_000_000)
            .expect("failed to mine test block")
    }

    fn chain_at_height_one() -> Blockchain {
        let miner = miner_key();

        let mut blockchain = Blockchain::new();

        let block = mine_next_coinbase_block(&blockchain, &miner);

        blockchain.append_block(block).unwrap();

        assert_eq!(blockchain.tip().height, 1);

        blockchain
    }

    #[test]
    fn transaction_can_arrive_between_getblocks_and_blocks() {
        let mut blockchain = chain_at_height_one();

        let miner = miner_key();
        let bob = bob_key();

        assert_eq!(
            blockchain.state().balance_of(&miner.address()),
            INITIAL_BLOCK_REWARD
        );

        assert_eq!(blockchain.state().nonce_of(&miner.address()), 0);

        let interleaved_transaction = Transaction::signed(&miner, bob.address(), 10, 0, 0).unwrap();

        let block_two = mine_next_coinbase_block(&blockchain, &miner);

        assert_eq!(block_two.height, 2);

        let target_hash = block_two.hash();

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();

        let address = listener.local_addr().unwrap();

        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();

            let request = receive_sync_message(&mut stream).unwrap();

            match request {
                SyncMessage::GetBlocks {
                    start_height,
                    limit,
                } => {
                    assert_eq!(start_height, 2);
                    assert_eq!(limit, MAX_BLOCKS_PER_REQUEST);
                }

                other => {
                    panic!("expected GetBlocks, received {:?}", other);
                }
            }

            let transaction_message = SyncMessage::transaction(interleaved_transaction).unwrap();

            send_sync_message(&mut stream, &transaction_message).unwrap();

            let blocks_message = SyncMessage::blocks(vec![block_two]).unwrap();

            send_sync_message(&mut stream, &blocks_message).unwrap();
        });

        let mut client = TcpStream::connect(address).unwrap();

        configure_stream(&client).unwrap();

        let storage_path = temporary_path("transaction-chain");
        let mempool_path = temporary_path("transaction-mempool");

        let storage_path_string = storage_path.to_string_lossy().into_owned();
        let mempool_path_string = mempool_path.to_string_lossy().into_owned();

        let mut mempool = Mempool::new();

        synchronize_to_tip(
            &mut client,
            &mut blockchain,
            &mut mempool,
            &storage_path_string,
            &mempool_path_string,
            2,
            target_hash,
        )
        .unwrap();

        server.join().unwrap();

        assert_eq!(blockchain.tip().height, 2);
        assert_eq!(blockchain.tip().hash(), target_hash);

        // The height-1 coinbase is still immature, so the interleaved spend
        // is correctly rejected instead of entering the mempool.
        assert_eq!(mempool.len(), 0);

        assert!(storage_path.exists());
        assert!(!mempool_path.exists());

        let _ = fs::remove_file(storage_path);
        let _ = fs::remove_file(mempool_path);
    }

    #[test]
    fn higher_announce_tip_can_arrive_between_getblocks_and_blocks() {
        let mut blockchain = chain_at_height_one();

        let miner = miner_key();

        let block_two = mine_next_coinbase_block(&blockchain, &miner);

        let mut remote_blockchain =
            Blockchain::from_blocks(blockchain.difficulty_bits(), blockchain.blocks().to_vec())
                .unwrap();

        remote_blockchain.append_block(block_two.clone()).unwrap();

        assert_eq!(remote_blockchain.tip().height, 2);

        let block_three = mine_next_coinbase_block(&remote_blockchain, &miner);

        assert_eq!(block_three.height, 3);

        let block_two_hash = block_two.hash();
        let block_three_hash = block_three.hash();

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();

        let address = listener.local_addr().unwrap();

        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();

            let first_request = receive_sync_message(&mut stream).unwrap();

            match first_request {
                SyncMessage::GetBlocks {
                    start_height,
                    limit,
                } => {
                    assert_eq!(start_height, 2);
                    assert_eq!(limit, MAX_BLOCKS_PER_REQUEST);
                }

                other => {
                    panic!("expected first GetBlocks, received {:?}", other);
                }
            }

            /*
             * A cadeia remota cresce para #3
             * enquanto o N├│ B espera o Blocks
             * referente ao pedido iniciado em #2.
             */
            let announce = SyncMessage::announce_tip(3, block_three_hash);

            send_sync_message(&mut stream, &announce).unwrap();

            let first_blocks = SyncMessage::blocks(vec![block_two]).unwrap();

            send_sync_message(&mut stream, &first_blocks).unwrap();

            /*
             * O N├│ B deve atualizar o alvo para #3
             * e solicitar o pr├│ximo lote.
             */
            let second_request = receive_sync_message(&mut stream).unwrap();

            match second_request {
                SyncMessage::GetBlocks {
                    start_height,
                    limit,
                } => {
                    assert_eq!(start_height, 3);
                    assert_eq!(limit, MAX_BLOCKS_PER_REQUEST);
                }

                other => {
                    panic!("expected second GetBlocks, received {:?}", other);
                }
            }

            let second_blocks = SyncMessage::blocks(vec![block_three]).unwrap();

            send_sync_message(&mut stream, &second_blocks).unwrap();
        });

        let mut client = TcpStream::connect(address).unwrap();

        configure_stream(&client).unwrap();

        let storage_path = temporary_path("announce-chain");
        let mempool_path = temporary_path("announce-mempool");

        let storage_path_string = storage_path.to_string_lossy().into_owned();
        let mempool_path_string = mempool_path.to_string_lossy().into_owned();

        let mut mempool = Mempool::new();

        synchronize_to_tip(
            &mut client,
            &mut blockchain,
            &mut mempool,
            &storage_path_string,
            &mempool_path_string,
            2,
            block_two_hash,
        )
        .unwrap();

        server.join().unwrap();

        /*
         * O alvo inicial era #2.
         * O AnnounceTip intercalado mudou o alvo para #3.
         */
        assert_eq!(blockchain.tip().height, 3);
        assert_eq!(blockchain.tip().hash(), block_three_hash);

        assert_eq!(mempool.len(), 0);

        assert!(storage_path.exists());

        let _ = fs::remove_file(storage_path);
        let _ = fs::remove_file(mempool_path);
    }
}

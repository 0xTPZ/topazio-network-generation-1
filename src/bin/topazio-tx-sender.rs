use std::{
    net::{Shutdown, TcpStream},
    path::Path,
    time::Duration,
};

use topazio_network::{
    NETWORK_NAME,
    crypto::hash_to_hex,
    identity::KeyPair,
    network::{Handshake, P2P_NETWORK_ID, P2P_PROTOCOL_VERSION, receive_handshake, send_handshake},
    node::NodeIdentity,
    storage::load_blockchain,
    sync::{SyncMessage, receive_sync_message, send_sync_message},
    transaction::Transaction,
};

const SERVER_ADDRESS: &str = "127.0.0.1:19481";

const STORAGE_PATH: &str = "data/topazio-chain-lab.dat";

const WALLET_IDENTITY_PATH: &str = "data/topazio-node-identity-lab.dat";

const P2P_IDENTITY_PATH: &str = "data/topazio-tx-sender-identity-lab.dat";

const SOCKET_TIMEOUT_SECONDS: u64 = 15;

const TEST_TRANSFER_AMOUNT: u64 = 10;

fn main() {
    println!("========================================");
    println!("      TOPAZIO TX P2P SENDER");
    println!("========================================");
    println!();

    println!("Rede: {}", NETWORK_NAME);

    println!("Network ID: {}", P2P_NETWORK_ID);

    println!("Protocolo P2P: v{}", P2P_PROTOCOL_VERSION);

    /*
     * A blockchain local é usada apenas
     * para descobrir:
     *
     * - saldo confirmado;
     * - nonce confirmado;
     * - altura;
     * - tip atual.
     */
    let blockchain = load_blockchain(Path::new(STORAGE_PATH)).unwrap_or_else(|error| {
        panic!("falha ao carregar blockchain {}: {}", STORAGE_PATH, error);
    });

    blockchain.validate().unwrap_or_else(|error| {
        panic!("blockchain local inválida: {}", error);
    });

    let tip = blockchain.tip();

    println!();

    println!("Blockchain:");

    println!("  Arquivo: {}", STORAGE_PATH);

    println!("  Altura: {}", tip.height);

    println!("  Tip: {}", hash_to_hex(&tip.hash()));

    /*
     * Esta identidade é a carteira que
     * atualmente recebe as recompensas
     * de mineração.
     *
     * Ela é usada como CARTEIRA.
     *
     * Não será usada como identidade
     * P2P desta conexão.
     */
    let wallet_identity =
        NodeIdentity::load(Path::new(WALLET_IDENTITY_PATH)).unwrap_or_else(|error| {
            panic!(
                "falha ao carregar carteira financiada {}: {}",
                WALLET_IDENTITY_PATH, error
            );
        });

    let wallet = wallet_identity.key_pair();

    let sender_address = wallet.address();

    let sender_balance = blockchain.state().balance_of(&sender_address);

    let sender_nonce = blockchain.state().nonce_of(&sender_address);

    /*
     * Bob é a carteira determinística
     * que já faz parte dos testes
     * econômicos atuais da rede lab.
     */
    let bob = KeyPair::from_secret_bytes([9u8; 32]);

    let recipient_address = bob.address();

    println!();

    println!("Carteira remetente:");

    println!("  Arquivo: {}", WALLET_IDENTITY_PATH);

    println!("  Endereço: {}", sender_address);

    println!("  Saldo confirmado: {} TPZ", sender_balance);

    println!("  Nonce confirmado: {}", sender_nonce);

    println!();

    println!("Carteira destinatária:");

    println!("  Bob: {}", recipient_address);

    println!(
        "  Saldo atual: {} TPZ",
        blockchain.state().balance_of(&recipient_address)
    );

    if sender_balance < TEST_TRANSFER_AMOUNT {
        panic!(
            "saldo insuficiente: carteira possui {} TPZ e o teste exige {} TPZ",
            sender_balance, TEST_TRANSFER_AMOUNT
        );
    }

    /*
     * A transação é assinada usando
     * exatamente o nonce confirmado
     * descoberto na blockchain.
     */
    let transaction = Transaction::signed(
        &wallet,
        recipient_address.clone(),
        TEST_TRANSFER_AMOUNT,
        0,
        sender_nonce,
    )
    .unwrap_or_else(|error| {
        panic!("falha ao criar transação: {}", error);
    });

    transaction.validate().unwrap_or_else(|error| {
        panic!("transação recém-criada falhou na validação: {}", error);
    });

    let transaction_hash = transaction.hash();

    println!();

    println!("========================================");
    println!("       TRANSACTION CRIADA");
    println!("========================================");

    println!("De: {}", sender_address);

    println!("Para: {}", recipient_address);

    println!("Valor: {} TPZ", TEST_TRANSFER_AMOUNT);

    println!("Nonce: {}", sender_nonce);

    println!("Hash: {}", hash_to_hex(&transaction_hash));

    println!("Assinatura: OK");

    /*
     * Identidade P2P independente.
     *
     * Isso impede que o handshake
     * seja identificado como uma
     * conexão do nó consigo mesmo.
     */
    let p2p_identity = load_or_create_p2p_identity();

    let p2p_node_id = p2p_identity.address();

    println!();

    println!("Identidade P2P do emissor:");

    println!("  Arquivo: {}", P2P_IDENTITY_PATH);

    println!("  Node ID: {}", p2p_node_id);

    if p2p_node_id == sender_address {
        panic!("identidade P2P do emissor coincide com a identidade econômica do servidor");
    }

    println!();

    println!("Conectando ao servidor: {}", SERVER_ADDRESS);

    let mut stream = TcpStream::connect(SERVER_ADDRESS).unwrap_or_else(|error| {
        panic!(
            "não foi possível conectar ao servidor {}: {}",
            SERVER_ADDRESS, error
        );
    });

    configure_stream(&stream).unwrap_or_else(|error| {
        panic!("falha ao configurar conexão TCP: {}", error);
    });

    println!("TCP: OK");

    /*
     * Handshake usa a identidade P2P
     * independente.
     */
    let local_handshake = Handshake::new(p2p_node_id.clone(), tip.height, tip.hash())
        .unwrap_or_else(|error| {
            panic!("falha ao criar handshake: {}", error);
        });

    send_handshake(&mut stream, &local_handshake).unwrap_or_else(|error| {
        panic!("falha ao enviar handshake: {}", error);
    });

    let remote_handshake = receive_handshake(&mut stream).unwrap_or_else(|error| {
        panic!("falha ao receber handshake do servidor: {}", error);
    });

    local_handshake
        .validate_peer(&remote_handshake)
        .unwrap_or_else(|error| {
            panic!("servidor rejeitado pelo handshake: {}", error);
        });

    println!();

    println!("========================================");
    println!("          HANDSHAKE OK");
    println!("========================================");

    println!("Servidor Node ID: {}", remote_handshake.node_id);

    println!("Altura do servidor: {}", remote_handshake.height);

    println!(
        "Tip do servidor: {}",
        hash_to_hex(&remote_handshake.tip_hash)
    );

    /*
     * IMPORTANTE:
     *
     * Um peer já sincronizado recebe
     * imediatamente um AnnounceTip
     * espontâneo do servidor.
     *
     * Precisamos consumir essa mensagem
     * antes de transmitir a Transaction.
     */
    println!();

    println!("Aguardando mensagem inicial do servidor...");

    let initial_message = receive_sync_message(&mut stream).unwrap_or_else(|error| {
        panic!("falha ao receber mensagem inicial do servidor: {}", error);
    });

    match initial_message {
        SyncMessage::AnnounceTip { height, tip_hash } => {
            println!("AnnounceTip inicial: OK");

            println!("  Altura anunciada: {}", height);

            println!("  Tip anunciado: {}", hash_to_hex(&tip_hash));

            if height != remote_handshake.height || tip_hash != remote_handshake.tip_hash {
                panic!("AnnounceTip inicial não corresponde ao handshake do servidor");
            }
        }

        SyncMessage::GetBlocks { .. } => {
            panic!("servidor enviou GetBlocks inesperadamente");
        }

        SyncMessage::Blocks { .. } => {
            panic!("servidor enviou Blocks inesperadamente");
        }

        SyncMessage::Transaction { .. } => {
            panic!("servidor enviou Transaction inesperadamente");
        }

        SyncMessage::TransactionAck { .. } => {
            panic!("servidor enviou TransactionAck inesperadamente");
        }
    }

    /*
     * Agora a conexão está alinhada
     * com o protocolo persistente.
     */
    let message = SyncMessage::transaction(transaction).unwrap_or_else(|error| {
        panic!("protocolo recusou a transação: {}", error);
    });

    println!();

    println!("Enviando Transaction P2P...");

    send_sync_message(&mut stream, &message).unwrap_or_else(|error| {
        panic!("falha ao enviar Transaction P2P: {}", error);
    });

    /*
     * Fazemos shutdown somente da metade
     * de escrita do socket.
     *
     * O TCP entrega todos os bytes já
     * enviados antes do FIN.
     *
     * Assim o servidor consegue consumir
     * completamente a Transaction antes
     * de detectar o encerramento.
     */
    stream.shutdown(Shutdown::Write).unwrap_or_else(|error| {
        panic!("falha ao finalizar escrita TCP: {}", error);
    });

    println!();

    println!("========================================");
    println!("        TRANSACTION ENVIADA");
    println!("========================================");

    println!("TCP: OK");

    println!("Handshake: OK");

    println!("AnnounceTip inicial: OK");

    println!("Assinatura: OK");

    println!("Protocolo Transaction: OK");

    println!("Valor: {} TPZ", TEST_TRANSFER_AMOUNT);

    println!("Hash: {}", hash_to_hex(&transaction_hash));

    println!();

    println!("Escrita TCP encerrada de forma ordenada.");

    println!("Confira agora o terminal do servidor.");
}

fn load_or_create_p2p_identity() -> NodeIdentity {
    let path = Path::new(P2P_IDENTITY_PATH);

    if path.exists() {
        println!();

        println!("Identidade P2P persistente encontrada.");

        return NodeIdentity::load(path).unwrap_or_else(|error| {
            panic!(
                "falha ao carregar identidade P2P {}: {}",
                P2P_IDENTITY_PATH, error
            );
        });
    }

    println!();

    println!("Criando identidade P2P independente...");

    let identity = NodeIdentity::generate();

    identity.save(path).unwrap_or_else(|error| {
        panic!(
            "falha ao salvar identidade P2P {}: {}",
            P2P_IDENTITY_PATH, error
        );
    });

    println!("Identidade P2P criada: OK");

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

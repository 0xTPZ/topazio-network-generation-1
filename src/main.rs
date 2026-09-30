use std::path::Path;

use topazio_network::{
    NETWORK_NAME, NETWORK_SYMBOL, NETWORK_VERSION,
    block::Block,
    blockchain::Blockchain,
    consensus::block_reward,
    crypto::hash_to_hex,
    genesis::{GENESIS_TIMESTAMP, genesis_hash_hex, verify_genesis},
    identity::KeyPair,
    mempool::Mempool,
    mining::mine_block,
    node::NodeIdentity,
    storage::{load_blockchain, save_blockchain},
    transaction::{Transaction, sum_fees},
};

const STORAGE_PATH: &str = "data/topazio-chain-lab.dat";

const NODE_IDENTITY_PATH: &str = "data/topazio-node-identity-lab.dat";

const MEMPOOL_PATH: &str = "data/topazio-mempool-lab.dat";

const DEMO_TRANSFER_AMOUNT: u64 = 25;

const DEMO_BOB_TARGET_BALANCE: u64 = 75;

fn main() {
    println!("========================================");

    println!("        TOPAZIO NETWORK");

    println!("========================================");

    println!("Rede: {}", NETWORK_NAME);

    println!("Símbolo: {}", NETWORK_SYMBOL);

    println!("Versão: {}", NETWORK_VERSION);

    println!("Status: laboratório");

    println!();
    println!("Genesis:");

    println!("  Timestamp: {}", GENESIS_TIMESTAMP);

    println!("  Hash: {}", genesis_hash_hex());

    println!(
        "  Verificação: {}",
        if verify_genesis() { "OK" } else { "FALHOU" }
    );

    let mut blockchain = load_or_create_blockchain();

    let node_identity = load_or_create_node_identity();

    let miner = node_identity.key_pair();

    let bob = KeyPair::from_secret_bytes([9u8; 32]);

    println!();
    println!("Identidade do nó:");

    println!("  Arquivo: {}", NODE_IDENTITY_PATH);

    println!("  Endereço minerador: {}", miner.address());

    println!();
    println!("Blockchain carregada:");

    println!("  Blocos: {}", blockchain.len());

    println!("  Altura atual: {}", blockchain.tip().height);

    println!(
        "  Supply atual: {} TPZ",
        blockchain
            .total_supply()
            .expect("failed to calculate supply",)
    );

    println!(
        "  Saldo minerador: {} TPZ",
        blockchain.state().balance_of(&miner.address())
    );

    println!(
        "  Saldo Bob: {} TPZ",
        blockchain.state().balance_of(&bob.address())
    );

    println!(
        "  Nonce minerador: {}",
        blockchain.state().nonce_of(&miner.address())
    );

    println!(
        "  Validação: {}",
        if blockchain.validate().is_ok() {
            "OK"
        } else {
            "FALHOU"
        }
    );

    /*
     * MEMPOOL PERSISTENTE
     */

    let mempool_existed = Path::new(MEMPOOL_PATH).exists();

    let mut mempool = load_or_create_mempool(blockchain.state());

    println!();
    println!("Mempool:");

    println!("  Arquivo: {}", MEMPOOL_PATH);

    println!("  Pendentes carregadas: {}", mempool.len());

    /*
     * Demonstração de laboratório.
     *
     * Só cria a transação quando ainda
     * não existe arquivo de mempool.
     */
    if !mempool_existed {
        let bob_balance = blockchain.state().balance_of(&bob.address());

        let miner_balance = blockchain.state().balance_of(&miner.address());

        if bob_balance < DEMO_BOB_TARGET_BALANCE && miner_balance >= DEMO_TRANSFER_AMOUNT {
            let sender_nonce = blockchain.state().nonce_of(&miner.address());

            let transfer =
                Transaction::signed(&miner, bob.address(), DEMO_TRANSFER_AMOUNT, 0, sender_nonce)
                    .expect("failed to create demo transaction");

            println!("  Criando transação persistente");

            println!("  Valor: {} TPZ", DEMO_TRANSFER_AMOUNT);

            println!("  Destino: {}", bob.address());

            println!("  Nonce: {}", sender_nonce);

            println!("  Hash: {}", hash_to_hex(&transfer.hash()));

            mempool
                .add_transaction(transfer, blockchain.state())
                .expect("mempool rejected valid demo transaction");

            mempool.save(MEMPOOL_PATH).expect("failed to save mempool");

            println!("  Entrada no mempool: OK");

            println!("  Persistência do mempool: OK");

            println!("  Pendentes salvas: {}", mempool.len());

            println!();

            println!("TESTE DE REINICIALIZAÇÃO:");

            println!("  A transação foi salva, mas NÃO foi minerada.");

            println!("  Execute cargo run novamente.");

            return;
        }

        mempool
            .save(MEMPOOL_PATH)
            .expect("failed to save empty mempool");
    }

    if !mempool.is_empty() {
        println!("  Reinicialização confirmada");

        println!("  Transações pendentes sobreviveram ao fechamento do nó");
    } else {
        println!("  Nenhuma transação pendente para minerar");
    }

    /*
     * MONTAGEM DO BLOCO
     */

    let next_height = blockchain.next_height();

    let subsidy = block_reward(next_height);
    let coinbase = Transaction::coinbase(miner.address(), subsidy, next_height)
        .expect("failed to create coinbase");

    let mut block_transactions = mempool
        .transactions_for_block(coinbase, blockchain.state())
        .expect("failed to build block transactions");
    let fees = sum_fees(&block_transactions).expect("failed to sum block fees");
    let coinbase_value = subsidy
        .checked_add(fees)
        .expect("subsidy plus fees overflow");
    block_transactions[0] = Transaction::coinbase(miner.address(), coinbase_value, next_height)
        .expect("failed to create fee-paying coinbase");

    println!();

    println!("Montagem do bloco #{}:", next_height);

    println!("  Coinbase: 1");

    println!("  Transações do mempool: {}", mempool.len());

    println!("  Total de transações: {}", block_transactions.len());

    let timestamp = GENESIS_TIMESTAMP
        .checked_add(next_height)
        .expect("block timestamp overflow");

    let candidate = Block::with_transactions(
        next_height,
        blockchain.tip().hash(),
        timestamp,
        0,
        &block_transactions,
    )
    .expect("failed to create block");

    println!();

    println!("Minerando bloco #{}:", next_height);

    println!("  Subsídio: {} unidades atômicas", subsidy);
    println!("  Taxas: {} unidades atômicas", fees);
    println!("  Coinbase: {} unidades atômicas", coinbase_value);

    let mined = mine_block(candidate, blockchain.difficulty_bits(), 1_000_000)
        .expect("failed to mine block");

    println!("  Nonce: {}", mined.nonce);

    println!("  Hash: {}", hash_to_hex(&mined.hash()));

    blockchain
        .append_block(mined)
        .expect("failed to append block");

    mempool.remove_confirmed(&block_transactions);

    println!();
    println!("Confirmação:");

    println!("  Bloco aceito: OK");

    println!("  Pendentes restantes: {}", mempool.len());

    save_blockchain(&blockchain, STORAGE_PATH).expect("failed to save blockchain");

    mempool
        .save(MEMPOOL_PATH)
        .expect("failed to save updated mempool");

    println!();
    println!("Persistência:");

    println!("  Blockchain: {}", STORAGE_PATH);

    println!("  Mempool: {}", MEMPOOL_PATH);

    println!("  Identidade: {}", NODE_IDENTITY_PATH);

    println!("  Salvamento: OK");

    println!();
    println!("Estado final:");

    println!("  Blocos: {}", blockchain.len());

    println!("  Altura atual: {}", blockchain.tip().height);

    println!(
        "  Saldo minerador: {} TPZ",
        blockchain.state().balance_of(&miner.address())
    );

    println!(
        "  Saldo Bob: {} TPZ",
        blockchain.state().balance_of(&bob.address())
    );

    println!(
        "  Nonce minerador: {}",
        blockchain.state().nonce_of(&miner.address())
    );

    println!(
        "  Supply total: {} TPZ",
        blockchain
            .total_supply()
            .expect("failed to calculate supply",)
    );

    println!("  Mempool pendente: {}", mempool.len());

    println!(
        "  Validação completa: {}",
        if blockchain.validate().is_ok() {
            "OK"
        } else {
            "FALHOU"
        }
    );
}

fn load_or_create_blockchain() -> Blockchain {
    let path = Path::new(STORAGE_PATH);

    if path.exists() {
        println!();
        println!("Persistência:");

        println!("  Blockchain encontrada");

        match load_blockchain(path) {
            Ok(blockchain) => {
                println!("  Carregamento: OK");

                println!("  Continuidade: bloco #{}", blockchain.tip().height);

                blockchain
            }

            Err(error) => {
                panic!("arquivo da blockchain existe, mas é inválido: {}", error);
            }
        }
    } else {
        println!();
        println!("Persistência:");

        println!("  Nenhuma blockchain encontrada");

        println!("  Criando nova cadeia");

        let blockchain = Blockchain::new();

        println!("  Genesis criado: OK");

        blockchain
    }
}

fn load_or_create_mempool(confirmed_state: &topazio_network::state::State) -> Mempool {
    let path = Path::new(MEMPOOL_PATH);

    if path.exists() {
        println!();

        println!("Mempool persistente:");

        println!("  Arquivo encontrado");

        let mempool = Mempool::load(path, confirmed_state).unwrap_or_else(|error| {
            panic!("arquivo do mempool existe, mas é inválido: {}", error);
        });

        println!("  Carregamento: OK");

        return mempool;
    }

    println!();

    println!("Mempool persistente:");

    println!("  Nenhum arquivo encontrado");

    println!("  Criando mempool vazio");

    Mempool::new()
}

/*
 * Não existe mais segredo fixo no código.
 *
 * Se já existe identidade, carregamos.
 *
 * Em uma instalação nova, geramos uma
 * identidade criptográfica independente
 * e persistimos no disco.
 */
fn load_or_create_node_identity() -> NodeIdentity {
    let path = Path::new(NODE_IDENTITY_PATH);

    if path.exists() {
        println!();

        println!("Identidade persistente:");

        println!("  Arquivo encontrado");

        let identity = NodeIdentity::load(path).unwrap_or_else(|error| {
            panic!(
                "arquivo da identidade do nó existe, mas é inválido: {}",
                error
            );
        });

        println!("  Carregamento: OK");

        return identity;
    }

    println!();

    println!("Identidade persistente:");

    println!("  Nenhuma identidade encontrada");

    println!("  Gerando nova identidade aleatória");

    let identity = NodeIdentity::generate();

    identity.save(path).unwrap_or_else(|error| {
        panic!("failed to persist node identity: {}", error);
    });

    println!("  Salvamento: OK");

    identity
}

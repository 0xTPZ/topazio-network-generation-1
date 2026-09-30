use std::{
    io::{Read, Write},
    net::TcpStream,
};

use crate::{
    block::Block,
    blockchain::Blockchain,
    crypto::Hash,
    transaction::{MAX_TRANSACTION_SIZE, Transaction},
};

// Transaction payloads in sync messages use transaction format v2.
pub const SYNC_PROTOCOL_VERSION: u16 = 2;

pub const MAX_BLOCKS_PER_REQUEST: u16 = 128;

/*
 * Motivo textual retornado em TransactionAck.
 *
 * Mantemos um limite pequeno porque essa mensagem
 * circula diretamente entre peers e nunca devemos
 * confiar em um tamanho arbitrário vindo da rede.
 */
pub const MAX_TRANSACTION_ACK_REASON_BYTES: usize = 1024;

const SYNC_MAGIC: &[u8; 8] = b"TPZSYNC1";

const MESSAGE_GET_BLOCKS: u8 = 1;

const MESSAGE_BLOCKS: u8 = 2;

const MESSAGE_ANNOUNCE_TIP: u8 = 3;

const MESSAGE_TRANSACTION: u8 = 4;

const MESSAGE_TRANSACTION_ACK: u8 = 5;

const MAX_SYNC_MESSAGE_SIZE: usize = 16 * 1024 * 1024;

const MAX_ENCODED_BLOCK_SIZE: usize = 1024 * 1024;

const MAX_ENCODED_TRANSACTION_SIZE: usize = MAX_TRANSACTION_SIZE;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncMessage {
    GetBlocks {
        start_height: u64,
        limit: u16,
    },

    Blocks {
        blocks: Vec<Block>,
    },

    AnnounceTip {
        height: u64,
        tip_hash: Hash,
    },

    /*
     * Transação pendente propagada
     * entre peers da rede.
     *
     * Coinbase NÃO pode circular através
     * desta mensagem.
     *
     * Coinbase nasce exclusivamente
     * durante a montagem de um bloco.
     */
    Transaction {
        transaction: Transaction,
    },

    /*
     * Confirmação protocolar relacionada
     * a uma Transaction recebida.
     *
     * O hash identifica exatamente qual
     * transação está sendo confirmada.
     *
     * accepted = true:
     *   a transação foi aceita pelo peer.
     *
     * accepted = false:
     *   a transação foi rejeitada.
     *
     * reason contém uma descrição curta,
     * limitada por MAX_TRANSACTION_ACK_REASON_BYTES.
     */
    TransactionAck {
        transaction_hash: Hash,
        accepted: bool,
        reason: String,
    },
}

impl SyncMessage {
    pub fn get_blocks(start_height: u64, limit: u16) -> Result<Self, String> {
        validate_request_limit(limit)?;

        Ok(Self::GetBlocks {
            start_height,
            limit,
        })
    }

    pub fn blocks(blocks: Vec<Block>) -> Result<Self, String> {
        validate_block_count(blocks.len())?;

        Ok(Self::Blocks { blocks })
    }

    pub fn announce_tip(height: u64, tip_hash: Hash) -> Self {
        Self::AnnounceTip { height, tip_hash }
    }

    pub fn transaction(transaction: Transaction) -> Result<Self, String> {
        validate_network_transaction(&transaction)?;

        Ok(Self::Transaction { transaction })
    }

    pub fn transaction_ack(
        transaction_hash: Hash,
        accepted: bool,
        reason: impl Into<String>,
    ) -> Result<Self, String> {
        let reason = reason.into();

        validate_transaction_ack_reason(&reason)?;

        Ok(Self::TransactionAck {
            transaction_hash,
            accepted,
            reason,
        })
    }

    pub fn encode(&self) -> Result<Vec<u8>, String> {
        let mut bytes = Vec::new();

        bytes.extend_from_slice(SYNC_MAGIC);

        bytes.extend_from_slice(&SYNC_PROTOCOL_VERSION.to_le_bytes());

        match self {
            Self::GetBlocks {
                start_height,
                limit,
            } => {
                validate_request_limit(*limit)?;

                bytes.push(MESSAGE_GET_BLOCKS);

                bytes.extend_from_slice(&start_height.to_le_bytes());

                bytes.extend_from_slice(&limit.to_le_bytes());
            }

            Self::Blocks { blocks } => {
                validate_block_count(blocks.len())?;

                bytes.push(MESSAGE_BLOCKS);

                let count = u16::try_from(blocks.len())
                    .map_err(|_| "too many blocks to encode".to_string())?;

                bytes.extend_from_slice(&count.to_le_bytes());

                for block in blocks {
                    let encoded = block.encode();

                    if encoded.len() > MAX_ENCODED_BLOCK_SIZE {
                        return Err("encoded block exceeds maximum size".to_string());
                    }

                    let length = u32::try_from(encoded.len())
                        .map_err(|_| "encoded block is too large".to_string())?;

                    bytes.extend_from_slice(&length.to_le_bytes());

                    bytes.extend_from_slice(&encoded);
                }
            }

            Self::AnnounceTip { height, tip_hash } => {
                bytes.push(MESSAGE_ANNOUNCE_TIP);

                bytes.extend_from_slice(&height.to_le_bytes());

                bytes.extend_from_slice(tip_hash);
            }

            Self::Transaction { transaction } => {
                validate_network_transaction(transaction)?;

                let encoded = transaction.encode();

                if encoded.len() > MAX_ENCODED_TRANSACTION_SIZE {
                    return Err(format!(
                        "encoded transaction exceeds maximum size of {} bytes",
                        MAX_ENCODED_TRANSACTION_SIZE
                    ));
                }

                let length = u32::try_from(encoded.len())
                    .map_err(|_| "encoded transaction is too large".to_string())?;

                bytes.push(MESSAGE_TRANSACTION);

                bytes.extend_from_slice(&length.to_le_bytes());

                bytes.extend_from_slice(&encoded);
            }

            Self::TransactionAck {
                transaction_hash,
                accepted,
                reason,
            } => {
                validate_transaction_ack_reason(reason)?;

                let reason_bytes = reason.as_bytes();

                let reason_length = u16::try_from(reason_bytes.len())
                    .map_err(|_| "transaction ack reason is too large".to_string())?;

                bytes.push(MESSAGE_TRANSACTION_ACK);

                bytes.extend_from_slice(transaction_hash);

                bytes.push(if *accepted { 1 } else { 0 });

                bytes.extend_from_slice(&reason_length.to_le_bytes());

                bytes.extend_from_slice(reason_bytes);
            }
        }

        if bytes.len() > MAX_SYNC_MESSAGE_SIZE {
            return Err("sync message exceeds maximum size".to_string());
        }

        Ok(bytes)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_SYNC_MESSAGE_SIZE {
            return Err("sync message exceeds maximum size".to_string());
        }

        let mut cursor = Cursor::new(bytes);

        let magic = cursor.read_exact(SYNC_MAGIC.len())?;

        if magic != SYNC_MAGIC {
            return Err("invalid Topazio sync magic".to_string());
        }

        let version = cursor.read_u16()?;

        if version != SYNC_PROTOCOL_VERSION {
            return Err(format!("unsupported sync protocol version: {}", version));
        }

        let message_type = cursor.read_u8()?;

        let message = match message_type {
            MESSAGE_GET_BLOCKS => {
                let start_height = cursor.read_u64()?;

                let limit = cursor.read_u16()?;

                validate_request_limit(limit)?;

                Self::GetBlocks {
                    start_height,
                    limit,
                }
            }

            MESSAGE_BLOCKS => {
                let count = cursor.read_u16()? as usize;

                validate_block_count(count)?;

                let mut blocks = Vec::with_capacity(count);

                for _ in 0..count {
                    let block_length = cursor.read_u32()? as usize;

                    if block_length == 0 {
                        return Err("empty encoded block".to_string());
                    }

                    if block_length > MAX_ENCODED_BLOCK_SIZE {
                        return Err("encoded block exceeds maximum size".to_string());
                    }

                    let block_bytes = cursor.read_exact(block_length)?;

                    let block = Block::decode(block_bytes)
                        .map_err(|error| format!("invalid block in sync message: {}", error))?;

                    blocks.push(block);
                }

                Self::Blocks { blocks }
            }

            MESSAGE_ANNOUNCE_TIP => {
                let height = cursor.read_u64()?;

                let hash_bytes = cursor.read_exact(32)?;

                let mut tip_hash = [0u8; 32];

                tip_hash.copy_from_slice(hash_bytes);

                Self::AnnounceTip { height, tip_hash }
            }

            MESSAGE_TRANSACTION => {
                let transaction_length = cursor.read_u32()? as usize;

                if transaction_length == 0 {
                    return Err("empty encoded transaction".to_string());
                }

                if transaction_length > MAX_ENCODED_TRANSACTION_SIZE {
                    return Err(format!(
                        "encoded transaction exceeds maximum size of {} bytes",
                        MAX_ENCODED_TRANSACTION_SIZE
                    ));
                }

                let transaction_bytes = cursor.read_exact(transaction_length)?;

                let transaction = Transaction::decode(transaction_bytes)
                    .map_err(|error| format!("invalid transaction in sync message: {}", error))?;

                validate_network_transaction(&transaction)?;

                Self::Transaction { transaction }
            }

            MESSAGE_TRANSACTION_ACK => {
                let hash_bytes = cursor.read_exact(32)?;

                let mut transaction_hash = [0u8; 32];

                transaction_hash.copy_from_slice(hash_bytes);

                let accepted_byte = cursor.read_u8()?;

                let accepted = match accepted_byte {
                    0 => false,
                    1 => true,
                    _ => {
                        return Err(format!(
                            "invalid transaction ack accepted flag: {}",
                            accepted_byte
                        ));
                    }
                };

                let reason_length = cursor.read_u16()? as usize;

                if reason_length > MAX_TRANSACTION_ACK_REASON_BYTES {
                    return Err(format!(
                        "transaction ack reason exceeds maximum size of {} bytes",
                        MAX_TRANSACTION_ACK_REASON_BYTES
                    ));
                }

                let reason_bytes = cursor.read_exact(reason_length)?;

                let reason = std::str::from_utf8(reason_bytes)
                    .map_err(|_| "transaction ack reason is not valid UTF-8".to_string())?
                    .to_string();

                validate_transaction_ack_reason(&reason)?;

                Self::TransactionAck {
                    transaction_hash,
                    accepted,
                    reason,
                }
            }

            _ => {
                return Err(format!("unknown sync message type: {}", message_type));
            }
        };

        if !cursor.is_finished() {
            return Err("sync message contains trailing bytes".to_string());
        }

        Ok(message)
    }
}

/*
 * Frame TCP:
 *
 * [ tamanho u32 ][ SyncMessage ]
 *
 * O tamanho recebido nunca é confiado
 * antes de ser comparado com o limite.
 */
pub fn send_sync_message(stream: &mut TcpStream, message: &SyncMessage) -> Result<(), String> {
    let payload = message.encode()?;

    if payload.len() > MAX_SYNC_MESSAGE_SIZE {
        return Err("sync message exceeds maximum size".to_string());
    }

    let length = u32::try_from(payload.len())
        .map_err(|_| "sync message is too large for TCP frame".to_string())?;

    stream
        .write_all(&length.to_le_bytes())
        .map_err(|error| format!("failed to write sync frame length: {}", error))?;

    stream
        .write_all(&payload)
        .map_err(|error| format!("failed to write sync message: {}", error))?;

    stream
        .flush()
        .map_err(|error| format!("failed to flush sync message: {}", error))?;

    Ok(())
}

pub fn receive_sync_message(stream: &mut TcpStream) -> Result<SyncMessage, String> {
    let mut length_bytes = [0u8; 4];

    stream
        .read_exact(&mut length_bytes)
        .map_err(|error| format!("failed to read sync frame length: {}", error))?;

    let length = u32::from_le_bytes(length_bytes) as usize;

    if length == 0 {
        return Err("empty sync TCP frame".to_string());
    }

    if length > MAX_SYNC_MESSAGE_SIZE {
        return Err(format!("sync TCP frame exceeds maximum size: {}", length));
    }

    let mut payload = vec![0u8; length];

    stream
        .read_exact(&mut payload)
        .map_err(|error| format!("failed to read sync message: {}", error))?;

    SyncMessage::decode(&payload)
}

/*
 * Seleciona uma faixa de blocos
 * usando a altura como índice.
 */
pub fn blocks_for_request(
    blockchain: &Blockchain,
    start_height: u64,
    limit: u16,
) -> Result<Vec<Block>, String> {
    validate_request_limit(limit)?;

    let start = usize::try_from(start_height)
        .map_err(|_| "requested height cannot be represented locally".to_string())?;

    let blocks = blockchain.blocks();

    if start >= blocks.len() {
        return Ok(Vec::new());
    }

    let requested_end = start
        .checked_add(limit as usize)
        .ok_or_else(|| "block request range overflow".to_string())?;

    let end = requested_end.min(blocks.len());

    Ok(blocks[start..end].to_vec())
}

/*
 * Validação atômica do lote.
 *
 * Os blocos recebidos NÃO são simplesmente
 * inseridos na cadeia ativa.
 *
 * Primeiro construímos uma cadeia candidata
 * completa e Blockchain::from_blocks()
 * reexecuta todas as regras:
 *
 * - genesis
 * - alturas
 * - previous hash
 * - PoW
 * - coinbase
 * - transações
 * - assinaturas
 * - saldos
 * - nonces
 * - supply
 *
 * Somente depois de tudo ser aprovado
 * substituímos a blockchain ativa.
 */
pub fn apply_block_batch(
    blockchain: &mut Blockchain,
    received_blocks: Vec<Block>,
) -> Result<usize, String> {
    if received_blocks.is_empty() {
        return Ok(0);
    }

    let expected_height = blockchain.next_height();

    if received_blocks[0].height != expected_height {
        return Err(format!(
            "unexpected first block height: expected {}, received {}",
            expected_height, received_blocks[0].height
        ));
    }

    for window in received_blocks.windows(2) {
        let previous = &window[0];

        let current = &window[1];

        let expected = previous
            .height
            .checked_add(1)
            .ok_or_else(|| "block height overflow".to_string())?;

        if current.height != expected {
            return Err(format!(
                "non-contiguous block batch: expected height {}, received {}",
                expected, current.height
            ));
        }
    }

    let count = received_blocks.len();

    let mut candidate_blocks = blockchain.blocks().to_vec();

    candidate_blocks.extend(received_blocks);

    let rebuilt = Blockchain::from_blocks(blockchain.difficulty_bits(), candidate_blocks).map_err(
        |error| {
            format!(
                "received block batch failed blockchain validation: {}",
                error
            )
        },
    )?;

    *blockchain = rebuilt;

    Ok(count)
}

fn validate_network_transaction(transaction: &Transaction) -> Result<(), String> {
    if transaction.is_coinbase() {
        return Err(
            "coinbase transaction cannot be propagated through P2P transaction message".to_string(),
        );
    }

    transaction
        .validate()
        .map_err(|error| format!("invalid network transaction: {}", error))?;

    let encoded_size = transaction.encode().len();

    if encoded_size > MAX_ENCODED_TRANSACTION_SIZE {
        return Err(format!(
            "encoded transaction exceeds maximum size of {} bytes",
            MAX_ENCODED_TRANSACTION_SIZE
        ));
    }

    Ok(())
}

fn validate_transaction_ack_reason(reason: &str) -> Result<(), String> {
    if reason.as_bytes().len() > MAX_TRANSACTION_ACK_REASON_BYTES {
        return Err(format!(
            "transaction ack reason exceeds maximum size of {} bytes",
            MAX_TRANSACTION_ACK_REASON_BYTES
        ));
    }

    Ok(())
}

fn validate_request_limit(limit: u16) -> Result<(), String> {
    if limit == 0 {
        return Err("block request limit cannot be zero".to_string());
    }

    if limit > MAX_BLOCKS_PER_REQUEST {
        return Err(format!(
            "block request exceeds maximum limit of {}",
            MAX_BLOCKS_PER_REQUEST
        ));
    }

    Ok(())
}

fn validate_block_count(count: usize) -> Result<(), String> {
    if count > MAX_BLOCKS_PER_REQUEST as usize {
        return Err(format!(
            "block response exceeds maximum limit of {}",
            MAX_BLOCKS_PER_REQUEST
        ));
    }

    Ok(())
}

struct Cursor<'a> {
    bytes: &'a [u8],
    position: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, position: 0 }
    }

    fn read_exact(&mut self, length: usize) -> Result<&'a [u8], String> {
        let end = self
            .position
            .checked_add(length)
            .ok_or_else(|| "sync cursor overflow".to_string())?;

        if end > self.bytes.len() {
            return Err("sync message is truncated".to_string());
        }

        let slice = &self.bytes[self.position..end];

        self.position = end;

        Ok(slice)
    }

    fn read_u8(&mut self) -> Result<u8, String> {
        Ok(self.read_exact(1)?[0])
    }

    fn read_u16(&mut self) -> Result<u16, String> {
        let bytes = self.read_exact(2)?;

        let mut value = [0u8; 2];

        value.copy_from_slice(bytes);

        Ok(u16::from_le_bytes(value))
    }

    fn read_u32(&mut self) -> Result<u32, String> {
        let bytes = self.read_exact(4)?;

        let mut value = [0u8; 4];

        value.copy_from_slice(bytes);

        Ok(u32::from_le_bytes(value))
    }

    fn read_u64(&mut self) -> Result<u64, String> {
        let bytes = self.read_exact(8)?;

        let mut value = [0u8; 8];

        value.copy_from_slice(bytes);

        Ok(u64::from_le_bytes(value))
    }

    fn is_finished(&self) -> bool {
        self.position == self.bytes.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::{
        net::{TcpListener, TcpStream},
        thread,
    };

    use crate::{
        blockchain::Blockchain, genesis::create_genesis_block, identity::KeyPair,
        transaction::Transaction,
    };

    fn alice_key() -> KeyPair {
        KeyPair::from_secret_bytes([7u8; 32])
    }

    fn bob_key() -> KeyPair {
        KeyPair::from_secret_bytes([9u8; 32])
    }

    fn sample_transfer() -> Transaction {
        let alice = alice_key();

        let bob = bob_key();

        Transaction::signed(&alice, bob.address(), 25, 0, 0).unwrap()
    }

    #[test]
    fn get_blocks_message_can_be_created() {
        let message = SyncMessage::get_blocks(1, 32).unwrap();

        assert_eq!(
            message,
            SyncMessage::GetBlocks {
                start_height: 1,
                limit: 32,
            }
        );
    }

    #[test]
    fn get_blocks_round_trip() {
        let original = SyncMessage::get_blocks(5, 64).unwrap();

        let encoded = original.encode().unwrap();

        let decoded = SyncMessage::decode(&encoded).unwrap();

        assert_eq!(original, decoded);
    }

    #[test]
    fn blocks_round_trip() {
        let genesis = create_genesis_block();

        let original = SyncMessage::blocks(vec![genesis.clone()]).unwrap();

        let encoded = original.encode().unwrap();

        let decoded = SyncMessage::decode(&encoded).unwrap();

        assert_eq!(
            decoded,
            SyncMessage::Blocks {
                blocks: vec![genesis],
            }
        );
    }

    #[test]
    fn empty_blocks_response_is_valid() {
        let original = SyncMessage::blocks(Vec::new()).unwrap();

        let encoded = original.encode().unwrap();

        let decoded = SyncMessage::decode(&encoded).unwrap();

        assert_eq!(decoded, SyncMessage::Blocks { blocks: Vec::new() });
    }

    #[test]
    fn announce_tip_can_be_created() {
        let hash = [7u8; 32];

        let message = SyncMessage::announce_tip(42, hash);

        assert_eq!(
            message,
            SyncMessage::AnnounceTip {
                height: 42,
                tip_hash: hash,
            }
        );
    }

    #[test]
    fn announce_tip_round_trip() {
        let hash = [9u8; 32];

        let original = SyncMessage::announce_tip(123, hash);

        let encoded = original.encode().unwrap();

        let decoded = SyncMessage::decode(&encoded).unwrap();

        assert_eq!(original, decoded);
    }

    #[test]
    fn announce_tip_preserves_height_and_hash() {
        let hash = [0xAB; 32];

        let message = SyncMessage::announce_tip(u64::MAX - 1, hash);

        let encoded = message.encode().unwrap();

        let decoded = SyncMessage::decode(&encoded).unwrap();

        match decoded {
            SyncMessage::AnnounceTip { height, tip_hash } => {
                assert_eq!(height, u64::MAX - 1);

                assert_eq!(tip_hash, hash);
            }

            _ => {
                panic!("expected AnnounceTip");
            }
        }
    }

    #[test]
    fn truncated_announce_tip_is_rejected() {
        let message = SyncMessage::announce_tip(10, [3u8; 32]);

        let encoded = message.encode().unwrap();

        let truncated = &encoded[..encoded.len() - 1];

        assert!(SyncMessage::decode(truncated).is_err());
    }

    #[test]
    fn announce_tip_with_trailing_bytes_is_rejected() {
        let message = SyncMessage::announce_tip(10, [4u8; 32]);

        let mut encoded = message.encode().unwrap();

        encoded.push(99);

        assert!(SyncMessage::decode(&encoded).is_err());
    }

    #[test]
    fn transaction_message_can_be_created() {
        let transaction = sample_transfer();

        let message = SyncMessage::transaction(transaction.clone()).unwrap();

        assert_eq!(message, SyncMessage::Transaction { transaction });
    }

    #[test]
    fn transaction_message_round_trip() {
        let transaction = sample_transfer();

        let original = SyncMessage::transaction(transaction).unwrap();

        let encoded = original.encode().unwrap();

        let decoded = SyncMessage::decode(&encoded).unwrap();

        assert_eq!(original, decoded);
    }

    #[test]
    fn coinbase_transaction_message_is_rejected() {
        let alice = alice_key();

        let coinbase = Transaction::coinbase(alice.address(), 50, 1).unwrap();

        assert!(SyncMessage::transaction(coinbase).is_err());
    }

    #[test]
    fn truncated_transaction_message_is_rejected() {
        let message = SyncMessage::transaction(sample_transfer()).unwrap();

        let encoded = message.encode().unwrap();

        let truncated = &encoded[..encoded.len() - 1];

        assert!(SyncMessage::decode(truncated).is_err());
    }

    #[test]
    fn transaction_message_with_trailing_bytes_is_rejected() {
        let message = SyncMessage::transaction(sample_transfer()).unwrap();

        let mut encoded = message.encode().unwrap();

        encoded.push(99);

        assert!(SyncMessage::decode(&encoded).is_err());
    }

    #[test]
    fn oversized_transaction_payload_is_rejected() {
        let mut encoded = Vec::new();

        encoded.extend_from_slice(SYNC_MAGIC);

        encoded.extend_from_slice(&SYNC_PROTOCOL_VERSION.to_le_bytes());

        encoded.push(MESSAGE_TRANSACTION);

        let oversized_length = (MAX_ENCODED_TRANSACTION_SIZE as u32) + 1;

        encoded.extend_from_slice(&oversized_length.to_le_bytes());

        assert!(SyncMessage::decode(&encoded).is_err());
    }

    #[test]
    fn tcp_transaction_round_trip() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();

        let address = listener.local_addr().unwrap();

        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();

            receive_sync_message(&mut stream).unwrap()
        });

        let mut client = TcpStream::connect(address).unwrap();

        let message = SyncMessage::transaction(sample_transfer()).unwrap();

        send_sync_message(&mut client, &message).unwrap();

        let received = server.join().unwrap();

        assert_eq!(received, message);
    }

    #[test]
    fn transaction_ack_accepted_can_be_created() {
        let hash = [0x11; 32];

        let message = SyncMessage::transaction_ack(hash, true, "accepted and persisted").unwrap();

        assert_eq!(
            message,
            SyncMessage::TransactionAck {
                transaction_hash: hash,
                accepted: true,
                reason: "accepted and persisted".to_string(),
            }
        );
    }

    #[test]
    fn transaction_ack_rejected_can_be_created() {
        let hash = [0x22; 32];

        let message = SyncMessage::transaction_ack(hash, false, "invalid nonce").unwrap();

        assert_eq!(
            message,
            SyncMessage::TransactionAck {
                transaction_hash: hash,
                accepted: false,
                reason: "invalid nonce".to_string(),
            }
        );
    }

    #[test]
    fn transaction_ack_accepted_round_trip() {
        let original = SyncMessage::transaction_ack([0x33; 32], true, "accepted").unwrap();

        let encoded = original.encode().unwrap();

        let decoded = SyncMessage::decode(&encoded).unwrap();

        assert_eq!(original, decoded);
    }

    #[test]
    fn transaction_ack_rejected_round_trip() {
        let original =
            SyncMessage::transaction_ack([0x44; 32], false, "nonce already used").unwrap();

        let encoded = original.encode().unwrap();

        let decoded = SyncMessage::decode(&encoded).unwrap();

        assert_eq!(original, decoded);
    }

    #[test]
    fn transaction_ack_preserves_unicode_reason() {
        let original =
            SyncMessage::transaction_ack([0x55; 32], false, "transação rejeitada").unwrap();

        let encoded = original.encode().unwrap();

        let decoded = SyncMessage::decode(&encoded).unwrap();

        assert_eq!(original, decoded);
    }

    #[test]
    fn transaction_ack_empty_reason_is_valid() {
        let original = SyncMessage::transaction_ack([0x66; 32], true, "").unwrap();

        let encoded = original.encode().unwrap();

        let decoded = SyncMessage::decode(&encoded).unwrap();

        assert_eq!(original, decoded);
    }

    #[test]
    fn oversized_transaction_ack_reason_is_rejected() {
        let reason = "X".repeat(MAX_TRANSACTION_ACK_REASON_BYTES + 1);

        assert!(SyncMessage::transaction_ack([0x77; 32], false, reason).is_err());
    }

    #[test]
    fn malformed_transaction_ack_flag_is_rejected() {
        let mut encoded = Vec::new();

        encoded.extend_from_slice(SYNC_MAGIC);

        encoded.extend_from_slice(&SYNC_PROTOCOL_VERSION.to_le_bytes());

        encoded.push(MESSAGE_TRANSACTION_ACK);

        encoded.extend_from_slice(&[0x88; 32]);

        encoded.push(2);

        encoded.extend_from_slice(&0u16.to_le_bytes());

        assert!(SyncMessage::decode(&encoded).is_err());
    }

    #[test]
    fn truncated_transaction_ack_is_rejected() {
        let message = SyncMessage::transaction_ack([0x99; 32], false, "invalid balance").unwrap();

        let encoded = message.encode().unwrap();

        let truncated = &encoded[..encoded.len() - 1];

        assert!(SyncMessage::decode(truncated).is_err());
    }

    #[test]
    fn transaction_ack_with_trailing_bytes_is_rejected() {
        let message = SyncMessage::transaction_ack([0xAA; 32], true, "accepted").unwrap();

        let mut encoded = message.encode().unwrap();

        encoded.push(99);

        assert!(SyncMessage::decode(&encoded).is_err());
    }

    #[test]
    fn oversized_transaction_ack_reason_payload_is_rejected() {
        let mut encoded = Vec::new();

        encoded.extend_from_slice(SYNC_MAGIC);

        encoded.extend_from_slice(&SYNC_PROTOCOL_VERSION.to_le_bytes());

        encoded.push(MESSAGE_TRANSACTION_ACK);

        encoded.extend_from_slice(&[0xBB; 32]);

        encoded.push(0);

        let oversized_length = (MAX_TRANSACTION_ACK_REASON_BYTES as u16) + 1;

        encoded.extend_from_slice(&oversized_length.to_le_bytes());

        assert!(SyncMessage::decode(&encoded).is_err());
    }

    #[test]
    fn tcp_transaction_ack_round_trip() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();

        let address = listener.local_addr().unwrap();

        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();

            receive_sync_message(&mut stream).unwrap()
        });

        let mut client = TcpStream::connect(address).unwrap();

        let message =
            SyncMessage::transaction_ack([0xCC; 32], true, "persisted in mempool").unwrap();

        send_sync_message(&mut client, &message).unwrap();

        let received = server.join().unwrap();

        assert_eq!(received, message);
    }

    #[test]
    fn zero_request_limit_is_rejected() {
        assert!(SyncMessage::get_blocks(1, 0).is_err());
    }

    #[test]
    fn excessive_request_limit_is_rejected() {
        assert!(SyncMessage::get_blocks(1, MAX_BLOCKS_PER_REQUEST + 1).is_err());
    }

    #[test]
    fn invalid_magic_is_rejected() {
        let message = SyncMessage::get_blocks(1, 10).unwrap();

        let mut encoded = message.encode().unwrap();

        encoded[0] = b'X';

        assert!(SyncMessage::decode(&encoded).is_err());
    }

    #[test]
    fn truncated_message_is_rejected() {
        let message = SyncMessage::get_blocks(1, 10).unwrap();

        let encoded = message.encode().unwrap();

        let truncated = &encoded[..encoded.len() - 1];

        assert!(SyncMessage::decode(truncated).is_err());
    }

    #[test]
    fn trailing_bytes_are_rejected() {
        let message = SyncMessage::get_blocks(1, 10).unwrap();

        let mut encoded = message.encode().unwrap();

        encoded.push(99);

        assert!(SyncMessage::decode(&encoded).is_err());
    }

    #[test]
    fn tcp_sync_message_round_trip() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();

        let address = listener.local_addr().unwrap();

        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();

            receive_sync_message(&mut stream).unwrap()
        });

        let mut client = TcpStream::connect(address).unwrap();

        let message = SyncMessage::get_blocks(7, 25).unwrap();

        send_sync_message(&mut client, &message).unwrap();

        let received = server.join().unwrap();

        assert_eq!(received, message);
    }

    #[test]
    fn tcp_announce_tip_round_trip() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();

        let address = listener.local_addr().unwrap();

        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();

            receive_sync_message(&mut stream).unwrap()
        });

        let mut client = TcpStream::connect(address).unwrap();

        let message = SyncMessage::announce_tip(77, [5u8; 32]);

        send_sync_message(&mut client, &message).unwrap();

        let received = server.join().unwrap();

        assert_eq!(received, message);
    }

    #[test]
    fn block_request_can_read_genesis() {
        let blockchain = Blockchain::new();

        let blocks = blocks_for_request(&blockchain, 0, 1).unwrap();

        assert_eq!(blocks.len(), 1);

        assert_eq!(blocks[0].height, 0);
    }

    #[test]
    fn block_request_past_tip_is_empty() {
        let blockchain = Blockchain::new();

        let blocks = blocks_for_request(&blockchain, 1, 10).unwrap();

        assert!(blocks.is_empty());
    }

    #[test]
    fn invalid_batch_does_not_modify_chain() {
        let mut blockchain = Blockchain::new();

        let original_blocks = blockchain.blocks().to_vec();

        let invalid_batch = vec![create_genesis_block()];

        let result = apply_block_batch(&mut blockchain, invalid_batch);

        assert!(result.is_err());

        assert_eq!(blockchain.blocks(), original_blocks.as_slice());
    }
}

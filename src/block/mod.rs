use crate::{
    consensus::INITIAL_DIFFICULTY_BITS,
    crypto::{Hash, sha256},
    transaction::{
        MAX_TRANSACTIONS_PER_BLOCK, Transaction, decode_transactions, encode_transactions,
    },
};

pub const BLOCK_FORMAT_VERSION: u16 = 2;
pub const MAX_BLOCK_DATA_SIZE: usize = 1024 * 1024;
pub const MAX_BLOCK_SIZE: usize = MAX_BLOCK_DATA_SIZE + 80;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Block {
    pub height: u64,
    pub previous_hash: Hash,
    pub timestamp: u64,
    pub nonce: u64,
    pub difficulty_bits: u16,
    pub data: Vec<u8>,
}

impl Block {
    pub fn new(
        height: u64,
        previous_hash: Hash,
        timestamp: u64,
        nonce: u64,
        data: Vec<u8>,
    ) -> Self {
        Self {
            height,
            previous_hash,
            timestamp,
            nonce,
            difficulty_bits: INITIAL_DIFFICULTY_BITS,
            data,
        }
    }

    pub fn with_difficulty(
        height: u64,
        previous_hash: Hash,
        timestamp: u64,
        nonce: u64,
        difficulty_bits: u16,
        data: Vec<u8>,
    ) -> Result<Self, String> {
        if difficulty_bits > 256 {
            return Err("block difficulty is above the hash size".to_string());
        }

        if data.len() > MAX_BLOCK_DATA_SIZE {
            return Err("block data exceeds the network limit".to_string());
        }

        Ok(Self {
            height,
            previous_hash,
            timestamp,
            nonce,
            difficulty_bits,
            data,
        })
    }

    pub fn with_transactions(
        height: u64,
        previous_hash: Hash,
        timestamp: u64,
        nonce: u64,
        transactions: &[Transaction],
    ) -> Result<Self, String> {
        Self::with_transactions_at_difficulty(
            height,
            previous_hash,
            timestamp,
            nonce,
            INITIAL_DIFFICULTY_BITS,
            transactions,
        )
    }

    pub fn with_transactions_at_difficulty(
        height: u64,
        previous_hash: Hash,
        timestamp: u64,
        nonce: u64,
        difficulty_bits: u16,
        transactions: &[Transaction],
    ) -> Result<Self, String> {
        if transactions.len() > MAX_TRANSACTIONS_PER_BLOCK {
            return Err("block transaction count is outside the network limit".to_string());
        }

        for transaction in transactions {
            transaction.validate()?;
        }

        Self::with_difficulty(
            height,
            previous_hash,
            timestamp,
            nonce,
            difficulty_bits,
            encode_transactions(transactions),
        )
    }

    pub fn transactions(&self) -> Result<Vec<Transaction>, String> {
        decode_transactions(&self.data)
    }

    pub fn hash(&self) -> Hash {
        sha256(&self.encode())
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::new();

        bytes.extend_from_slice(&self.height.to_le_bytes());

        bytes.extend_from_slice(&self.previous_hash);

        bytes.extend_from_slice(&self.timestamp.to_le_bytes());

        bytes.extend_from_slice(&self.nonce.to_le_bytes());

        bytes.extend_from_slice(&BLOCK_FORMAT_VERSION.to_le_bytes());

        bytes.extend_from_slice(&self.difficulty_bits.to_le_bytes());

        bytes.extend_from_slice(&(self.data.len() as u64).to_le_bytes());

        bytes.extend_from_slice(&self.data);

        bytes
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_BLOCK_SIZE {
            return Err("encoded block exceeds the network limit".to_string());
        }

        let mut cursor = 0usize;

        let height = read_u64(bytes, &mut cursor)?;

        let previous_hash_bytes = read_bytes(bytes, &mut cursor, 32)?;

        let mut previous_hash = [0u8; 32];

        previous_hash.copy_from_slice(previous_hash_bytes);

        let timestamp = read_u64(bytes, &mut cursor)?;

        let nonce = read_u64(bytes, &mut cursor)?;

        let format_version = read_u16(bytes, &mut cursor)?;

        if format_version != BLOCK_FORMAT_VERSION {
            return Err(format!(
                "unsupported block format version: {}",
                format_version
            ));
        }

        let difficulty_bits = read_u16(bytes, &mut cursor)?;

        if difficulty_bits > 256 {
            return Err("block difficulty is above the hash size".to_string());
        }

        let data_length = usize::try_from(read_u64(bytes, &mut cursor)?)
            .map_err(|_| "block data length is too large".to_string())?;

        if data_length > MAX_BLOCK_DATA_SIZE {
            return Err("block data exceeds the network limit".to_string());
        }

        let data = read_bytes(bytes, &mut cursor, data_length)?.to_vec();

        if cursor != bytes.len() {
            return Err("block contains trailing bytes".to_string());
        }

        Ok(Self {
            height,
            previous_hash,
            timestamp,
            nonce,
            difficulty_bits,
            data,
        })
    }
}

fn read_u16(bytes: &[u8], cursor: &mut usize) -> Result<u16, String> {
    let raw = read_bytes(bytes, cursor, 2)?;
    let mut array = [0u8; 2];
    array.copy_from_slice(raw);
    Ok(u16::from_le_bytes(array))
}

fn read_u64(bytes: &[u8], cursor: &mut usize) -> Result<u64, String> {
    let raw = read_bytes(bytes, cursor, 8)?;

    let mut array = [0u8; 8];

    array.copy_from_slice(raw);

    Ok(u64::from_le_bytes(array))
}

fn read_bytes<'a>(bytes: &'a [u8], cursor: &mut usize, length: usize) -> Result<&'a [u8], String> {
    let end = cursor
        .checked_add(length)
        .ok_or_else(|| "block decode offset overflow".to_string())?;

    if end > bytes.len() {
        return Err("unexpected end of block data".to_string());
    }

    let result = &bytes[*cursor..end];

    *cursor = end;

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::KeyPair;

    fn alice_key() -> KeyPair {
        KeyPair::from_secret_bytes([7u8; 32])
    }

    fn bob_key() -> KeyPair {
        KeyPair::from_secret_bytes([9u8; 32])
    }

    #[test]
    fn same_block_produces_same_hash() {
        let block_a = Block::new(1, [0u8; 32], 1_700_000_000, 0, b"Topazio Network".to_vec());

        let block_b = block_a.clone();

        assert_eq!(block_a.hash(), block_b.hash());
    }

    #[test]
    fn changing_nonce_changes_block_hash() {
        let block_a = Block::new(1, [0u8; 32], 1_700_000_000, 1, b"Topazio".to_vec());

        let mut block_b = block_a.clone();

        block_b.nonce = 2;

        assert_ne!(block_a.hash(), block_b.hash());
    }

    #[test]
    fn changing_data_changes_block_hash() {
        let block_a = Block::new(1, [0u8; 32], 1_700_000_000, 0, b"TPZ".to_vec());

        let block_b = Block::new(1, [0u8; 32], 1_700_000_000, 0, b"TOPAZIO".to_vec());

        assert_ne!(block_a.hash(), block_b.hash());
    }

    #[test]
    fn block_transactions_round_trip() {
        let alice = alice_key();

        let bob = bob_key();

        let transaction = Transaction::signed(&alice, bob.address(), 25, 0, 0).unwrap();

        let block =
            Block::with_transactions(1, [0u8; 32], 1_700_000_000, 0, &[transaction.clone()])
                .unwrap();

        assert_eq!(block.transactions().unwrap(), vec![transaction]);
    }

    #[test]
    fn changing_transaction_changes_block_hash() {
        let alice = alice_key();

        let bob = bob_key();

        let transaction_a = Transaction::signed(&alice, bob.address(), 25, 0, 0).unwrap();

        let transaction_b = Transaction::signed(&alice, bob.address(), 26, 0, 0).unwrap();

        let block_a =
            Block::with_transactions(1, [0u8; 32], 1_700_000_000, 0, &[transaction_a]).unwrap();

        let block_b =
            Block::with_transactions(1, [0u8; 32], 1_700_000_000, 0, &[transaction_b]).unwrap();

        assert_ne!(block_a.hash(), block_b.hash());
    }

    #[test]
    fn block_encode_decode_round_trip() {
        let original = Block::new(
            7,
            [3u8; 32],
            1_788_825_607,
            12_345,
            b"Topazio persistence".to_vec(),
        );

        let encoded = original.encode();

        let decoded = Block::decode(&encoded).unwrap();

        assert_eq!(original, decoded);

        assert_eq!(original.hash(), decoded.hash());
    }

    #[test]
    fn truncated_block_is_rejected() {
        let block = Block::new(1, [0u8; 32], 1_788_825_601, 0, b"TPZ".to_vec());

        let mut encoded = block.encode();

        encoded.pop();

        assert!(Block::decode(&encoded).is_err());
    }
}

use crate::{
    consensus::AtomicUnits,
    crypto::{Hash, sha256},
    identity::{KeyPair, address_from_public_key, validate_address, verify_signature},
};

// Version 2 adds the explicit fee field. Version 1 transactions are not
// silently migrated: their encoded form is rejected by decode().
pub const TRANSACTION_VERSION: u16 = 2;

/*
 * Limites defensivos para dados vindos
 * de arquivo, rede ou qualquer origem externa.
 *
 * Endereços atuais da Topazio Lab são muito
 * menores que 256 bytes, então este limite
 * ainda deixa bastante margem.
 */
pub const MAX_ADDRESS_BYTES: usize = 256;

/*
 * Uma transação Topazio Lab atual ocupa
 * apenas algumas centenas de bytes.
 *
 * 16 KiB é uma margem grande para o laboratório,
 * mas impede peers de anunciarem estruturas
 * absurdamente grandes.
 */
pub const MAX_TRANSACTION_SIZE: usize = 16 * 1024;

const SIGNING_DOMAIN: &[u8] = b"TOPAZIO_NETWORK_TX_TESTNET_V2";

pub const MAX_TRANSACTIONS_PER_BLOCK: usize = 512;

const TYPE_TRANSFER: u8 = 0;
const TYPE_COINBASE: u8 = 1;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransactionKind {
    Transfer,
    Coinbase,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Transaction {
    pub kind: TransactionKind,
    pub from: String,
    pub to: String,
    pub amount: AtomicUnits,
    pub fee: AtomicUnits,
    pub nonce: u64,
    pub sender_public_key: [u8; 32],
    pub signature: [u8; 64],
}

impl Transaction {
    pub fn signed(
        key_pair: &KeyPair,
        to: impl Into<String>,
        amount: AtomicUnits,
        fee: AtomicUnits,
        nonce: u64,
    ) -> Result<Self, String> {
        let from = key_pair.address();

        let to = to.into();

        validate_address_length("sender", &from)?;

        validate_address_length("recipient", &to)?;

        let sender_public_key = key_pair.public_key_bytes();

        let signing_payload = Self::build_signing_payload(
            TransactionKind::Transfer,
            &from,
            &to,
            amount,
            fee,
            nonce,
            &sender_public_key,
        );

        let signature = key_pair.sign(&signing_payload);

        let transaction = Self {
            kind: TransactionKind::Transfer,
            from,
            to,
            amount,
            fee,
            nonce,
            sender_public_key,
            signature,
        };

        transaction.validate()?;

        validate_encoded_size(&transaction)?;

        Ok(transaction)
    }

    pub fn coinbase(
        to: impl Into<String>,
        amount: AtomicUnits,
        block_height: u64,
    ) -> Result<Self, String> {
        let to = to.into();

        validate_address_length("recipient", &to)?;

        let transaction = Self {
            kind: TransactionKind::Coinbase,
            from: String::new(),
            to,
            amount,
            fee: 0,
            nonce: block_height,
            sender_public_key: [0u8; 32],
            signature: [0u8; 64],
        };

        transaction.validate()?;

        validate_encoded_size(&transaction)?;

        Ok(transaction)
    }

    pub fn is_coinbase(&self) -> bool {
        self.kind == TransactionKind::Coinbase
    }

    pub fn total_debit(&self) -> Result<AtomicUnits, String> {
        self.amount
            .checked_add(self.fee)
            .ok_or_else(|| "transaction amount plus fee overflows".to_string())
    }

    pub fn validate(&self) -> Result<(), String> {
        match self.kind {
            TransactionKind::Transfer => self.validate_transfer(),

            TransactionKind::Coinbase => self.validate_coinbase(),
        }
    }

    fn validate_transfer(&self) -> Result<(), String> {
        if self.from.trim().is_empty() {
            return Err("sender cannot be empty".to_string());
        }

        if self.to.trim().is_empty() {
            return Err("recipient cannot be empty".to_string());
        }

        validate_address_length("sender", &self.from)?;

        validate_address_length("recipient", &self.to)?;

        validate_address(&self.from)?;
        validate_address(&self.to)?;

        if self.from == self.to {
            return Err("sender and recipient cannot be the same".to_string());
        }

        if self.amount == 0 {
            return Err("transaction amount must be greater than zero".to_string());
        }

        self.total_debit()?;

        let expected_address = address_from_public_key(&self.sender_public_key);

        if self.from != expected_address {
            return Err("sender address does not match public key".to_string());
        }

        let signing_payload = self.signing_payload();

        if !verify_signature(&self.sender_public_key, &signing_payload, &self.signature) {
            return Err("invalid transaction signature".to_string());
        }

        validate_encoded_size(self)?;

        Ok(())
    }

    fn validate_coinbase(&self) -> Result<(), String> {
        if !self.from.is_empty() {
            return Err("coinbase sender must be empty".to_string());
        }

        if self.to.trim().is_empty() {
            return Err("coinbase recipient cannot be empty".to_string());
        }

        validate_address_length("recipient", &self.to)?;

        validate_address(&self.to)?;

        if self.fee != 0 {
            return Err("coinbase fee must be zero".to_string());
        }

        if self.sender_public_key != [0u8; 32] {
            return Err("coinbase cannot contain sender public key".to_string());
        }

        if self.signature != [0u8; 64] {
            return Err("coinbase cannot contain signature".to_string());
        }

        validate_encoded_size(self)?;

        Ok(())
    }

    pub fn hash(&self) -> Hash {
        sha256(&self.encode())
    }

    pub fn signing_payload(&self) -> Vec<u8> {
        Self::build_signing_payload(
            self.kind,
            &self.from,
            &self.to,
            self.amount,
            self.fee,
            self.nonce,
            &self.sender_public_key,
        )
    }

    fn build_signing_payload(
        kind: TransactionKind,
        from: &str,
        to: &str,
        amount: u64,
        fee: u64,
        nonce: u64,
        sender_public_key: &[u8; 32],
    ) -> Vec<u8> {
        let mut bytes = Vec::new();

        bytes.extend_from_slice(SIGNING_DOMAIN);

        bytes.extend_from_slice(&TRANSACTION_VERSION.to_le_bytes());

        bytes.push(kind_to_byte(kind));

        let from_bytes = from.as_bytes();

        let to_bytes = to.as_bytes();

        bytes.extend_from_slice(&(from_bytes.len() as u64).to_le_bytes());

        bytes.extend_from_slice(from_bytes);

        bytes.extend_from_slice(&(to_bytes.len() as u64).to_le_bytes());

        bytes.extend_from_slice(to_bytes);

        bytes.extend_from_slice(&amount.to_le_bytes());

        bytes.extend_from_slice(&fee.to_le_bytes());

        bytes.extend_from_slice(&nonce.to_le_bytes());

        bytes.extend_from_slice(sender_public_key);

        bytes
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut bytes = Vec::new();

        bytes.extend_from_slice(&TRANSACTION_VERSION.to_le_bytes());

        bytes.push(kind_to_byte(self.kind));

        let from_bytes = self.from.as_bytes();

        let to_bytes = self.to.as_bytes();

        bytes.extend_from_slice(&(from_bytes.len() as u64).to_le_bytes());

        bytes.extend_from_slice(from_bytes);

        bytes.extend_from_slice(&(to_bytes.len() as u64).to_le_bytes());

        bytes.extend_from_slice(to_bytes);

        bytes.extend_from_slice(&self.amount.to_le_bytes());

        bytes.extend_from_slice(&self.fee.to_le_bytes());

        bytes.extend_from_slice(&self.nonce.to_le_bytes());

        bytes.extend_from_slice(&self.sender_public_key);

        bytes.extend_from_slice(&self.signature);

        bytes
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, String> {
        if bytes.len() > MAX_TRANSACTION_SIZE {
            return Err(format!(
                "transaction exceeds maximum size of {} bytes",
                MAX_TRANSACTION_SIZE
            ));
        }

        let mut cursor = 0usize;

        let version = read_u16(bytes, &mut cursor)?;

        if version != TRANSACTION_VERSION {
            return Err(format!("unsupported transaction version: {}", version));
        }

        let kind_byte = read_u8(bytes, &mut cursor)?;

        let kind = byte_to_kind(kind_byte)?;

        let from_len = read_length(bytes, &mut cursor, "sender", MAX_ADDRESS_BYTES)?;

        let from = String::from_utf8(read_bytes(bytes, &mut cursor, from_len)?.to_vec())
            .map_err(|_| "sender is not valid UTF-8".to_string())?;

        let to_len = read_length(bytes, &mut cursor, "recipient", MAX_ADDRESS_BYTES)?;

        let to = String::from_utf8(read_bytes(bytes, &mut cursor, to_len)?.to_vec())
            .map_err(|_| "recipient is not valid UTF-8".to_string())?;

        let amount = read_u64(bytes, &mut cursor)?;

        let fee = read_u64(bytes, &mut cursor)?;

        let nonce = read_u64(bytes, &mut cursor)?;

        let public_key_bytes = read_bytes(bytes, &mut cursor, 32)?;

        let mut sender_public_key = [0u8; 32];

        sender_public_key.copy_from_slice(public_key_bytes);

        let signature_bytes = read_bytes(bytes, &mut cursor, 64)?;

        let mut signature = [0u8; 64];

        signature.copy_from_slice(signature_bytes);

        if cursor != bytes.len() {
            return Err("transaction contains trailing bytes".to_string());
        }

        let transaction = Self {
            kind,
            from,
            to,
            amount,
            fee,
            nonce,
            sender_public_key,
            signature,
        };

        transaction.validate()?;

        Ok(transaction)
    }
}

pub fn encode_transactions(transactions: &[Transaction]) -> Vec<u8> {
    let mut bytes = Vec::new();

    bytes.extend_from_slice(&(transactions.len() as u64).to_le_bytes());

    for transaction in transactions {
        let encoded = transaction.encode();

        bytes.extend_from_slice(&(encoded.len() as u64).to_le_bytes());

        bytes.extend_from_slice(&encoded);
    }

    bytes
}

pub fn sum_fees(transactions: &[Transaction]) -> Result<AtomicUnits, String> {
    let mut total = 0u64;

    for transaction in transactions {
        if transaction.is_coinbase() {
            continue;
        }

        transaction.validate()?;
        total = total
            .checked_add(transaction.fee)
            .ok_or_else(|| "transaction fees overflow".to_string())?;
    }

    Ok(total)
}

pub fn decode_transactions(bytes: &[u8]) -> Result<Vec<Transaction>, String> {
    let mut cursor = 0usize;

    let count = usize::try_from(read_u64(bytes, &mut cursor)?)
        .map_err(|_| "transaction count is too large".to_string())?;

    /*
     * Cada transação precisa ocupar ao menos
     * alguns bytes. Esse limite simples impede
     * reservar memória absurda com Vec::with_capacity()
     * a partir de um contador malicioso.
     */
    let maximum_possible_count = bytes.len().saturating_div(8).saturating_add(1);

    if count > maximum_possible_count {
        return Err("transaction count is inconsistent with encoded data".to_string());
    }

    if count > 10_000 {
        return Err("transaction batch exceeds the network limit".to_string());
    }

    let mut transactions = Vec::with_capacity(count);

    for _ in 0..count {
        let transaction_len = usize::try_from(read_u64(bytes, &mut cursor)?)
            .map_err(|_| "transaction length is too large".to_string())?;

        if transaction_len == 0 {
            return Err("encoded transaction cannot be empty".to_string());
        }

        if transaction_len > MAX_TRANSACTION_SIZE {
            return Err(format!(
                "encoded transaction exceeds maximum size of {} bytes",
                MAX_TRANSACTION_SIZE
            ));
        }

        let transaction_bytes = read_bytes(bytes, &mut cursor, transaction_len)?;

        transactions.push(Transaction::decode(transaction_bytes)?);
    }

    if cursor != bytes.len() {
        return Err("transaction batch contains trailing bytes".to_string());
    }

    Ok(transactions)
}

fn validate_address_length(field_name: &str, value: &str) -> Result<(), String> {
    if value.as_bytes().len() > MAX_ADDRESS_BYTES {
        return Err(format!(
            "{} exceeds maximum size of {} bytes",
            field_name, MAX_ADDRESS_BYTES
        ));
    }

    Ok(())
}

fn validate_encoded_size(transaction: &Transaction) -> Result<(), String> {
    let encoded_size = transaction.encode().len();

    if encoded_size > MAX_TRANSACTION_SIZE {
        return Err(format!(
            "transaction exceeds maximum size of {} bytes",
            MAX_TRANSACTION_SIZE
        ));
    }

    Ok(())
}

fn read_length(
    bytes: &[u8],
    cursor: &mut usize,
    field_name: &str,
    maximum: usize,
) -> Result<usize, String> {
    let length = usize::try_from(read_u64(bytes, cursor)?)
        .map_err(|_| format!("{} length is too large", field_name))?;

    if length > maximum {
        return Err(format!(
            "{} exceeds maximum size of {} bytes",
            field_name, maximum
        ));
    }

    Ok(length)
}

fn kind_to_byte(kind: TransactionKind) -> u8 {
    match kind {
        TransactionKind::Transfer => TYPE_TRANSFER,

        TransactionKind::Coinbase => TYPE_COINBASE,
    }
}

fn byte_to_kind(value: u8) -> Result<TransactionKind, String> {
    match value {
        TYPE_TRANSFER => Ok(TransactionKind::Transfer),

        TYPE_COINBASE => Ok(TransactionKind::Coinbase),

        _ => Err(format!("unknown transaction type: {}", value)),
    }
}

fn read_u8(bytes: &[u8], cursor: &mut usize) -> Result<u8, String> {
    Ok(read_bytes(bytes, cursor, 1)?[0])
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
        .ok_or_else(|| "decode offset overflow".to_string())?;

    if end > bytes.len() {
        return Err("unexpected end of encoded data".to_string());
    }

    let result = &bytes[*cursor..end];

    *cursor = end;

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn alice_key() -> KeyPair {
        KeyPair::from_secret_bytes([7u8; 32])
    }

    fn bob_key() -> KeyPair {
        KeyPair::from_secret_bytes([9u8; 32])
    }

    #[test]
    fn valid_signed_transaction_is_accepted() {
        let alice = alice_key();

        let bob = bob_key();

        let transaction = Transaction::signed(&alice, bob.address(), 25, 0, 0).unwrap();

        assert!(transaction.validate().is_ok());
    }

    #[test]
    fn positive_fee_is_accepted_and_is_part_of_transaction() {
        let alice = alice_key();
        let bob = bob_key();

        let transaction = Transaction::signed(&alice, bob.address(), 25, 7, 0).unwrap();

        assert_eq!(transaction.amount, 25);
        assert_eq!(transaction.fee, 7);
        assert_eq!(transaction.total_debit().unwrap(), 32);
        assert!(transaction.validate().is_ok());
    }

    #[test]
    fn signed_transaction_uses_sender_address() {
        let alice = alice_key();

        let bob = bob_key();

        let transaction = Transaction::signed(&alice, bob.address(), 25, 0, 0).unwrap();

        assert_eq!(transaction.from, alice.address());
    }

    #[test]
    fn valid_coinbase_is_accepted() {
        let alice = alice_key();

        let coinbase = Transaction::coinbase(alice.address(), 50, 1).unwrap();

        assert!(coinbase.validate().is_ok());

        assert!(coinbase.is_coinbase());
    }

    #[test]
    fn zero_subsidy_coinbase_is_accepted() {
        let alice = alice_key();

        let coinbase = Transaction::coinbase(alice.address(), 0, 117_600_001).unwrap();

        assert!(coinbase.validate().is_ok());
    }

    #[test]
    fn coinbase_has_no_sender_signature() {
        let alice = alice_key();

        let coinbase = Transaction::coinbase(alice.address(), 50, 1).unwrap();

        assert_eq!(coinbase.from, "");

        assert_eq!(coinbase.sender_public_key, [0u8; 32]);

        assert_eq!(coinbase.signature, [0u8; 64]);
    }

    #[test]
    fn same_transaction_produces_same_hash() {
        let alice = alice_key();

        let bob = bob_key();

        let a = Transaction::signed(&alice, bob.address(), 25, 0, 7).unwrap();

        let b = Transaction::signed(&alice, bob.address(), 25, 0, 7).unwrap();

        assert_eq!(a.hash(), b.hash());
    }

    #[test]
    fn changing_amount_invalidates_signature() {
        let alice = alice_key();

        let bob = bob_key();

        let mut transaction = Transaction::signed(&alice, bob.address(), 25, 0, 0).unwrap();

        transaction.amount = 250;

        assert!(transaction.validate().is_err());
    }

    #[test]
    fn changing_fee_invalidates_signature() {
        let alice = alice_key();
        let bob = bob_key();

        let mut transaction = Transaction::signed(&alice, bob.address(), 25, 7, 0).unwrap();
        transaction.fee = 8;

        assert!(transaction.validate().is_err());
    }

    #[test]
    fn changing_recipient_invalidates_signature() {
        let alice = alice_key();

        let bob = bob_key();

        let carol = KeyPair::from_secret_bytes([11u8; 32]);

        let mut transaction = Transaction::signed(&alice, bob.address(), 25, 0, 0).unwrap();

        transaction.to = carol.address();

        assert!(transaction.validate().is_err());
    }

    #[test]
    fn changing_nonce_invalidates_signature() {
        let alice = alice_key();

        let bob = bob_key();

        let mut transaction = Transaction::signed(&alice, bob.address(), 25, 0, 0).unwrap();

        transaction.nonce = 1;

        assert!(transaction.validate().is_err());
    }

    #[test]
    fn wrong_sender_address_is_rejected() {
        let alice = alice_key();

        let bob = bob_key();

        let mut transaction = Transaction::signed(&alice, bob.address(), 25, 0, 0).unwrap();

        transaction.from = bob.address();

        assert!(transaction.validate().is_err());
    }

    #[test]
    fn zero_amount_is_rejected() {
        let alice = alice_key();

        let bob = bob_key();

        assert!(Transaction::signed(&alice, bob.address(), 0, 0, 0,).is_err());
    }

    #[test]
    fn amount_plus_fee_overflow_is_rejected() {
        let alice = alice_key();
        let bob = bob_key();

        assert!(Transaction::signed(&alice, bob.address(), u64::MAX, 1, 0).is_err());
    }

    #[test]
    fn sender_cannot_send_to_itself() {
        let alice = alice_key();

        assert!(Transaction::signed(&alice, alice.address(), 25, 0, 0,).is_err());
    }

    #[test]
    fn transaction_encode_decode_round_trip() {
        let alice = alice_key();

        let bob = bob_key();

        let original = Transaction::signed(&alice, bob.address(), 123, 0, 9).unwrap();

        let decoded = Transaction::decode(&original.encode()).unwrap();

        assert_eq!(original, decoded);
    }

    #[test]
    fn coinbase_encode_decode_round_trip() {
        let alice = alice_key();

        let original = Transaction::coinbase(alice.address(), 50, 1).unwrap();

        let decoded = Transaction::decode(&original.encode()).unwrap();

        assert_eq!(original, decoded);
    }

    #[test]
    fn coinbase_fee_must_be_zero() {
        let alice = alice_key();
        let mut coinbase = Transaction::coinbase(alice.address(), 50, 1).unwrap();
        coinbase.fee = 1;

        assert!(coinbase.validate().is_err());
    }

    #[test]
    fn transaction_batch_round_trip() {
        let alice = alice_key();

        let bob = bob_key();

        let transactions = vec![
            Transaction::coinbase(alice.address(), 50, 1).unwrap(),
            Transaction::signed(&alice, bob.address(), 25, 0, 0).unwrap(),
        ];

        let encoded = encode_transactions(&transactions);

        let decoded = decode_transactions(&encoded).unwrap();

        assert_eq!(transactions, decoded);
    }

    #[test]
    fn malformed_transaction_batch_is_rejected() {
        assert!(decode_transactions(&[2u8, 0, 0],).is_err());
    }

    #[test]
    fn legacy_transaction_version_is_rejected() {
        assert!(Transaction::decode(&1u16.to_le_bytes()).is_err());
    }

    #[test]
    fn modified_signature_is_rejected() {
        let alice = alice_key();

        let bob = bob_key();

        let mut transaction = Transaction::signed(&alice, bob.address(), 25, 0, 0).unwrap();

        transaction.signature[0] ^= 0x01;

        assert!(transaction.validate().is_err());
    }

    #[test]
    fn changing_amount_changes_transaction_hash() {
        let alice = alice_key();

        let bob = bob_key();

        let a = Transaction::signed(&alice, bob.address(), 25, 0, 0).unwrap();

        let b = Transaction::signed(&alice, bob.address(), 26, 0, 0).unwrap();

        assert_ne!(a.hash(), b.hash());
    }

    #[test]
    fn oversized_transaction_is_rejected() {
        let bytes = vec![0u8; MAX_TRANSACTION_SIZE + 1];

        assert!(Transaction::decode(&bytes,).is_err());
    }

    #[test]
    fn oversized_sender_is_rejected_before_allocation() {
        let mut bytes = Vec::new();

        bytes.extend_from_slice(&TRANSACTION_VERSION.to_le_bytes());

        bytes.push(TYPE_TRANSFER);

        bytes.extend_from_slice(&((MAX_ADDRESS_BYTES as u64) + 1).to_le_bytes());

        assert!(Transaction::decode(&bytes,).is_err());
    }

    #[test]
    fn oversized_recipient_is_rejected_before_allocation() {
        let mut bytes = Vec::new();

        bytes.extend_from_slice(&TRANSACTION_VERSION.to_le_bytes());

        bytes.push(TYPE_COINBASE);

        bytes.extend_from_slice(&0u64.to_le_bytes());

        bytes.extend_from_slice(&((MAX_ADDRESS_BYTES as u64) + 1).to_le_bytes());

        assert!(Transaction::decode(&bytes,).is_err());
    }
}

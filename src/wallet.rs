use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::{Path, PathBuf},
};

use argon2::{Algorithm, Argon2, Params, Version};
use chacha20poly1305::{ChaCha20Poly1305, Key, KeyInit, Nonce, aead::Aead};
use rand::Rng;
use zeroize::Zeroize;

use crate::{identity::KeyPair, transaction::Transaction};

const WALLET_MAGIC: &[u8; 8] = b"TPZWLT01";
const WALLET_VERSION: u16 = 1;
const SALT_SIZE: usize = 16;
const NONCE_SIZE: usize = 12;
const SECRET_SIZE: usize = 32;
const MAX_WALLET_FILE_SIZE: usize = 16 * 1024;

#[derive(Clone)]
pub struct Wallet {
    keypair: KeyPair,
}

impl Wallet {
    pub fn create(path: impl AsRef<Path>, password: &str) -> Result<Self, String> {
        if password.is_empty() {
            return Err("wallet password cannot be empty".to_string());
        }

        let path = path.as_ref();
        if path.exists() {
            return Err("wallet file already exists".to_string());
        }

        let mut secret = [0u8; SECRET_SIZE];
        rand::rng().fill(&mut secret);
        let keypair = KeyPair::from_secret_bytes(secret);

        let bytes = encrypt_secret(&secret, password)?;
        write_atomic(path, &bytes)?;

        Ok(Self { keypair })
    }

    pub fn open(path: impl AsRef<Path>, password: &str) -> Result<Self, String> {
        if password.is_empty() {
            return Err("wallet password cannot be empty".to_string());
        }

        let bytes =
            fs::read(path.as_ref()).map_err(|error| format!("failed to read wallet: {}", error))?;

        if bytes.len() > MAX_WALLET_FILE_SIZE {
            return Err("wallet file exceeds the network limit".to_string());
        }

        let secret = decrypt_secret(&bytes, password)?;
        Ok(Self {
            keypair: KeyPair::from_secret_bytes(secret),
        })
    }

    pub fn address(&self) -> String {
        self.keypair.address()
    }

    pub fn public_key_bytes(&self) -> [u8; 32] {
        self.keypair.public_key_bytes()
    }

    pub fn sign_transfer(
        &self,
        to: impl Into<String>,
        amount: u64,
        fee: u64,
        nonce: u64,
    ) -> Result<Transaction, String> {
        Transaction::signed(&self.keypair, to, amount, fee, nonce)
    }
}

fn derive_key(password: &str, salt: &[u8; SALT_SIZE]) -> Result<[u8; 32], String> {
    let params = Params::new(19 * 1024, 2, 1, Some(32))
        .map_err(|error| format!("invalid wallet KDF parameters: {}", error))?;
    let argon2 = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut key = [0u8; 32];
    argon2
        .hash_password_into(password.as_bytes(), salt, &mut key)
        .map_err(|error| format!("wallet key derivation failed: {}", error))?;
    Ok(key)
}

fn encrypt_secret(secret: &[u8; SECRET_SIZE], password: &str) -> Result<Vec<u8>, String> {
    let mut salt = [0u8; SALT_SIZE];
    let mut nonce = [0u8; NONCE_SIZE];
    rand::rng().fill(&mut salt);
    rand::rng().fill(&mut nonce);

    let mut key_bytes = derive_key(password, &salt)?;
    let cipher = ChaCha20Poly1305::new(Key::from_slice(&key_bytes));
    let ciphertext = match cipher.encrypt(Nonce::from_slice(&nonce), secret.as_ref()) {
        Ok(ciphertext) => ciphertext,
        Err(_) => {
            key_bytes.zeroize();
            return Err("wallet encryption failed".to_string());
        }
    };
    key_bytes.zeroize();

    let mut bytes =
        Vec::with_capacity(WALLET_MAGIC.len() + 2 + SALT_SIZE + NONCE_SIZE + 4 + ciphertext.len());
    bytes.extend_from_slice(WALLET_MAGIC);
    bytes.extend_from_slice(&WALLET_VERSION.to_le_bytes());
    bytes.extend_from_slice(&salt);
    bytes.extend_from_slice(&nonce);
    bytes.extend_from_slice(&(ciphertext.len() as u32).to_le_bytes());
    bytes.extend_from_slice(&ciphertext);
    Ok(bytes)
}

fn decrypt_secret(bytes: &[u8], password: &str) -> Result<[u8; SECRET_SIZE], String> {
    let minimum = WALLET_MAGIC.len() + 2 + SALT_SIZE + NONCE_SIZE + 4;
    if bytes.len() < minimum {
        return Err("wallet file is truncated".to_string());
    }

    if &bytes[..WALLET_MAGIC.len()] != WALLET_MAGIC {
        return Err("invalid Topazio wallet magic".to_string());
    }

    let mut cursor = WALLET_MAGIC.len();
    let version = u16::from_le_bytes([bytes[cursor], bytes[cursor + 1]]);
    cursor += 2;
    if version != WALLET_VERSION {
        return Err(format!("unsupported wallet version: {}", version));
    }

    let mut salt = [0u8; SALT_SIZE];
    salt.copy_from_slice(&bytes[cursor..cursor + SALT_SIZE]);
    cursor += SALT_SIZE;

    let mut nonce = [0u8; NONCE_SIZE];
    nonce.copy_from_slice(&bytes[cursor..cursor + NONCE_SIZE]);
    cursor += NONCE_SIZE;

    let ciphertext_len = u32::from_le_bytes([
        bytes[cursor],
        bytes[cursor + 1],
        bytes[cursor + 2],
        bytes[cursor + 3],
    ]) as usize;
    cursor += 4;

    if ciphertext_len != bytes.len().saturating_sub(cursor) || ciphertext_len < SECRET_SIZE + 16 {
        return Err("invalid wallet ciphertext length".to_string());
    }

    let mut key_bytes = derive_key(password, &salt)?;
    let cipher = ChaCha20Poly1305::new(Key::from_slice(&key_bytes));
    let plaintext = match cipher.decrypt(Nonce::from_slice(&nonce), &bytes[cursor..]) {
        Ok(plaintext) => plaintext,
        Err(_) => {
            key_bytes.zeroize();
            return Err("invalid wallet password or corrupted wallet".to_string());
        }
    };
    key_bytes.zeroize();

    if plaintext.len() != SECRET_SIZE {
        return Err("invalid wallet secret length".to_string());
    }

    let mut secret = [0u8; SECRET_SIZE];
    secret.copy_from_slice(&plaintext);
    Ok(secret)
}

fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        if !parent.as_os_str().is_empty() {
            fs::create_dir_all(parent)
                .map_err(|error| format!("failed to create wallet directory: {}", error))?;
        }
    }

    let temporary = temporary_path(path);
    let result = (|| {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|error| format!("failed to create wallet temporary file: {}", error))?;
        file.write_all(bytes)
            .map_err(|error| format!("failed to write wallet: {}", error))?;
        file.sync_all()
            .map_err(|error| format!("failed to synchronize wallet: {}", error))?;
        drop(file);
        fs::rename(&temporary, path)
            .map_err(|error| format!("failed to promote wallet file: {}", error))
    })();

    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }

    result
}

fn temporary_path(path: &Path) -> PathBuf {
    path.with_extension(format!("tmp-{}", std::process::id()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn temporary_path() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "topazio-wallet-{}-{}.dat",
            std::process::id(),
            unique
        ))
    }

    #[test]
    fn wallet_round_trip_keeps_address_and_signs() {
        let path = temporary_path();
        let wallet = Wallet::create(&path, "correct horse battery staple").unwrap();
        let address = wallet.address();
        let reopened = Wallet::open(&path, "correct horse battery staple").unwrap();
        assert_eq!(reopened.address(), address);
        let recipient = crate::identity::KeyPair::from_secret_bytes([9u8; 32]).address();
        assert!(reopened.sign_transfer(recipient, 1, 0, 0).is_ok());
        let _ = fs::remove_file(path);
    }

    #[test]
    fn wrong_password_is_rejected() {
        let path = temporary_path();
        Wallet::create(&path, "one password").unwrap();
        assert!(Wallet::open(&path, "another password").is_err());
        let _ = fs::remove_file(path);
    }
}

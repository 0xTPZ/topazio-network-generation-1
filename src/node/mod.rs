use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
};

use crate::identity::KeyPair;

const IDENTITY_MAGIC: &[u8; 8] = b"TPZNODE1";

const SECRET_LENGTH: usize = 32;

#[derive(Debug, Clone)]
pub struct NodeIdentity {
    secret: [u8; SECRET_LENGTH],
}

impl NodeIdentity {
    pub fn from_secret(secret: [u8; SECRET_LENGTH]) -> Self {
        Self { secret }
    }

    pub fn generate() -> Self {
        Self {
            secret: rand::random(),
        }
    }

    pub fn key_pair(&self) -> KeyPair {
        KeyPair::from_secret_bytes(self.secret)
    }

    pub fn address(&self) -> String {
        self.key_pair().address()
    }

    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), String> {
        let path = path.as_ref();

        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                fs::create_dir_all(parent).map_err(|error| {
                    format!("failed to create node identity directory: {}", error)
                })?;
            }
        }

        let mut bytes = Vec::with_capacity(IDENTITY_MAGIC.len() + SECRET_LENGTH);

        bytes.extend_from_slice(IDENTITY_MAGIC);

        bytes.extend_from_slice(&self.secret);

        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(path)
            .map_err(|error| format!("failed to create node identity file: {}", error))?;

        file.write_all(&bytes)
            .map_err(|error| format!("failed to write node identity: {}", error))?;

        file.flush()
            .map_err(|error| format!("failed to flush node identity: {}", error))?;

        file.sync_all()
            .map_err(|error| format!("failed to synchronize node identity: {}", error))?;

        Ok(())
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self, String> {
        let bytes =
            fs::read(path).map_err(|error| format!("failed to read node identity: {}", error))?;

        let expected_length = IDENTITY_MAGIC.len() + SECRET_LENGTH;

        if bytes.len() != expected_length {
            return Err(format!(
                "invalid node identity length: expected {}, received {}",
                expected_length,
                bytes.len()
            ));
        }

        if &bytes[..IDENTITY_MAGIC.len()] != IDENTITY_MAGIC {
            return Err("invalid Topazio node identity magic".to_string());
        }

        let mut secret = [0u8; SECRET_LENGTH];

        secret.copy_from_slice(&bytes[IDENTITY_MAGIC.len()..]);

        Ok(Self { secret })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::time::{SystemTime, UNIX_EPOCH};

    fn temporary_path(name: &str) -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();

        std::env::temp_dir().join(format!(
            "topazio-node-{}-{}-{}.dat",
            name,
            std::process::id(),
            unique
        ))
    }

    #[test]
    fn identity_can_be_saved_and_loaded() {
        let path = temporary_path("save-load");

        let identity = NodeIdentity::from_secret([7u8; 32]);

        let expected_address = identity.address();

        identity.save(&path).unwrap();

        let loaded = NodeIdentity::load(&path).unwrap();

        assert_eq!(loaded.address(), expected_address);

        let _ = fs::remove_file(path);
    }

    #[test]
    fn generated_identity_can_be_persisted() {
        let path = temporary_path("generated");

        let identity = NodeIdentity::generate();

        let address = identity.address();

        identity.save(&path).unwrap();

        let loaded = NodeIdentity::load(&path).unwrap();

        assert_eq!(loaded.address(), address);

        let _ = fs::remove_file(path);
    }

    #[test]
    fn invalid_identity_magic_is_rejected() {
        let path = temporary_path("bad-magic");

        let mut bytes = vec![0u8; 40];

        bytes[..8].copy_from_slice(b"BADNODE!");

        fs::write(&path, bytes).unwrap();

        assert!(NodeIdentity::load(&path).is_err());

        let _ = fs::remove_file(path);
    }

    #[test]
    fn truncated_identity_is_rejected() {
        let path = temporary_path("truncated");

        fs::write(&path, IDENTITY_MAGIC).unwrap();

        assert!(NodeIdentity::load(&path).is_err());

        let _ = fs::remove_file(path);
    }

    #[test]
    fn different_generated_nodes_have_different_addresses() {
        let first = NodeIdentity::generate();

        let second = NodeIdentity::generate();

        assert_ne!(first.address(), second.address());
    }
}

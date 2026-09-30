use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};

use crate::crypto::sha256;

pub const ADDRESS_PREFIX: &str = "tpztest1";

// Kept as a source compatibility alias for the original lab binaries.
pub const LAB_ADDRESS_PREFIX: &str = ADDRESS_PREFIX;

#[derive(Clone)]
pub struct KeyPair {
    signing_key: SigningKey,
}

impl KeyPair {
    pub fn from_secret_bytes(secret: [u8; 32]) -> Self {
        Self {
            signing_key: SigningKey::from_bytes(&secret),
        }
    }

    pub fn public_key_bytes(&self) -> [u8; 32] {
        self.signing_key.verifying_key().to_bytes()
    }

    pub fn address(&self) -> String {
        address_from_public_key(&self.public_key_bytes())
    }

    pub fn sign(&self, message: &[u8]) -> [u8; 64] {
        let signature: Signature = self.signing_key.sign(message);

        signature.to_bytes()
    }
}

pub fn address_from_public_key(public_key: &[u8; 32]) -> String {
    let hash = sha256(public_key);

    format!("{}{}", ADDRESS_PREFIX, hex::encode(hash),)
}

pub fn validate_address(address: &str) -> Result<(), String> {
    let suffix = address
        .strip_prefix(ADDRESS_PREFIX)
        .ok_or_else(|| "address uses an unknown Topazio network prefix".to_string())?;

    if suffix.len() != 64 {
        return Err("address must contain a 32-byte hexadecimal payload".to_string());
    }

    let decoded =
        hex::decode(suffix).map_err(|_| "address payload is not hexadecimal".to_string())?;
    if decoded.len() != 32 {
        return Err("address payload must contain 32 bytes".to_string());
    }

    Ok(())
}

pub fn verify_signature(public_key: &[u8; 32], message: &[u8], signature: &[u8; 64]) -> bool {
    let verifying_key = match VerifyingKey::from_bytes(public_key) {
        Ok(key) => key,
        Err(_) => return false,
    };

    let signature = Signature::from_bytes(signature);

    verifying_key.verify(message, &signature).is_ok()
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
    fn same_secret_produces_same_public_key() {
        let key_a = KeyPair::from_secret_bytes([7u8; 32]);

        let key_b = KeyPair::from_secret_bytes([7u8; 32]);

        assert_eq!(key_a.public_key_bytes(), key_b.public_key_bytes());
    }

    #[test]
    fn different_secrets_produce_different_public_keys() {
        let alice = alice_key();

        let bob = bob_key();

        assert_ne!(alice.public_key_bytes(), bob.public_key_bytes());
    }

    #[test]
    fn same_key_produces_same_address() {
        let key_a = alice_key();

        let key_b = alice_key();

        assert_eq!(key_a.address(), key_b.address());
    }

    #[test]
    fn different_keys_produce_different_addresses() {
        let alice = alice_key();

        let bob = bob_key();

        assert_ne!(alice.address(), bob.address());
    }

    #[test]
    fn address_uses_testnet_prefix() {
        let alice = alice_key();

        assert!(alice.address().starts_with(ADDRESS_PREFIX,));
    }

    #[test]
    fn valid_signature_is_accepted() {
        let alice = alice_key();

        let message = b"alice sends 25 TPZ to bob";

        let signature = alice.sign(message);

        assert!(verify_signature(
            &alice.public_key_bytes(),
            message,
            &signature,
        ));
    }

    #[test]
    fn modified_message_invalidates_signature() {
        let alice = alice_key();

        let signature = alice.sign(b"alice sends 25 TPZ");

        assert!(!verify_signature(
            &alice.public_key_bytes(),
            b"alice sends 250 TPZ",
            &signature,
        ));
    }

    #[test]
    fn wrong_public_key_invalidates_signature() {
        let alice = alice_key();

        let bob = bob_key();

        let message = b"transfer";

        let signature = alice.sign(message);

        assert!(!verify_signature(
            &bob.public_key_bytes(),
            message,
            &signature,
        ));
    }

    #[test]
    fn address_can_be_recreated_from_public_key() {
        let alice = alice_key();

        let recreated = address_from_public_key(&alice.public_key_bytes());

        assert_eq!(alice.address(), recreated);
    }
}

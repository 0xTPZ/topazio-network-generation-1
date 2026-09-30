use sha2::{Digest, Sha256};

pub type Hash = [u8; 32];

pub fn sha256(data: &[u8]) -> Hash {
    let digest = Sha256::digest(data);

    let mut hash = [0u8; 32];
    hash.copy_from_slice(&digest);

    hash
}

pub fn hash_to_hex(hash: &Hash) -> String {
    hex::encode(hash)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_data_produces_same_hash() {
        let data = b"topazio-network";

        let hash_a = sha256(data);
        let hash_b = sha256(data);

        assert_eq!(hash_a, hash_b);
    }

    #[test]
    fn different_data_produces_different_hashes() {
        let hash_a = sha256(b"topazio-network");
        let hash_b = sha256(b"topazio-network-2");

        assert_ne!(hash_a, hash_b);
    }

    #[test]
    fn sha256_hash_has_32_bytes() {
        let hash = sha256(b"tpz");

        assert_eq!(hash.len(), 32);
    }

    #[test]
    fn hash_hex_has_64_characters() {
        let hash = sha256(b"tpz");
        let hex = hash_to_hex(&hash);

        assert_eq!(hex.len(), 64);
    }
}

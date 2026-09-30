# Wallet

Wallet code derives Ed25519 identities and signs transactions. The wallet file format encrypts key material using Argon2id key derivation and ChaCha20-Poly1305 authenticated encryption, with random salt and nonce generation in the implementation. Addresses derive from the public identity data according to the format implemented here.

No real wallet, seed, key file, or user-associated address data is included. Tests use deterministic synthetic key fixtures. The historical wallet and address format should not be assumed compatible with later versions or suitable for real funds.

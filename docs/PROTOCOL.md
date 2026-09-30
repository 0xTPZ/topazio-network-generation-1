# Protocol and data model

The source defines blocks, transactions, account state, peer messages, RPC requests, and persistent encodings. Transaction format v2 carries amount, fee, account nonce, and an Ed25519 signature. Block serialization includes height, previous hash, timestamp, nonce, difficulty bits, and transactions. SHA-256 is used for the block/hash primitives.

The address and serialization details are those implemented in this snapshot and are not asserted to be wire-compatible with later Topazio generations. No native-token operation model or typed product-operation API is present. No external configured-genesis format is included.

This document intentionally summarizes structure without publishing literal network identities, genesis identifiers, or operational endpoints.

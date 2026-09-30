HISTORICAL SOFTWARE — NOT FOR PRODUCTION USE

# Topazio Network — Generation 1

_Early Rust Core / Testnet RC Era_

This repository preserves a selected Rust network-core snapshot from the Topazio project, dated by its containing snapshot directory to approximately 12 September 2026. It is the earliest Rust generation currently identified by the historical audit, not a claim that no earlier Rust work exists. An archive dated around 11 September remains unexamined.

The preserved code contains a blockchain implementation, SHA-256 hashing and proof-of-work validation, a node, persistent storage, transactions, a mempool, peer networking and synchronization, mining, wallets, RPC/CLI programs, and one integration test. It is a test/rehearsal-era artifact, not a current Topazio release and not suitable for production or handling real funds.

The Phase 7E snapshot was selected because it is a compact, source-complete Rust state before the later native-token/product API expansion and the current configured-genesis network runtime. It is related to, but not identical to, the later `v1.0.0-testnet-rc1` tree. See [history](docs/HISTORY.md) and [original state](docs/ORIGINAL-STATE.md).

This copy preserves historical code and its known limitations. It has not been modernized or repaired. Optional author metadata was removed from the copied Cargo manifest for privacy. No wallet files, chainstate, logs, local configuration, operational scripts, binaries, release archives, vendored dependencies, or current-generation code are included. The code's MIT metadata is retained, but the selected source did not include an original license text file; see [dependencies and licensing](docs/DEPENDENCIES.md).

The source tree used static defaults and interfaces that are unsafe for production deployment, including unauthenticated RPC and a wildcard P2P bind default. Do not run a node exposed to a network. Historical credentials or keys are treated as private; no real wallet material was selected. Synthetic deterministic key fixtures remain only in test code.

No Generation 2 token features or Generation 3 configured-genesis/public-testnet runtime were backported. Absence is part of the record. Read the [security notes](docs/SECURITY-HISTORICAL.md) before inspecting or building this historical code.

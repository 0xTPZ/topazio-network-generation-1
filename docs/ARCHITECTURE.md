# Architecture

The crate exposes modules for blocks, blockchain validation, consensus, cryptography, Genesis construction, identity, mempool, mining, networking, node behavior, state, storage, synchronization, transactions, wallet handling, configuration, and RPC. Executables provide node, miner, wallet, CLI, transaction-sender, and client/server utilities.

At a high level, signed transactions are checked against account state and queued in the mempool; blocks serialize transactions and header fields, carry SHA-256-derived identifiers, and are checked against proof-of-work and chain rules. The blockchain updates state and persists chain data. Node networking exchanges peer and synchronization messages. RPC exposes node operations to the local CLI/miner programs.

This is a code-level map, not a claim of production-grade fault tolerance. The snapshot contains both reusable modules and lab/rehearsal paths. No live chainstate or deployment settings accompany it.

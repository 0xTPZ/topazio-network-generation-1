# Mining

Mining code searches nonce values until the block hash satisfies its difficulty target. The standalone miner obtains candidate work over RPC, constructs a coinbase transaction, mines locally, and submits the result. Consensus code supplies the reward and difficulty parameters.

The snapshot's lab difficulty and reward rules describe this historical test/rehearsal state. There is no basis here for claiming production mining readiness or public-testnet activation.

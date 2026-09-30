# Consensus

The snapshot implements a proof-of-work chain using SHA-256 hashes and a leading-zero-bits difficulty check. Consensus code defines initial difficulty, block reward, coinbase maturity, adjustment behavior, expected supply, and reward halving. The observed schedule halves the initial 2.5 TPZ reward every 4.2 million blocks. These are historical source parameters, not recommendations or guarantees about any later network.

The code includes a static Genesis construction/verification concept and an expected value used by tests. This is not the later configured/official public-testnet Genesis runtime. The project state is test/rehearsal software; no mainnet readiness is established.

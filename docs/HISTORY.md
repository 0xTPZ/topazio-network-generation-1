# History

## Scope

This is a curated preservation of the Phase 7E Rust snapshot stored under a directory dated 12 September 2026. The date is a source-directory label and supports an approximate period of 11–13 September 2026; it is not a verified commit timestamp. Confidence in the snapshot's identity as a Rust Generation 1 representative is **medium**. Confidence that it is the first Rust version is **low / unresolved**, because the older archive dated around 11 September has not been examined.

## Why Phase 7E

It has a coherent source tree for a Rust chain core and node, plus manifests and a network integration test. It predates the later token/product expansion and current-generation configured-genesis runtime. Compared with the tagged RC1 tree, its 27 `src` Rust files share paths with the RC tree, but only eight of those shared files were byte-identical in the earlier comparison. The RC1 tree is therefore a related, later state, not a duplicate or safe substitute.

## Wider project context

TopazioCoin PHP History and TopazioCoin Genesis (Python) are known 2025 experiments. The historical record does not establish direct code lineage from those experiments to this Rust implementation. A gap in the technical chronology remains. The Rust tree is a substantial technical stage with a chain, persistent state, PoW, networking, wallet, miner, and RPC/CLI.

## Later generations

Generation 2, identified around 21–23 September 2026, adds native-token and product API work, including typed-operation signing. Generation 3 adds configured Genesis, P2P chain identity, and Public Testnet V1 and is the current ecosystem. Neither later generation is included here.

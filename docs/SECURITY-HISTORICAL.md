# Historical security notes

This software is preserved for study, not deployment. Known limitations include unauthenticated RPC, a wildcard P2P bind default, test-oriented networking and consensus assumptions, and no established mainnet readiness gate. Do not expose a node or use it to protect funds.

The selection scan found no private-key markers, private-key hex candidates, wallet files, seed phrases, credentials, or personal email addresses. Key-like byte arrays in the source are deterministic synthetic test fixtures. Operational scripts and TOML configurations were excluded. The only broad IP-pattern matches in the source selection were loopback/wildcard/documentation-reserved values; none was an RFC1918 or globally routable operational address.

Secret scanning is pattern-based and cannot prove the absence of every possible secret. The staged and remote states were separately scanned before/after publication. No secret value is reproduced in this documentation.

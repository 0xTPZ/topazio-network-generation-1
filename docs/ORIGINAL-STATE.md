# Original state and preservation record

- Historical source: Phase 7E snapshot directory dated 2026-09-12.
- Historical period: circa 11–13 September 2026; medium confidence for this representative, unresolved confidence for being the earliest Rust version.
- Scope: 27 Rust source files, one Rust integration test, `Cargo.toml`, and `Cargo.lock`; 30 selected source files total.
- Public copy: source code preserved in place except removal of the optional author metadata line from the copied Cargo manifest. New documentation and ignore rules were authored for this archive.
- Excluded: source README/docs/configs/scripts/systemd files, build outputs, caches, logs, chainstate, wallets, archives, releases, vendored dependencies, and all Generation 2/3 material.
- Related RC: `v1.0.0-testnet-rc1` is later and related; it is not the source copied here.
- Older archive: the approximately 2026-09-11 ZIP remains uninspected; the repository makes no definitive first-version claim.
- Publication/preservation date: 2026-09-30.
- Source handling: `E:\Topazio` was read-only for this task. The source tree showed concurrent activity during the task; observed count changes are not attributed to this preservation.
- Build: PASS (`cargo test --locked --offline`, compiled successfully in the copy).
- Original tests: PASS — 220 tests passed across library, binaries, integration test, and doc-test targets; no source changes were made to obtain this result.
- Smoke test: NOT ATTEMPTED; no live network service was launched.
- Scan results: original selection, sanitized copy, staged index, and remote tree/history were checked before/after publication. Pattern scans are not mathematical proof of absence.



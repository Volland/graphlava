# Proposal

## Why

The engine bake-off (`spikes/bakeoff/RESULTS.md`) kept redb 2.6.4 but found that redb 4.6 removes most of its weak spots: relaxed mix p99 167 µs instead of 453 µs, 257 instead of 411 bytes per statement, almost no file growth under non-durable commits, and 280k instead of 175k reads/s at 16 readers. The file format freezes at the end of v0 ([[18-roadmap#Format freeze]]), so the upgrade is cheapest before it.

## What Changes

- Raise the project MSRV from 1.87 to 1.90, which redb 4.x needs, in `Cargo.toml`, the CI MSRV job, `openspec/config.yaml` and the README.
- Move `crates/graphlava` from redb 2.6.4 to 4.6.x: the storage adapter, the fault-injecting backend, the durability mapping (4.x has no `Durability::Eventual`), and quick repair (rule `tx-quick-repair`).
- Re-check the redb properties the design relies on ([[06-transactions-durability#Durability levels]], rule `tx-durability-verify`) on 4.x with the bake-off harness: durable commit after non-durable ones, crash repair time, savepoint after write.
- Rerun the crash harness, the refinement replay, the W3C suites and the micro-benchmark baseline; rerun bake-off runs A and C on 4.x.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

None. Requirements are unchanged; the engine under them changes ([[adr:0002]] is amended, not replaced).

## Impact

- **Code:** `crates/graphlava/src/storage/`, `Cargo.toml`, `Cargo.lock`, CI.
- **File format:** redb's own format may change between 2.6 and 4.6; files written before the upgrade are migrated or rejected with `FormatMismatch`. Since no user data is kept before the freeze, rejection is acceptable.
- **Bindings:** the v1 binding crates build on the core; they must build with Rust 1.90.

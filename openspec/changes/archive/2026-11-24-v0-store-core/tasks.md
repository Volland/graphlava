# Tasks

Numbers in parentheses are the task ids of the former `v0-core` change.

## 1. Gate: decisions and bake-off

- [x] 1.1 Decide every gate-v0 question in `tg.md/open-questions.md`; record each as an ADR and add a `resolved_by` edge (1.3)
- [x] 1.2 Update specs and tasks for the decisions of 1.1 (1.4)
- [x] 1.3 Build the bake-off harness in `spikes/bakeoff/` (redb, libmdbx, Sanakirja behind a minimal `Storage` trait) and run [[17-benchmarks#Engine bake-off]] and [[17-benchmarks#Ordering count comparison]] at 10M statements (1.1)
- [x] 1.4 Measure redb crash-repair time at 10M and 100M statements and confirm whether redb allows a savepoint after writes (15.10)
- [x] 1.5 Confirm or reopen [[adr:0002]] and [[adr:0015]] from the bake-off results (1.2)

## 2. Workspace and storage

- [x] 2.1 Create the Rust workspace (core crate, FFI crate) with CI on Linux, macOS and Windows, and iOS/Android build jobs (2.1)
- [x] 2.2 Implement the `Storage` trait and redb adapter ([[05-storage-trait#Trait API]], [[05-storage-trait#redb adapter]]) (2.2)
- [x] 2.3 Implement the v1 table layout and key byte order ([[04-physical-layout#Tables]], [[04-physical-layout#Key byte order]]) (2.3)

## 3. Formal model

- [x] 3.1 Create the `formal/` Lake project with `MetaCore.Id`, `MetaCore.Store`, `MetaCore.Nesting`, `MetaCore.Durability`, zero `sorry` (11.1)
- [x] 3.2 Add the axiom audit `formal/scripts/lean-audit.sh` and the proof ledger [[20-proofs]] with `proves::` edges (11.2)
- [x] 3.3 Add the executable oracle `lake exe oracle <seed> <n>` emitting JSON-lines traces (11.3)
- [x] 3.4 Close PO-ID-STRING and PO-ID-DOUBLE for the prefix-coded layout, and PO-CASCADE-FUEL (11.5)
- [x] 3.5 Write `crates/graphlava/tests/refinement.rs`: replay seeds 1..64 × 2,000 operations through the public API and compare results and the final state through the abstraction function (11.4)
- [x] 3.6 Add a debug assertion in the Rust store for each `<assumption>` in [[20-proofs]] (11.8)

## 4. term-identity

- [x] 4.1 Implement ID encoding; annotate `@tg: implements:: [[openspec:term-identity#Stable term identity]]` (3.1)
- [x] 4.2 Implement the prefix-coded inline literal layout ([[adr:0021]]) with the `id_layout_matches_model` and `double_magnitude_order` property tests; annotate `[[openspec:term-identity#Order-preserving inline literals]]` (3.2, 15.1)
- [x] 4.3 Implement canonicalisation; annotate `[[openspec:term-identity#Canonical literal forms]]` (3.3)
- [x] 4.4 Implement the dictionary, the language-tag table ([[adr:0033]]), the `dt` table, the namespace cap, owned literals and the mark-and-sweep with the kept set; annotate `[[openspec:term-identity#Dictionary reclamation without write cost]]` (3.4, 15.3)
- [x] 4.5 Write a test per scenario in `specs/term-identity/spec.md`, each annotated `@tg: verifies::` (3.5, 15.12)

## 5. statement-store

- [x] 5.1 Implement statements, the `stmt` record (with the `unasserted` flag of [[adr:0032]]) and in-place update; annotate `[[openspec:statement-store#Statement identity under update]]` (4.1)
- [x] 5.2 Implement unique keys, `multi` and `functional` predicates and compare-and-set; annotate `[[openspec:statement-store#Unique statements by default]]` and `[[openspec:statement-store#Functional predicates update in place]]` (4.2, 15.8)
- [x] 5.3 Implement inline property records, `plist`, the sort-key `propidx`, spill at 256 entries or 2 KiB ([[adr:0034]]) and value lookups; annotate `[[openspec:statement-store#Inline literal properties]]` (4.3, 15.2)
- [x] 5.4 Implement promotion; annotate `[[openspec:statement-store#Promotion on annotation]]` (4.4)
- [x] 5.5 Implement cascade delete with the object-position guard and `DETACH` ([[adr:0036]]); annotate `[[openspec:statement-store#Cascade delete of annotations]]` (4.5)
- [x] 5.6 Implement SPO, POS, OSP and on-demand orderings; annotate `[[openspec:statement-store#Index coverage of base orderings]]` (4.6)
- [x] 5.7 Write a test per scenario in `specs/statement-store/spec.md`, each annotated with its `@tg: verifies::` id (4.7, 15.12)

## 6. transactions

- [x] 6.1 Implement the owner-process lock; annotate `[[openspec:transactions#Single owner process]]` (5.1)
- [x] 6.2 Implement read snapshots and the single writer; annotate `[[openspec:transactions#Snapshot reads and one writer]]` (5.2)
- [x] 6.3 Implement `sync`, `ordered` and `relaxed`, with one engine transaction per batch, per-request overlays and check-then-apply ([[adr:0025]]); measure bytes per request at batch sizes 1, 8 and 64; annotate `[[openspec:transactions#Per-transaction durability levels]]` and `[[openspec:transactions#Batched commit]]` (5.3, 15.5)
- [x] 6.4 Implement the core API, one-shot write requests and the in-process writer time limit ([[adr:0026]]); annotate `[[openspec:transactions#One-shot remote writes]]` (15.6)
- [x] 6.5 Make allocation, cascades and index updates atomic; annotate `[[openspec:transactions#Atomic transactions]]` (5.4)
- [x] 6.6 Implement the format version and feature flags; annotate `[[openspec:transactions#Portable file format]]` (5.5)
- [x] 6.7 Create `crates/graphlava/tests/e2e/` with a temp-dir fixture that opens, closes and reopens a real file through the public API only (12.1)
- [x] 6.8 Build the crash harness: a child writer killed at every I/O step, checked against `MetaCore.Durability.recover` outcomes (12.3)
- [x] 6.9 Write crash and cross-platform tests per scenario in `specs/transactions/spec.md`, each annotated with its `@tg: verifies::` id (5.6, 15.12)

## 7. Documentation and checks

- [x] 7.1 Record new decisions or constraints from implementation in lat.md as typed sections (14.1)
- [x] 7.2 Add `openspec:` frontmatter: term-identity → `02-id-encoding`, `03-dictionary`; statement-store → `01-data-model`, `04-physical-layout`; transactions → `06-transactions-durability` (14.2)
- [x] 7.3 Run `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`, `lake build`, `formal/scripts/lean-audit.sh`, `tg check`, `tg ontology check` and `tg trace term-identity statement-store transactions --gaps`, and close every gap (14.3)

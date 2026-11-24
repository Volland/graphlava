# Proposal

## Why

graphlava needs a working store before any query surface can exist ([[adr:0013]]). This is the first of four slices of the former `v0-core` change: the engine gate, the storage layer, term identity, statements and transactions. Each slice can be verified and archived on its own.

## What Changes

- Gate: the engine bake-off spike (redb, libmdbx, Sanakirja at 10M statements) and the redb crash-repair measurement. The gate-v0 open questions are decided in ADRs 0031–0041.
- `Storage` trait and the redb adapter; the v1 table layout and key byte order.
- 64-bit term IDs with kind tags, chunk allocation, prefix-coded order-preserving inline literals ([[adr:0011]], [[adr:0021]]); canonical literal forms; the term dictionary with the language-tag table, the `dt` table, owned literals and mark-and-sweep ([[adr:0023]], [[adr:0033]]).
- Statements with stable IDs, in-place update, unique `(s,p,o)` with `multi` and `functional` predicates, inline literal properties with `plist` and the sort-key `propidx`, promotion, cascade delete with the object-position guard ([[adr:0004]], [[adr:0005]], [[adr:0006]], [[adr:0022]], [[adr:0028]], [[adr:0036]]); SPO/POS/OSP orderings ([[adr:0015]]).
- Transactions: owner-process lock, snapshot reads, one writer, `sync` / `ordered` / `relaxed`, batched group commit, one-shot write requests and compare-and-set, format version and feature flags ([[adr:0002]], [[adr:0012]], [[adr:0018]], [[adr:0025]], [[adr:0026]]).
- Refinement replay against the Lean oracle and the crash harness.

## Capabilities

### New Capabilities

- `term-identity`: 64-bit IDs, inline literals, canonical forms, the dictionary and its sweep.
- `statement-store`: statements with stable IDs, inline properties, promotion, annotations, orderings and cascade delete.
- `transactions`: the single-owner database, transactions, durability levels and crash behaviour.

### Modified Capabilities

None.

## Impact

- **Code:** `crates/graphlava` (storage, ids, dictionary, store, tx modules), `spikes/bakeoff/`.
- **Dependencies:** redb (pinned exactly); libmdbx and sanakirja in the spike only.
- **lat.md sections:** [[01-data-model]], [[02-id-encoding]], [[03-dictionary]], [[04-physical-layout]], [[05-storage-trait]], [[06-transactions-durability]].
- **File format:** defines the v0 bytes (ID layout, key order, tables, canonicalisation, feature flags); frozen only in `v0-hardening-freeze`.
- **Verification:** term-identity L0 (`MetaCore.Id.*`), L1, L2, L3; statement-store L0 (`MetaCore.Store.*`), L1, L3; transactions L0 (`MetaCore.Durability.*`), L3, L4. PO-ID-STRING, PO-ID-DOUBLE and PO-CASCADE-FUEL close here.

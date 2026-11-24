---
type: Proof
status: partial
module: formal/MetaCore
specs: ["adr:0019"]
---
# Proof ledger

Every Lean 4 theorem in `formal/` that discharges a requirement, with its assumptions, plus the proof obligations that are still open. `lake build` checks the theorems; this file traces them to requirements.

- cites:: [[19-verification#Formal model]] {intent: usesMethodIn}

Each section is one proof or one open obligation (`PO-*`). Edges point at the `<rule>` elements of the layer specs; the matching OpenSpec requirement is named in the `OpenSpec:` line until the `v0-core` change is archived into `openspec/specs/`, after which the edges should target `[[openspec:<capability>#<Requirement>]]` directly. A `proves::` edge carries the Lean name in `theorem` and its coverage in `status`. An `<assumption>` is something the Rust code must guarantee for the theorem to transfer; each one is checked by a test or a debug assertion.

## Kind tag and ID injectivity

The kind of an ID is readable from the ID alone, distinct (kind, payload) pairs give distinct IDs, and IDs sort by kind first, so every inline literal sorts after every non-literal.

OpenSpec: `term-identity` / Stable term identity, scenario "Kind is readable from the ID".

- proves:: [[02-id-encoding#Bit layout]] {theorem: "MetaCore.Id.kind_mk, MetaCore.Id.mk_injective, MetaCore.Id.mk_lt_u64", status: partial}
- proves:: [[02-id-encoding#^id-kind-tag]] {theorem: "MetaCore.Id.mk_lt_of_kind_lt, MetaCore.Id.null_least", status: proved}

<assumption>

The Rust `Id::new(kind, payload)` equals `kind << 60 | payload` for `payload < 2^60` and `kind < 8`; inline literals are `0b10 << 62 | payload` (doubles) and `0b11 << 62 | subtype << 58 | payload` (others). Property test `id_layout_matches_model` checks both against the arithmetic definitions.

</assumption>

## Chunk split and system graph

For chunked kinds the chunk number and offset are recovered exactly from the ID, so the home graph is a pure function of the ID. The system graph's ID is `0x5000_0000_0000_0000`.

- proves:: [[02-id-encoding#^id-chunk-split]] {theorem: "MetaCore.Id.chunkOf_chunked", status: proved}
- proves:: [[02-id-encoding#^id-graph-bootstrap]] {theorem: "MetaCore.Id.system_graph_id", status: proved}

## Inline integer order

The offset-binary encoding of integers in [-2^57, 2^57 − 1] round-trips, and unsigned ID order equals integer order, so `?v > c` is a key range within the integer run. Every inline literal sorts after every non-literal ID ([[adr:0021]]).

OpenSpec: `term-identity` / Order-preserving inline literals, scenario "Integer range filter uses value order".

- proves:: [[02-id-encoding#Inline literals]] {theorem: "MetaCore.Id.decInt_encInt, MetaCore.Id.encInt_lt_iff", status: partial}
- proves:: [[02-id-encoding#^id-kind-tag]] {theorem: "MetaCore.Id.encInt_literal, MetaCore.Id.nonliteral_lt_encInt, MetaCore.Id.encDbl_literal", status: proved}
- proves:: [[02-id-encoding#^id-numeric-adjacent]] {theorem: "MetaCore.Id.double_before_int, MetaCore.Id.int_before_decimal", status: proved}

Partial: dates and dateTimes use the same offset-binary transform and inherit the theorem by instantiation; strings are PO-ID-STRING.

<assumption>

Composite keys store IDs big-endian, so byte order equals numeric order ([[04-physical-layout#Key byte order]]); covered by property test `key_bytes_order_eq_u64_order`.

</assumption>

## Inline double order

The sign-flip transform on the 61-bit magnitude field of an inline double is strictly monotone in value order, with `-0.0` just below `+0.0` as distinct IDs, and every inline double sorts before every inline integer.

- proves:: [[02-id-encoding#^id-double-inline]] {theorem: "MetaCore.Id.encDbl_strictMono, MetaCore.Id.negZero_lt_posZero", status: partial}

Partial: the model takes the magnitude field as given. That the 9-bit rebiased exponent followed by the mantissa orders non-negative doubles by value is the remaining part of PO-ID-DOUBLE.

<assumption>

For non-negative IEEE doubles in the inline range, the bit pattern with the exponent rebiased by `-767` (from 1023 to 256) is monotone in value; property test `double_magnitude_order` checks it on 10^7 random pairs and the range ends.

</assumption>

## Never reuse an ID

The allocator high-water mark is monotone under insert, update and cascade delete, and a fresh ID equals the old mark, so it is above every ID ever handed out.

OpenSpec: `term-identity` / Stable term identity, scenario "Deleted IDs are not reused".

- proves:: [[02-id-encoding#^id-never-reused]] {theorem: "MetaCore.Store.insert_next_mono, MetaCore.Store.insert_id_fresh_or_existing, MetaCore.Store.update_next, MetaCore.Store.cascadeDelete_next", status: proved}
- proves:: [[01-data-model#^dm-id-stable]] {theorem: "MetaCore.Store.cascadeDelete_next", status: proved}

<assumption>

The allocator counter commits in the same redb transaction as the writes that use it ([[02-id-encoding#^id-alloc-atomic]]); covered by the crash sweep.

</assumption>

## Unique statements

Insert and update preserve the invariant `WF`, which includes at most one statement per `(s,p,o)` on a non-multi predicate. Re-inserting is a no-op returning the same ID; on a multi predicate it yields a new ID.

`WF` also requires positive IDs below the allocator mark and no duplicate IDs.

OpenSpec: `statement-store` / Unique statements by default, scenarios "Repeated insert is idempotent" and "Multi predicate keeps parallel statements".

- proves:: [[01-data-model#^dm-unique-spo]] {theorem: "MetaCore.Store.insert_wf, MetaCore.Store.update_wf, MetaCore.Store.insert_idempotent", status: proved}
- proves:: [[01-data-model#^dm-multi-predicate]] {theorem: "MetaCore.Store.insert_multi_distinct", status: partial}

<assumption>

The `multi` flag of a predicate does not change during a transaction; changing it is a schema migration ([[01-data-model#^dm-multi-predicate]]).

</assumption>

## Update in place

An update keeps the set of IDs and the allocator unchanged, makes the same ID read the new object, and leaves every other statement untouched, so annotations stay attached.

OpenSpec: `statement-store` / Statement identity under update, scenario "Overwrite keeps ID and annotations".

- proves:: [[01-data-model#^dm-update-in-place]] {theorem: "MetaCore.Store.update_ids, MetaCore.Store.update_lookup, MetaCore.Store.update_preserves_others, MetaCore.Store.update_wf", status: proved}

## Cascade delete

A successful cascade delete removes the deleted statement, keeps `WF`, and leaves no annotation whose subject is a missing statement (`RefClosed`). The closure is checked at runtime, so a fuel bug yields an error, never a dangling annotation.

OpenSpec: `statement-store` / Cascade delete of annotations, scenario "Annotation chain is removed".

- proves:: [[01-data-model#^dm-delete-cascade]] {theorem: "MetaCore.Store.cascadeDelete_correct", status: partial}
- proves:: [[06-transactions-durability#^tx-cascade-annotations]] {theorem: "MetaCore.Store.removeAll_refClosed", status: partial}

Partial: the hyperedge guard and keep-annotations mode are not modelled (tests only), and fuel adequacy is PO-CASCADE-FUEL.

## Acyclic nesting

Adding `child :in parent` when `parent` is not already under `child` keeps nesting acyclic, and the guard is also necessary: a write that violates it really would close a cycle.

OpenSpec: `named-graphs` / Acyclic nesting, scenario "Cycle rejected".

- proves:: [[09-graphs#^gr-dag-cycle]] {theorem: "MetaCore.Nesting.addEdge_acyclic, MetaCore.Nesting.guard_necessary", status: proved}
- proves:: [[01-data-model#^dm-nesting-dag]] {theorem: "MetaCore.Nesting.addEdge_acyclic", status: partial}

Partial: the depth limit of 64 is not modelled; the reachability check that implements the guard is tested against `Reach` by property test.

## Transitive scope

Direct members are in the transitive scope, and scope composes: whatever is in a child's scope is in every ancestor's scope.

OpenSpec: `named-graphs` / Transitive graph scope by default, scenario "Parent scope includes children".

- proves:: [[09-graphs#^gr-scope-transitive]] {theorem: "MetaCore.Nesting.direct_sub_transitive, MetaCore.Nesting.scope_trans", status: partial}

Partial: "Shared child counts once" concerns the result multiset of the executor and is an E2E scenario ([[21-e2e-tests#Graph scope and nesting]]).

## Durable prefix

A crash leaves the first `k` whole transactions with `durableBound ≤ k ≤ length`: every acknowledged `sync` or `ordered` commit survives, a `relaxed` commit followed by a durable one survives, and recovery never applies part of a transaction.

OpenSpec: `transactions` / Per-transaction durability levels and Atomic transactions, scenarios "Sync commit survives a crash", "Ordered commit is acknowledged after its group is durable", "Relaxed commit may be lost but never corrupts".

- proves:: [[06-transactions-durability#^tx-durability-prefix]] {theorem: "MetaCore.Durability.durable_survives, MetaCore.Durability.relaxed_covered", status: partial}
- proves:: [[06-transactions-durability#^tx-atomic]] {theorem: "MetaCore.Durability.recover, MetaCore.Durability.crash_outcome_exists", status: partial}

<assumption>

redb commits are totally ordered and atomic, and a durable commit makes every earlier commit durable. This is a property of the engine, checked by the crash sweep on every supported OS ([[06-transactions-durability#Crash recovery]]).

</assumption>

## Open obligations

Stated but not yet proved; each names its planned module. Closing one moves it to a section above.

- **PO-CASCADE-FUEL** (`MetaCore.Store`): `closure st (|stmts|+1) [id]` is closed whenever `WF st`. Needs a cardinality argument on `Nodup` lists.
- **PO-ID-STRING** (`MetaCore.Id`): big-endian packing of strings of at most 7 bytes, zero padded, preserves lexicographic order.
- **PO-ID-DOUBLE** (`MetaCore.Id`): the rebiased magnitude field orders non-negative doubles by value. The sign-flip transform itself is proved (`encDbl_strictMono`).
- **PO-CANON** (`MetaCore.Canon`): canonicalisation is idempotent and equal values of one datatype get one canonical form.
- **PO-PROMOTE** (`MetaCore.Store`): promoting an inline property changes no query result, only its ID becomes visible ([[01-data-model#^dm-props-virtual]]).
- **PO-STRAT** (`MetaCore.Datalog`): a program accepted by the stratifier has a unique perfect model, and the stratifier rejects every program with a cycle through negation.
- **PO-SEMINAIVE** (`MetaCore.Datalog`): semi-naive evaluation computes the same fixpoint as naive evaluation.
- **PO-MAGIC** (`MetaCore.Datalog`): magic-set rewriting preserves the answers to the goal.
- **PO-DROP** (`MetaCore.Nesting`): plain drop leaves descendants as roots; recursive drop keeps a descendant that has a surviving parent.

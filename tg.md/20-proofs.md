---
type: Proof
status: partial
module: formal/MetaCore
specs: ["adr:0019"]
---
# Proof ledger

Every Lean 4 theorem in `formal/` that discharges a requirement, with its assumptions, plus the proof obligations that are still open. `lake build` checks the theorems; this file traces them to requirements.

- cites:: [[19-verification#Formal model]] {intent: usesMethodIn}

Each section is one proof or one open obligation (`PO-*`). Edges point at the `<rule>` elements of the layer specs; the matching OpenSpec requirement is named in the `OpenSpec:` line until the v0 slice that adds it (`v0-store-core`, `v0-graphs-hyperedges`, `v0-query`) is archived into `openspec/specs/`, after which the edges should target `[[openspec:<capability>#<Requirement>]]` directly. A `proves::` edge carries the Lean name in `theorem` and its coverage in `status`. An `<assumption>` is something the Rust code must guarantee for the theorem to transfer; each one is checked by a test or a debug assertion.

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

Partial: dates and dateTimes use the same offset-binary transform and inherit the theorem by instantiation; strings are proved separately ([[20-proofs#Inline string order]]).

<assumption>

Composite keys store IDs big-endian, so byte order equals numeric order ([[04-physical-layout#Key byte order]]); covered by property test `key_bytes_order_eq_u64_order`.

</assumption>

## Inline double order

Unsigned ID order equals value order for every inline double, negatives included, with `-0.0` just below `+0.0` as distinct IDs, and every inline double sorts before every inline integer. This closes PO-ID-DOUBLE.

A non-negative inline double is `+0`, `+∞`, or a normal value with IEEE biased exponent `e` in [768, 1277] (unbiased [-255, 254]) and 52-bit mantissa `m`; its value times 2^1075 is `(2^52 + m) · 2^e`. The magnitude field `(e − 767) · 2^52 + m` (`0` for zero, `511 · 2^52` for infinity) is below 2^61 and strictly monotone in value, so magnitude order is value order; the sign-flip transform (`encDbl`) then extends this to negatives.

OpenSpec: `term-identity` / Order-preserving inline literals.

- proves:: [[02-id-encoding#^id-double-inline]] {theorem: "MetaCore.Id.PosDbl.mag_strictMono, MetaCore.Id.PosDbl.mag_lt_iff, MetaCore.Id.PosDbl.mag_lt_D, MetaCore.Id.encDblV_strictMono, MetaCore.Id.encDbl_strictMono, MetaCore.Id.negZero_lt_posZero", status: proved}
- proves:: [[02-id-encoding#^id-kind-tag]] {theorem: "MetaCore.Id.encDblV_literal", status: proved}

<assumption>

The Rust encoder computes the magnitude field exactly as `PosDbl.mag`: for a normal double with biased exponent `e` in [768, 1277] it is `(e − 767) << 52 | mantissa`, `0` for `±0.0`, and `511 << 52` for `±∞`; every other double (NaN, subnormal, out of range) goes to the dictionary. The sign selects `2^61 + mag` or `2^61 − 1 − mag` (`encDbl`). Property test `double_magnitude_order` checks the encoder against `f64` comparison on 10^7 random pairs and the range ends, and that the rebiased exponent of an inline value is in 1..510.

</assumption>

## Inline string order

Inline string IDs are injective and sort in lexicographic byte order, provided the string does not end in `0x00`; the same holds for inline `rdf:langString`, ordered by text then language. This closes PO-ID-STRING.

An inline `xsd:string` of at most 7 bytes is packed big-endian, zero padded on the right, into 56 bits placed at payload bits 57..2: the ID is `lit 5 (pack bs · 4)`. Zero padding makes `s` and `s ++ [0x00]` collide, so the inline precondition (`InlineStr`) is: every byte below 256, at most 7 bytes, and the last byte (if any) is not `0x00`. Under it the encoding is injective, unsigned ID order equals lexicographic byte order (a proper prefix sorts first), and every inline string stays inside the subtype `0x5` run. `rdf:langString` (subtype `0x6`) packs 5 bytes the same way, then a 16-bit language ID.

OpenSpec: `term-identity` / Order-preserving inline literals.

- proves:: [[02-id-encoding#Inline literals]] {theorem: "MetaCore.Id.encStr_injective, MetaCore.Id.encStr_lt_iff, MetaCore.Id.encStr_in_run, MetaCore.Id.pack_payload_lt, MetaCore.Id.encLangStr_lt_iff, MetaCore.Id.langPayload_lt", status: proved}
- proves:: [[02-id-encoding#^id-inline-range]] {theorem: "MetaCore.Id.packN_injective, MetaCore.Id.packN_lt_iff", status: partial}

Partial for `id-inline-range`: the rule says every string of at most 7 bytes is inline; the proof needs the last byte to be nonzero, so a string ending in `0x00` must go to the dictionary (or the rule must be narrowed).

<assumption>

Inline strings never end with a `0x00` byte; such strings are dictionary literals. The Rust packing equals `packN 7`: bytes in order, big-endian, zero padded to 7 bytes, shifted left by 2. For `rdf:langString` the payload is `(packN 5 bytes << 16 | lang) << 2` with the same no-trailing-zero rule. Property test `string_pack_order` checks injectivity and that ID order equals byte-wise lexicographic order on random strings of 0..7 bytes, including prefix pairs.

</assumption>

## Never reuse an ID

The allocator high-water mark is monotone under insert, update and cascade delete, and a fresh ID equals the old mark, so it is above every ID ever handed out.

Statements and hyperedges share one allocator: a hyperedge takes the mark as its ID, a rejected hyperedge allocates nothing, and the mark is monotone under every operation of the oracle step function.

OpenSpec: `term-identity` / Stable term identity, scenario "Deleted IDs are not reused".

- proves:: [[02-id-encoding#^id-never-reused]] {theorem: "MetaCore.Store.insert_next_mono, MetaCore.Store.insert_id_fresh_or_existing, MetaCore.Store.update_next, MetaCore.Store.cascadeDelete_next", status: proved}
- proves:: [[01-data-model#^dm-id-stable]] {theorem: "MetaCore.Store.cascadeDelete_next", status: proved}
- proves:: [[02-id-encoding#^id-never-reused]] {theorem: "MetaCore.Hyperedge.create_some, MetaCore.Hyperedge.create_none_iff, MetaCore.Hyperedge.cascadeDelete_next, MetaCore.Graphs.apply_next_mono", status: proved}

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

A successful delete removes the statement, its annotations and (with `DETACH`) its object referrers, keeps `WF`, and leaves no dangling subject or object; without `DETACH` an outside object reference blocks it ([[adr:0036]]). Fuel never runs out.

Deleting X computes a doomed set D: X, every statement annotating a member (subject position, always), and with `DETACH` every statement naming a member in object position, transitively. Without `DETACH` the delete fails and changes nothing when a statement outside D names a member of D in object position ([[adr:0036]]). A successful delete removes exactly D, keeps `WF`, removes X, and leaves no statement whose subject or object is a missing statement (`RefClosed`, which now covers both positions). The guard is necessary: a blocked delete really has an outside object reference, and removing D would leave it dangling. `DETACH` is never blocked.

The closure runs with fuel `|stmts| + 1`, and that fuel always reaches a closed set: the number of entries outside D drops on every round that adds something, and a round that adds nothing is a fixpoint. So a delete fails only on the object guard, never for fuel. This closes PO-CASCADE-FUEL (no `WF` needed).

OpenSpec: `statement-store` / Cascade delete of annotations, scenario "Annotation chain is removed".

- proves:: [[01-data-model#^dm-delete-cascade]] {theorem: "MetaCore.Store.cascadeDelete_correct, MetaCore.Store.cascadeDelete_removed, MetaCore.Store.closure_closed", status: partial}
- proves:: [[06-transactions-durability#^tx-cascade-annotations]] {theorem: "MetaCore.Store.removeAll_refClosed, MetaCore.Store.closure_closed", status: partial}
- proves:: [[06-transactions-durability#Cascade delete#References from the object position]] {theorem: "MetaCore.Store.cascadeDelete_blocked, MetaCore.Store.cascadeDelete_none_iff, MetaCore.Store.cascadeDelete_detach_some", status: proved}

Partial: keep-annotations mode is not modelled (tests only). The hyperedge guard is modelled on the extended store ([[20-proofs#Hyperedge guard on delete]]), whose cascade runs this statement cascade unchanged, so `closure_closed` covers it too.

<assumption>

The Rust delete computes the same D: breadth-first over SPO prefixes for annotations and, under `DETACH`, OSP prefixes for statements naming a member in object position. Without `DETACH` it fails with nothing written when, after D is complete, an OSP probe finds a statement not in D whose object is a member of D (a reference from inside D does not block). The oracle trace (`formal/Main.lean`, ~1 delete in 3 with `detach`) checks the removed set and the blocked outcome against the model, now through `MetaCore.Graphs.apply`, which also deletes hyperedges and runs the hyperedge guard.

</assumption>

## Hyperedge shape

A hyperedge write is accepted only when its roles satisfy its type, so it has at most the type's arity, exactly that arity when every role is required; a rejected write allocates nothing.

Satisfying the type means: every role named is declared, no role is named twice (all cardinalities are one), every required role is present, and every filler is a node or a live statement, never a literal. An accepted hyperedge therefore has at most the type's arity, and exactly that arity when every role is required. A rejected write changes nothing and allocates no ID.

The model is `MetaCore.Hyperedge`: `HType` (declared and required roles), `Satisfies` (the shape as a proposition), `shapeB` and `validB` (the executable check, `shapeB_iff`), and `create`. The extended invariant `HWF` adds to `WF`: hyperedge IDs positive, below the shared allocator, distinct and disjoint from statement IDs, each hyperedge of a declared type it satisfies, no literal filler.

OpenSpec: `hyperedges` / Typed hyperedges with roles, scenarios "Create a meeting hyperedge" and "Missing role rejected".

- proves:: [[08-hyperedges#^he-type-declared]] {theorem: "MetaCore.Hyperedge.shapeB_iff, MetaCore.Hyperedge.create_some, MetaCore.Hyperedge.create_none_iff, MetaCore.Hyperedge.create_hwf", status: partial}
- proves:: [[08-hyperedges#^he-type-fixed]] {theorem: "MetaCore.Hyperedge.satisfies_arity, MetaCore.Hyperedge.satisfies_fixed_arity", status: partial}

Partial: roles of cardinality `many`, node-kind constraints on roles, literal attributes and the record layout are not modelled.

<assumption>

The Rust validator rejects exactly the role lists `shapeB` rejects, with `HyperedgeShapeMismatch`, and allocates no ID on rejection. The oracle (`formal/Main.lean`) emits about one invalid hyperedge in three (missing required role, undeclared role, role named twice) against the two schema types of the trace header.

</assumption>

## Hyperedge guard on delete

A statement filling a role blocks its delete unless `DETACH`, which deletes the whole hyperedge; a filler is never nulled, and no surviving hyperedge is left with a deleted filler.

A hyperedge can always be deleted by ID. After a successful delete no surviving hyperedge has a deleted statement as a filler.

`MetaCore.Hyperedge.cascadeDelete` extends the statement cascade: on a hyperedge ID it removes that hyperedge only (nothing refers to a hyperedge, so it never blocks); on a statement it runs `MetaCore.Store.cascadeDelete` (same doomed set D, same object guard) and then the hyperedge guard. Without `DETACH` a hyperedge with a filler in D blocks the delete; with `DETACH` every such hyperedge is removed whole and every other hyperedge is kept unchanged. `HRefClosed` is `RefClosed` plus "every statement filler is live"; a successful delete keeps `HWF` and `HRefClosed` and removes the target. The guard is necessary: when it fires, keeping the hyperedges would leave a dangling filler. A delete fails exactly when the target is a statement and the object guard or (without `DETACH`) the hyperedge guard fires; `DETACH` never fails.

OpenSpec: `hyperedges` / Hyperedge roles block delete, scenarios "Hyperedge role blocks delete" and "Detach removes the hyperedge".

- proves:: [[08-hyperedges#^he-delete-filler]] {theorem: "MetaCore.Hyperedge.cascadeDelete_correct, MetaCore.Hyperedge.cascadeDelete_none_iff, MetaCore.Hyperedge.cascadeDelete_detach_some, MetaCore.Hyperedge.cascadeDelete_hedge_blocked, MetaCore.Hyperedge.cascadeDelete_removed, MetaCore.Hyperedge.cascadeDelete_hedges_sub, MetaCore.Hyperedge.cascadeDelete_plain_keeps_hedges", status: partial}
- proves:: [[01-data-model#^dm-delete-cascade]] {theorem: "MetaCore.Hyperedge.cascadeDelete_correct, MetaCore.Hyperedge.cascadeDelete_hedge", status: partial}

Partial: node deletion, annotations of hyperedges and promoted role edges (`hedge_promo`) are not modelled, so the cascade from a hyperedge to its annotations is tests only.

<assumption>

The Rust delete probes `incidence` for every member of D after the statement closure is complete; without `DETACH` any hit fails the delete with nothing written, with `DETACH` every hit's hyperedge is deleted (record and incidence keys) in the same transaction. The oracle trace checks blocked outcomes and removed sets that contain hyperedge IDs.

</assumption>

## Acyclic nesting

Adding `child :in parent` when `parent` is not already under `child` keeps nesting acyclic, and the guard is also necessary: a write that violates it really would close a cycle.

The oracle decides the guard with an executable reachability closure (`MetaCore.Graphs.reachB`, fuel `|edges| + 1`) that is proved equal to `Reach`. Over the step function `MetaCore.Graphs.apply`, nesting stays acyclic under every operation (nest by `addEdge_acyclic`, unnest because removing an edge cannot add a path), and nest is rejected exactly when `child = parent` or `parent` is already under `child`.

OpenSpec: `named-graphs` / Acyclic nesting, scenario "Cycle rejected".

- proves:: [[09-graphs#^gr-dag-cycle]] {theorem: "MetaCore.Nesting.addEdge_acyclic, MetaCore.Nesting.guard_necessary", status: proved}
- proves:: [[09-graphs#^gr-dag-cycle]] {theorem: "MetaCore.Graphs.apply_acyclic, MetaCore.Graphs.apply_nest, MetaCore.Graphs.reachB_iff", status: proved}
- proves:: [[01-data-model#^dm-nesting-dag]] {theorem: "MetaCore.Nesting.addEdge_acyclic", status: partial}

Partial: the depth limit of 64 is not modelled; the reachability check that implements the guard is tested against `Reach` by property test.

## Transitive scope

Direct members are in the transitive scope, and scope composes: whatever is in a child's scope is in every ancestor's scope.

The oracle's scope result (`MetaCore.Graphs.scope`) is exactly the live statements with a direct graph in the scope of g (transitive) or with g as a direct graph (direct mode), sorted, each ID once.

OpenSpec: `named-graphs` / Transitive graph scope by default, scenario "Parent scope includes children".

- proves:: [[09-graphs#^gr-scope-transitive]] {theorem: "MetaCore.Nesting.direct_sub_transitive, MetaCore.Nesting.scope_trans, MetaCore.Graphs.scope_mem, MetaCore.Graphs.scope_nodup", status: partial}
- proves:: [[09-graphs#^gr-scope-direct-mode]] {theorem: "MetaCore.Graphs.scope_mem", status: partial}

Partial: `scope_nodup` proves "Shared child counts once" for the model's scope set; the result multiset of the executor is still an E2E scenario ([[21-e2e-tests#Graph scope and nesting]]). Direct mode is modelled per query (`GRAPH!`), not as the connection flag.

## Graph membership

Every live statement stays a direct member of at least one graph, with no duplicates, after every operation; removing the last membership deletes the statement.

Inserting into g leaves the returned ID a direct member of g; an existing non-multi triple keeps its ID and gains the membership. Removing a membership that is not the last one keeps the statement; removing the last one is a plain (non-`DETACH`) cascade delete of the statement, which may be blocked and then changes nothing. Update keeps the ID and the memberships.

The model is `MetaCore.Graphs`: `World` (the extended store, a membership map, nesting edges), `Op` and the step function `apply`, which the oracle runs.

OpenSpec: `named-graphs` / One statement in several graphs, scenarios "Insert into a second graph adds a membership" and "Delete in one graph keeps the other".

- proves:: [[09-graphs#^gr-insert-existing]] {theorem: "MetaCore.Graphs.apply_insert_member, MetaCore.Store.insert_id_fresh_or_existing", status: partial}
- proves:: [[09-graphs#^gr-delete-in-graph]] {theorem: "MetaCore.Graphs.apply_memOK, MetaCore.Graphs.apply_removeMember", status: partial}

Partial: home graphs, chunks, `moved` tombstones, promoted memberships and moves are not modelled; graph 0 stands for the default graph.

<assumption>

The Rust store keeps, per live statement, the set of its direct graphs (home membership unless tombstoned, plus `gmem` extras), and deletes the statement through the plain cascade when the last one is removed. The oracle's final line lists every live statement's direct graphs, which the replay compares.

</assumption>

## Oracle step invariant

Every oracle operation keeps the full invariant (`HWF`, `HRefClosed`, memberships, acyclic nesting) from the empty world on, given that terms name live statements.

It is the state invariant behind the refinement replay ([[19-verification#Refinement bridge]]). The liveness precondition is `OpLive`, which the generator guarantees. This is the precondition every other theorem in this file needs, so it holds in every state the refinement replay visits.

- proves:: [[09-graphs#Membership operations]] {theorem: "MetaCore.Graphs.empty_inv, MetaCore.Graphs.apply_inv, MetaCore.Graphs.apply_hwf, MetaCore.Graphs.apply_hrefClosed, MetaCore.Hyperedge.insert_hrefClosed, MetaCore.Hyperedge.update_hrefClosed", status: proved}

## Durable prefix

A crash leaves the first `k` whole transactions with `durableBound ≤ k ≤ length`: every acknowledged `sync` or `ordered` commit survives, a `relaxed` commit followed by a durable one survives, and recovery never applies part of a transaction.

OpenSpec: `transactions` / Per-transaction durability levels and Atomic transactions, scenarios "Sync commit survives a crash", "Ordered commit is acknowledged after its group is durable", "Relaxed commit may be lost but never corrupts".

- proves:: [[06-transactions-durability#^tx-durability-prefix]] {theorem: "MetaCore.Durability.durable_survives, MetaCore.Durability.relaxed_covered", status: partial}
- proves:: [[06-transactions-durability#^tx-atomic]] {theorem: "MetaCore.Durability.recover, MetaCore.Durability.crash_outcome_exists", status: partial}

<assumption>

redb commits are totally ordered and atomic, and a durable commit makes every earlier commit durable. This is a property of the engine, checked by the crash sweep on every supported OS ([[06-transactions-durability#Crash recovery]]).

</assumption>

<assumption>

A binding never calls the core with a handle opened by another process: the Python binding records the opening PID and raises `ForkedHandleError` before any core call from a forked child, and never flushes or closes an inherited handle there ([[23-binding-decisions#Python detects fork instead of sharing handles]], [[adr:0042]]). The model has one owner process per file; a forked child writing through a copied handle would add a second, unmodelled writer. Checked by the fork tests of `crates/graphlava-py/tests/test_fork.py`.

</assumption>

<assumption>

No two handles to one file exist in one process: the Node binding keeps a process-global registry of canonical paths in the addon, so a second `Database.open` of an open file, from the main thread or any `worker_thread`, rejects with `LOCKED` before the core opens it, and handles cannot be posted to workers ([[23-binding-decisions#Node keeps one handle per file per process]], [[adr:0042]]). redb's OS lock does not separate threads of one process on every platform, and the model has one owner per file; two in-process handles would add a second, unmodelled writer. Checked by the worker and same-thread open tests of `bindings/node/test/single-owner.test.mjs`.

</assumption>

## Datalog safety and stratification

The stratifier accepts a program exactly when no dependency cycle passes through a negative edge, and then its stratum assignment is valid; the safety check equals its definition. This closes PO-STRAT together with [[20-proofs#Stratified model]].

The model is `MetaCore.Datalog`. The dependency graph has an edge from a rule's head predicate to every body predicate, split into `posEdges` and `negEdges`. `IsStrat P s` means positive edges go to a stratum `≤` and negative edges to a stratum `<`. `stratB` rejects a program when a negative edge `(h, b)` has `b = h` or `b` reaching `h` (decided with `MetaCore.Graphs.reachB`). For an accepted program `stratum p` counts the predicates `p` reaches by a path through a negative edge, and `stratum_valid` proves it a valid stratification. `stratB_complete` proves that any valid stratification forces acceptance, so `stratB_iff`: accepted ⟺ some valid stratification exists ⟺ no cycle through negation. `safeB_iff` proves the executable safety check equal to `Safe`: every head variable and every variable of a negated atom occurs in a positive body atom.

OpenSpec: `datalog-query` / Stratified negation and aggregation, scenario "Unstratifiable program".

- proves:: [[13-datalog#^dl-stratified]] {theorem: "MetaCore.Datalog.stratB_iff, MetaCore.Datalog.stratum_valid, MetaCore.Datalog.stratB_complete, MetaCore.Datalog.negCycle_rejected, MetaCore.Datalog.stratB_iff_not_negCycle, MetaCore.Datalog.IsStrat.neg_lower, MetaCore.Datalog.IsStrat.pos_le", status: partial}
- proves:: [[13-datalog#^dl-safety]] {theorem: "MetaCore.Datalog.safeB_iff, MetaCore.Datalog.programSafeB_iff", status: partial}

Partial: aggregation is not modelled (for stratification it is one more negative-like edge, which `negEdges` would absorb without changing a proof); built-ins in `dl-safety` are not modelled; the error naming the cycle or the unsafe variable is tests only. The model checks a cycle with a reachability closure per negative edge rather than SCCs; the Rust stratifier must reject the same programs.

<assumption>

The Rust stratifier builds the same dependency graph (an edge per body atom, aggregation edges counted as negative) and rejects a program exactly when a strongly connected component contains a negative or aggregation edge; this is the same condition as `NegCycle`. A property test compares its accept/reject decision with `stratB` on random small programs.

</assumption>

## Semi-naive evaluation

Within one stratum, semi-naive evaluation computes exactly the naive least fixpoint, and for safe rules that is the declarative least model; both loops terminate within a bound fixed by the finite Herbrand base. This discharges PO-SEMINAIVE.

The rules of a stratum are grounded over the active domain (head constants and constants of the base facts); negated atoms are decided against `low`, the complete lower strata, so the stratum is monotone. On the ground program `G` with heads `HB`: naive iteration from the base facts `I0` stops within `|HB| + 1` rounds at a set closed under the rules (`naive_closed`) that equals the inductive least fixpoint `Derivable` (`mem_naive_iff`, `naive_least`). Semi-naive evaluation runs one naive round, then fires only ground rules whose body holds and has an atom in the previous delta, stopping when the delta is empty within `|HB| + 2` rounds; its invariant is that a satisfied ground rule either has a delta atom or has fired. `semiNaive_eq_naive` proves the two results have the same facts. For a safe stratum, `mem_evalNaive_iff` and `mem_evalSemiNaive_iff` prove the result is exactly `PDerivable`, the least model over unrestricted valuations, so grounding over the active domain loses nothing.

OpenSpec: `datalog-query` / On-demand evaluation.

- proves:: [[13-datalog#^dl-semi-naive]] {theorem: "MetaCore.Datalog.semiNaive_eq_naive, MetaCore.Datalog.evalSemiNaive_eq_evalNaive, MetaCore.Datalog.mem_evalSemiNaive_iff, MetaCore.Datalog.mem_evalNaive_iff, MetaCore.Datalog.mem_naive_iff, MetaCore.Datalog.naive_closed, MetaCore.Datalog.semiNaive_closed", status: partial}

Partial: the delta condition is modelled as a filter on ground rules ("some body atom is in the delta, all are in the current set"); the Rust plan that rewrites each rule into one variant per body position, with sorted relations and merge joins, is not modelled. Magic sets, budgets, built-ins and aggregation are not modelled. The equality is per stratum, given `low` complete; composing strata is covered by [[20-proofs#Stratified model]].

<assumption>

Negated atoms of a stratum read only predicates of strictly lower strata, all of which are fully evaluated before the stratum starts (guaranteed by `IsStrat.neg_lower` when the stratifier accepts). The Rust semi-naive loop's union of per-position delta variants derives the same facts per round as `deltaFire`; a property test compares the engine's fixpoint with `MetaCore.Datalog.evalProgram` on random small programs.

</assumption>

## Stratified model

A safe stratified program has exactly one stable model, and bottom-up evaluation along any valid stratification computes it, so the answer does not depend on the stratification chosen. With [[20-proofs#Datalog safety and stratification]] this closes PO-STRAT.

`GL P I0 M` is the least model of the reduct of `P` with respect to `M`: negated atoms are decided against `M`, over unrestricted valuations. `Stable P I0 M` means `M` equals `GL P I0 M`. `stable_unique` proves that two stable models of a program with any valid stratification agree, by induction on the stratum: a derivation of a fact in stratum `k` reads negation only below `k` (`GL.agree`). `stage P s edb` evaluates the rules of stratum `k` semi-naively on the previous stage, deciding negation against it; a stage adds only facts of its own stratum (`stage_new`), so lower strata are final once computed (`stage_low`). `evalAlong_stable` proves that the last stage is a stable model for every valid `s`; hence `evalAlong_indep`: any two valid stratifications give the same facts. `evalProgram_correct` proves that the executable evaluator, which uses the canonical `stratum P`, returns that unique stable model for every safe program the stratifier accepts. `evalProgram_none_iff` proves it returns `none` exactly for unsafe or unstratifiable programs.

OpenSpec: `datalog-query` / Stratified negation and aggregation.

- proves:: [[13-datalog#^dl-stratified]] {theorem: "MetaCore.Datalog.stable_unique, MetaCore.Datalog.GL.agree, MetaCore.Datalog.evalAlong_stable, MetaCore.Datalog.evalAlong_indep, MetaCore.Datalog.evalProgram_correct, MetaCore.Datalog.evalProgram_none_iff, MetaCore.Datalog.stage_new, MetaCore.Datalog.stage_low", status: partial}

The model is characterised as the unique stable (Gelfond–Lifschitz) model; that this coincides with Przymusinski's perfect model for stratified programs is the classical result (Gelfond and Lifschitz 1988), cited, not formalised. Partial for the rule for the same reasons as above: aggregation, built-ins and error messages are not modelled.

<assumption>

The Rust engine evaluates strata in an order compatible with a valid stratification (each stratum after every stratum it depends on) and decides a negated atom only after the stratum of its predicate is complete. Any such order gives the same result by `evalAlong_indep`, so the Rust SCC order need not equal `stratum`. The property test against `MetaCore.Datalog.evalProgram` covers programs with negation.

</assumption>

## Suspend and the extension journal

Suspend and the iOS extension journal add no theorem: both reuse [[20-proofs#Durable prefix]].

Suspend is a durable commit boundary (a flush) followed by ending the snapshots, so every commit acknowledged before it survives a kill while suspended; a journal frame is one ordinary write request, so its exactly-once application is atomicity of one transaction.

OpenSpec: `mobile-bindings` / iOS suspend and resume, Android background and kill safety, iOS extension writes (`Formal: none`; L3/L4 tests: `crates/graphlava/tests/suspend.rs`, `crates/graphlava-ffi/tests/mobile.rs`).

- cites:: [[20-proofs#Durable prefix]] {intent: usesConclusionsFrom}
- cites:: [[16-api#^api-ios-suspend]] {intent: usesConclusionsFrom}

<assumption>

`graphlava_suspend` leaves no partially applied transaction: it aborts the open interactive write (whose writes live in its own write set, never in the batch), commits the open batch with a flush, and releases the engine handle only after every snapshot has ended. Checked by `crates/graphlava/tests/suspend.rs` (one test per step) and the FFI fuzz target `ffi_calls`.

</assumption>

<assumption>

A journal frame's write request and its `journal_seq:<extension-id>` metadata update commit in one transaction (`Database::submit_once`), and the journal is truncated only after a durable flush. Checked by the crash test `crash_while_applying_the_journal` (kill after the second of three frames) and the torn-journal fuzz target `journal_frames`.

</assumption>

## Open obligations

Stated but not yet proved; each names its planned module. Closing one moves it to a section above.

- **PO-CANON** (`MetaCore.Canon`): canonicalisation is idempotent and equal values of one datatype get one canonical form.
- **PO-PROMOTE** (`MetaCore.Store`): promoting an inline property changes no query result, only its ID becomes visible ([[01-data-model#^dm-props-virtual]]).
- **PO-MAGIC** (`MetaCore.Datalog`): magic-set rewriting preserves the answers to the goal.
- **PO-DROP** (`MetaCore.Nesting`): plain drop leaves descendants as roots; recursive drop keeps a descendant that has a surviving parent.

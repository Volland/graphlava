# Design

## Context

See proposal.md (Why). The full design lives in the lat.md graph under `tg.md/` (specs `00` to `18`) and the decisions in `docs/adr/` (0002–0018). This file summarises the decisions that shape v0 and points to them; it does not restate the specs.

## Goals / Non-Goals

**Goals:** an embeddable Rust core that implements the seven capabilities of this change on redb, with SPARQL 1.2 and Datalog as the two query surfaces, and a frozen file format at the end.

**Non-goals (v0):** Cypher, materialized user rules, the vector index, the server crate, remote `SERVICE`, mobile bindings, v2 block encoding, automatic partitioned graphs, hyperedge star-pattern collapse. See [[18-roadmap#v1]] and [[18-roadmap#v2]].

## Decisions

Each decision is an ADR; the specs link to it with `follows_from`.

| Decision | Chosen | Rejected | ADR |
| --- | --- | --- | --- |
| Engine and process model | redb, one owner process per file | libmdbx multi-process, Sanakirja (deferred) | [[adr:0002]] |
| Graph scoping | range-driven for small graphs, partitioned for large | `g` key column; filter-only | [[adr:0003]] |
| Statement uniqueness | unique `(s,p,o)`, `multi` per predicate | ID in every key | [[adr:0004]] |
| Literal routing | inline properties by object kind, auto value indexes, no demotion | everything in orderings; per-property choice | [[adr:0005]] |
| Delete semantics | cascade to annotations; hyperedge roles block | dangling; block always | [[adr:0006]] |
| `GRAPH` default | transitive; `GRAPH!` direct | direct default | [[adr:0007]] |
| Nesting shape | DAG, cycle rejection, depth 64 | tree; cyclic | [[adr:0008]] |
| Datalog maintenance | on-demand default; budgeted materialization (v1) | always synchronous | [[adr:0009]] |
| ID allocation | chunks per `(graph, kind)` | separate hyperedge space; incidence scans | [[adr:0010]] |
| ID layout | 4-bit kind, 40+20 chunk split, order-preserving inline literals | flat dictionary; 128-bit IDs | [[adr:0011]] |
| Durability | guarantee-based, `ordered` as group commit | drop `ordered`; own barrier (deferred) | [[adr:0012]] |
| Build order | core + Datalog IR + SPARQL first | Cypher first; everything in v0 | [[adr:0013]] |
| SPARQL scope | 1.2 query + update, `SERVICE <meta:datalog>` | custom `CALL` | [[adr:0014]] |
| Orderings | SPO, POS, OSP always; others on demand | all six (bake-off baseline) | [[adr:0015]] |
| Dictionary GC | background sweep | reference counts | [[adr:0016]] |
| Membership IDs | lazy, on annotation | always | [[adr:0017]] |
| File format | one portable format with feature flags | per-platform formats | [[adr:0018]] |
| Verification | Lean 4 model + refinement replay + tests | TLA+ (deferred), Verus/Kani (deferred), tests only | [[adr:0019]] |
| Benchmark gating | instruction counts per PR, statistical wall-clock nightly | wall-clock on PR runners; manual runs | [[adr:0020]] |

### Constraints

External rules the design must respect, written as `Constraint` sections in [[07-platforms]]:

- iOS kills apps that hold file locks in shared containers when suspended (`0xdead10cc`): binds the owner-process lock handling ([[16-api#Mobile bindings]]).
- Windows byte-range locks are mandatory: binds the lock placement in [[05-storage-trait#redb adapter]].
- Android devices with 16 KB pages: binds native library alignment.
- The W3C SPARQL 1.1 and 1.2 specifications: bind [[11-sparql#Conformance]].

## Formal model and test strategy

The invariants of term-identity, statement-store, named-graphs and transactions are stated and proved in Lean 4 in `formal/MetaCore/` and listed with their assumptions in [[20-proofs]]. The abstraction function maps the redb tables to `MetaCore.Store.Store`: every `stmt` row and every inline property becomes one `(id, triple)` entry, with unpromoted properties given no ID, and `meta.next` becomes `next`. The replay test checks `α (rust_op s) = model_op (α s)` per operation on oracle traces.

| Capability | L0 Lean | L1 replay | L3/L4 E2E and crash |
| --- | --- | --- | --- |
| term-identity | `Id.*`, `Store.insert_next_mono`, `cascadeDelete_next` | yes | portable file, trace replay |
| statement-store | `Store.insert_wf`, `update_*`, `cascadeDelete_correct` | yes | agent session, cascade and hyperedge guard |
| named-graphs | `Nesting.addEdge_acyclic`, `scope_trans` | v0.1 | graph scope and nesting |
| transactions | `Durability.durable_survives`, `relaxed_covered` | n/a | crash at every commit point, second process |
| hyperedges | open (11.6) | v0.1 | cascade and hyperedge guard |
| sparql-query | none: covered by W3C conformance (L5) | n/a | same answers on every surface |
| datalog-query | open (PO-STRAT, PO-SEMINAIVE) | n/a | same answers on every surface |

## Risks / Trade-offs

- [redb page size and overflow behaviour may raise write amplification] → measured in the bake-off; own pager stays possible behind the `Storage` trait ([[open-questions#Page size on redb]]).
- [Three orderings make some cyclic queries slower] → on-demand orderings, measured in [[17-benchmarks#Ordering count comparison]].
- [Transitive `GRAPH` departs from SPARQL] → direct compatibility mode for conformance tests.
- [Inline literal layout is frozen at v0] → prefix-coded layout with exact inline doubles decided before the freeze ([[adr:0021]]); its order theorems are in `MetaCore.Id`.
- [Small write transactions copy B-tree paths in every touched tree] → one engine transaction per group-commit batch ([[adr:0025]]); bytes per request measured in the spike.
- [redb crash repair on large files lengthens app start after an OS kill] → measured in the first spike week ([[open-questions#Crash repair time on large files]]).
- [SPARQL 1.2 drafts change] → pinned draft ([[open-questions#SPARQL 1.2 draft pin]]).
- [Format freeze locks early mistakes] → freeze only after the v0 benchmarks run and the ID-encoding proof obligations close ([[18-roadmap#Format freeze]], [[20-proofs#Open obligations]]).
- [Lean model drifts from the Rust code] → refinement replay on every PR; failing seeds kept as regressions ([[adr:0019]]).

## Migration Plan

No existing data or users. The format freeze at the end of v0 is the first point after which migrations are required ([[adr:0018]]).

## Open Questions

The questions with gate v0 in [[open-questions]] change specs or tasks and must be decided before the matching tasks start; task group 1 tracks that. Questions with gate v1 or v2 do not affect this change.

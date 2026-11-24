# Proposal

## Why

meta-core needs a first usable release: an embeddable, mutable metagraph store for agent memory with statements about statements, hyperedges and nested named graphs, queryable through SPARQL 1.2 and Datalog. The design was settled in a grilling session (ADRs 0002–0018); v0 turns it into a working core before any other surface is built ([[adr:0013]]).

## What Changes

- New embeddable Rust database with a single owner process per file, on redb ([[adr:0002]]).
- 64-bit term IDs with order-preserving inline literals and a term dictionary with background sweep ([[adr:0011]], [[adr:0016]]).
- Statements with stable IDs, updated in place; unique `(s,p,o)` with opt-in multi predicates; literal-valued properties stored inline per node with promotion on annotation; cascade delete ([[adr:0004]], [[adr:0005]], [[adr:0006]]).
- Three always-on orderings (SPO, POS, OSP) and on-demand others ([[adr:0015]]).
- Named and nested graphs: chunked statement IDs per `(graph, kind)`, transitive scope by default, DAG nesting, lazy membership IDs ([[adr:0003]], [[adr:0007]], [[adr:0008]], [[adr:0010]], [[adr:0017]]).
- Hyperedges as records with an incidence index and virtual role triples.
- Transactions with `sync`, `ordered` (group commit) and `relaxed` durability ([[adr:0012]]).
- SPARQL 1.2 query and update, with Datalog callable through `SERVICE <meta:datalog>` ([[adr:0014]]).
- Datalog as a standalone query surface with on-demand rules ([[adr:0009]]).
- One portable file format, frozen at the end of v0 ([[adr:0018]]).

Gate: the engine bake-off spike ([[17-benchmarks#Engine bake-off]]) runs before implementation starts.

## Capabilities

### New Capabilities

- `term-identity`: 64-bit IDs, inline literals, canonical forms, the term dictionary and its sweep.
- `statement-store`: statements with stable IDs, inline properties, promotion, annotations, orderings and cascade delete.
- `transactions`: the single-owner database, transactions, durability levels and crash behaviour.
- `named-graphs`: graph membership, transitive and direct scope, DAG nesting and graph drop.
- `hyperedges`: typed n-ary hyperedges, their roles and their graph membership.
- `sparql-query`: SPARQL 1.2 query and update, reifiers, graph semantics and the Datalog service.
- `datalog-query`: the standalone Datalog surface, stratification and on-demand evaluation.

### Modified Capabilities

None; this is the first change.

## Impact

- **Code:** new Rust workspace (core crate, C FFI crate); `spikes/bakeoff/` for the bake-off harness.
- **Dependencies:** redb; a SPARQL parser ([[open-questions#SPARQL parser reuse]]); roaring bitmaps.
- **lat.md sections touched:** [[01-data-model]], [[02-id-encoding]], [[03-dictionary]], [[04-physical-layout]], [[05-storage-trait]], [[06-transactions-durability]], [[08-hyperedges]], [[09-graphs]], [[10-ir]], [[11-sparql]], [[13-datalog]], [[14-planner-execution]], [[16-api]].
- **Binding decisions:** [[adr:0002]] to [[adr:0018]]; platform constraints in [[07-platforms]].
- **Open questions gating v0:** every question with gate v0 in [[open-questions]].
- **Requirements added:** all requirements of the seven capabilities above, cited as `[[openspec:<capability>#<Requirement>]]`.

# Proposal

## Why

The store is only useful to agents through queries. This third slice of the former `v0-core` change adds the extended-Datalog IR, the planner and executor, and the two v0 surfaces, SPARQL 1.2 and Datalog ([[adr:0013]], [[adr:0014]]).

## What Changes

- IR operators, modes and virtual relations; statistics, join selection (Leapfrog Triejoin, index nested-loop, BFS path kernel), vectorised batches and the plan cache with drift invalidation ([[adr:0027]], [[adr:0041]]); the per-node predicate summary for unbound predicates ([[adr:0039]]).
- Datalog: Nemo-compatible parser with `?-` goals and `@assert` ([[adr:0040]]), stratification, magic sets and semi-naive evaluation, metagraph relations.
- SPARQL 1.2 on `spargebra` ([[adr:0038]]): query forms, update with a default write graph, reifiers as statement IDs with IRI form and aliases ([[adr:0031]], [[adr:0032]]), in-place update, `GRAPH` / `GRAPH!`, `SERVICE <meta:datalog>`, W3C suites.
- The Rust API (`sparql()`, `datalog()`) and the C FFI.

## Capabilities

### New Capabilities

- `sparql-query`: SPARQL 1.2 query and update, reifiers, graph semantics and the Datalog service.
- `datalog-query`: the standalone Datalog surface, stratification and on-demand evaluation.

### Modified Capabilities

None.

## Impact

- **Code:** `crates/graphlava` ir, planner, exec, datalog and sparql modules; `crates/graphlava-ffi`.
- **Dependencies:** `spargebra`, roaring bitmaps.
- **lat.md sections:** [[10-ir]], [[11-sparql]], [[13-datalog]], [[14-planner-execution]], [[16-api]].
- **Verification:** sparql-query L5 (W3C suites) and L3; datalog-query L0 (PO-STRAT, PO-SEMINAIVE, task 11.7), L3, L5.

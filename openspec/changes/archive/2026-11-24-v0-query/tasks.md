# Tasks

Numbers in parentheses are the task ids of the former `v0-core` change.

## 1. IR, planner and executor

- [x] 1.1 Implement the IR operators, modes and virtual relations, with the per-node predicate summary for unbound predicates ([[10-ir#Operators]], [[adr:0039]]) (8.1)
- [x] 1.2 Implement statistics, join selection, Leapfrog Triejoin, vectorized batches and the plan cache with 4x drift invalidation and the missing-ordering fallback ([[14-planner-execution#Join selection]], [[adr:0041]]) (8.2)
- [x] 1.3 Implement the index nested-loop join and the BFS path kernel ([[adr:0027]]) (15.7)

## 2. datalog-query

- [x] 2.1 Model Datalog stratification and prove PO-STRAT and PO-SEMINAIVE (`MetaCore.Datalog`) (11.7)
- [x] 2.2 Implement the Datalog parser (Nemo syntax, `?- goal.`, `@assert`) and the standalone entry point; annotate `[[openspec:datalog-query#Standalone Datalog programs]]` and `[[openspec:datalog-query#Nemo-compatible syntax]]` (8.3)
- [x] 2.3 Implement stratification; annotate `[[openspec:datalog-query#Stratified negation and aggregation]]` (8.4)
- [x] 2.4 Implement magic sets and semi-naive evaluation; annotate `[[openspec:datalog-query#On-demand evaluation]]` (8.5)
- [x] 2.5 Expose metagraph relations to Datalog; annotate `[[openspec:datalog-query#Metagraph access from Datalog]]` (8.6)
- [x] 2.6 Write a test per scenario in `specs/datalog-query/spec.md`, each annotated with its `@tg: verifies::` id (8.7)

## 3. sparql-query

- [x] 3.1 Implement query forms on `spargebra`; annotate `[[openspec:sparql-query#SPARQL 1.2 query forms]]` (9.1)
- [x] 3.2 Implement update with the default write graph; annotate `[[openspec:sparql-query#SPARQL update]]` (9.2)
- [x] 3.3 Implement reifier binding, reifier IRIs, aliases and unasserted triples; annotate `[[openspec:sparql-query#Reifiers are statement IDs]]` (9.3)
- [x] 3.4 Implement same-reifier in-place update; annotate `[[openspec:sparql-query#In-place update through the same reifier]]` (9.4)
- [x] 3.5 Implement `GRAPH`, `GRAPH!` and direct mode; annotate `[[openspec:sparql-query#Graph scope semantics]]` (9.5)
- [x] 3.6 Implement `SERVICE <meta:datalog>`; annotate `[[openspec:sparql-query#Datalog through SERVICE]]` (9.6)
- [x] 3.7 Pin the SPARQL 1.2 TR snapshot in [[11-sparql#Conformance]] and wire the W3C test suites into CI; annotate `[[openspec:sparql-query#Conformance]]` (9.7)
- [x] 3.8 Write a test per scenario in `specs/sparql-query/spec.md`, each annotated with its `@tg: verifies::` id (9.8)

## 4. API

- [x] 4.1 Implement the Rust API with `sparql()` and `datalog()` entry points and the C FFI ([[16-api#Rust API]], [[16-api#C FFI]]) (10.1)

## 5. Documentation and checks

- [x] 5.1 Add `openspec:` frontmatter: sparql-query → `11-sparql`; datalog-query → `13-datalog` (14.2)
- [x] 5.2 Run the full check list and `tg trace sparql-query datalog-query --gaps`, and close every gap (14.3)

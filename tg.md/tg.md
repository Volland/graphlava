# Project

This directory defines the high-level concepts, business logic and architecture of the project in markdown, managed with the `tg` CLI (compatible with lat.md). Run `tg check` to validate it.

- [[code-ontology]] — shared vocabulary of intent types and typed edges between intent and code
- [[pre-design]] — original design handover for meta-core (input to the specs, not maintained)
- [[00-overview]] — vision, goals, architecture and map of the specs
- [[01-data-model]] — terms, statements, inline properties, promotion, annotations, metagraph kinds
- [[02-id-encoding]] — 64-bit ID layout, inline literals, canonical forms, allocation
- [[03-dictionary]] — term dictionary layout, interning and background sweep
- [[04-physical-layout]] — redb tables, key byte order, v1 and v2 encodings, size estimates
- [[05-storage-trait]] — Storage trait API and the redb adapter
- [[06-transactions-durability]] — transaction model, durability levels, group commit, recovery, cascade delete
- [[07-platforms]] — platform constraints for iOS, Android, Windows, macOS and Linux
- [[08-hyperedges]] — hyperedge records, incidence index, role triples, star-pattern collapse
- [[09-graphs]] — named and nested graphs, chunks, scope expansion, partitioned graphs, drop
- [[10-ir]] — shared extended-Datalog IR: operators, modes, virtual relations
- [[11-sparql]] — SPARQL 1.2 scope, reifiers, graph semantics, Datalog service, conformance
- [[12-cypher]] — Cypher/GQL subset for v1 and its mapping
- [[13-datalog]] — Datalog surface, standalone queries, stratification, evaluation and maintenance
- [[14-planner-execution]] — statistics, join selection, Leapfrog Triejoin, execution, plan cache
- [[15-vector-index]] — scope-aware vector index (exact and IVF/RaBitQ) for v1
- [[16-api]] — Rust API, C FFI, mobile bindings, server crate
- [[17-benchmarks]] — measurements that gate performance claims
- [[18-roadmap]] — milestones from the bake-off spike to v2
- [[19-verification]] — assurance levels L0–L6, Lean model, refinement bridge, CI gates
- [[20-proofs]] — ledger of Lean 4 theorems and open proof obligations, traced to requirements
- [[21-e2e-tests]] — catalogue of end-to-end scenarios on real files, processes and the FFI
- [[open-questions]] — undecided design questions with options and recommendations
- [[resolved-questions]] — questions answered by ADRs

# Tasks

## 1. Gate: bake-off and v0 decisions

- [ ] 1.1 Build the bake-off harness in `spikes/bakeoff/` (redb, libmdbx, Sanakirja behind a minimal `Storage` trait) and run [[17-benchmarks#Engine bake-off]] and [[17-benchmarks#Ordering count comparison]] at 10M statements
- [ ] 1.2 Confirm or reopen [[adr:0002]] and [[adr:0015]] from the bake-off results
- [ ] 1.3 Decide every gate-v0 question in `tg.md/open-questions.md`; record each as an ADR and add a `resolved_by` edge
- [ ] 1.4 Update specs and these tasks for any decision from 1.3 that changes them

## 2. Workspace and storage

- [x] 2.1 Create the Rust workspace (core crate, FFI crate) with CI on Linux, macOS and Windows, and iOS/Android build jobs
- [ ] 2.2 Implement the `Storage` trait and redb adapter ([[05-storage-trait#Trait API]], [[05-storage-trait#redb adapter]])
- [ ] 2.3 Implement the v1 table layout and key byte order ([[04-physical-layout#Tables]], [[04-physical-layout#Key byte order]])

## 3. term-identity

- [ ] 3.1 Implement ID encoding and annotate with `@tg: implements:: [[openspec:term-identity#Stable term identity]]`
- [ ] 3.2 Implement inline literal encodings and annotate with `@tg: implements:: [[openspec:term-identity#Order-preserving inline literals]]`
- [ ] 3.3 Implement canonicalisation and annotate with `@tg: implements:: [[openspec:term-identity#Canonical literal forms]]`
- [ ] 3.4 Implement the dictionary and background sweep and annotate with `@tg: implements:: [[openspec:term-identity#Dictionary reclamation without write cost]]`
- [ ] 3.5 Write a test per scenario in `specs/term-identity/spec.md`, each annotated `@tg: verifies:: [[openspec:term-identity#<Requirement>#<Scenario>]]`

## 4. statement-store

- [ ] 4.1 Implement statements, the `stmt` record and in-place update; annotate `[[openspec:statement-store#Statement identity under update]]`
- [ ] 4.2 Implement unique keys and `multi` predicates; annotate `[[openspec:statement-store#Unique statements by default]]`
- [ ] 4.3 Implement inline property records and value lookups; annotate `[[openspec:statement-store#Inline literal properties]]`
- [ ] 4.4 Implement promotion; annotate `[[openspec:statement-store#Promotion on annotation]]`
- [ ] 4.5 Implement cascade delete with the hyperedge guard; annotate `[[openspec:statement-store#Cascade delete of annotations]]`
- [ ] 4.6 Implement SPO, POS, OSP and on-demand orderings; annotate `[[openspec:statement-store#Index coverage of base orderings]]`
- [ ] 4.7 Write a test per scenario in `specs/statement-store/spec.md`, each annotated with its `@tg: verifies::` id

## 5. transactions

- [ ] 5.1 Implement the owner-process lock; annotate `[[openspec:transactions#Single owner process]]`
- [ ] 5.2 Implement read snapshots and the single writer; annotate `[[openspec:transactions#Snapshot reads and one writer]]`
- [ ] 5.3 Implement `sync`, `ordered` (group commit) and `relaxed`; annotate `[[openspec:transactions#Per-transaction durability levels]]`
- [ ] 5.4 Make allocation, cascades and index updates atomic; annotate `[[openspec:transactions#Atomic transactions]]`
- [ ] 5.5 Implement the format version and feature flags; annotate `[[openspec:transactions#Portable file format]]`
- [ ] 5.6 Write crash and cross-platform tests per scenario in `specs/transactions/spec.md`, each annotated with its `@tg: verifies::` id

## 6. named-graphs

- [ ] 6.1 Implement chunk membership and `gmem`; annotate `[[openspec:named-graphs#Home graph membership]]`
- [ ] 6.2 Implement scope expansion (transitive and direct) with `in_star`; annotate `[[openspec:named-graphs#Transitive graph scope by default]]`
- [ ] 6.3 Implement cycle and depth checks; annotate `[[openspec:named-graphs#Acyclic nesting]]`
- [ ] 6.4 Implement lazy membership IDs; annotate `[[openspec:named-graphs#Annotatable membership]]`
- [ ] 6.5 Implement plain and recursive drop; annotate `[[openspec:named-graphs#Graph drop]]`
- [ ] 6.6 Write a test per scenario in `specs/named-graphs/spec.md`, each annotated with its `@tg: verifies::` id

## 7. hyperedges

- [ ] 7.1 Implement records and type validation; annotate `[[openspec:hyperedges#Typed hyperedges with roles]]`
- [ ] 7.2 Implement the incidence index and virtual role triples; annotate `[[openspec:hyperedges#Role triples are queryable]]`
- [ ] 7.3 Implement per-(graph, type) chunks and type scans; annotate `[[openspec:hyperedges#Hyperedges in graphs and type scans]]`
- [ ] 7.4 Implement role promotion; annotate `[[openspec:hyperedges#Annotating a role]]`
- [ ] 7.5 Write a test per scenario in `specs/hyperedges/spec.md`, each annotated with its `@tg: verifies::` id

## 8. IR, planner and datalog-query

- [ ] 8.1 Implement the IR operators, modes and virtual relations ([[10-ir#Operators]])
- [ ] 8.2 Implement statistics, join selection, Leapfrog Triejoin, vectorized batches and the plan cache ([[14-planner-execution#Join selection]])
- [ ] 8.3 Implement the Datalog parser and standalone entry point; annotate `[[openspec:datalog-query#Standalone Datalog programs]]` and `[[openspec:datalog-query#Nemo-compatible syntax]]`
- [ ] 8.4 Implement stratification; annotate `[[openspec:datalog-query#Stratified negation and aggregation]]`
- [ ] 8.5 Implement magic sets and semi-naive evaluation; annotate `[[openspec:datalog-query#On-demand evaluation]]`
- [ ] 8.6 Expose metagraph relations to Datalog; annotate `[[openspec:datalog-query#Metagraph access from Datalog]]`
- [ ] 8.7 Write a test per scenario in `specs/datalog-query/spec.md`, each annotated with its `@tg: verifies::` id

## 9. sparql-query

- [ ] 9.1 Implement query forms; annotate `[[openspec:sparql-query#SPARQL 1.2 query forms]]`
- [ ] 9.2 Implement update; annotate `[[openspec:sparql-query#SPARQL update]]`
- [ ] 9.3 Implement reifier binding; annotate `[[openspec:sparql-query#Reifiers are statement IDs]]`
- [ ] 9.4 Implement same-reifier in-place update; annotate `[[openspec:sparql-query#In-place update through the same reifier]]`
- [ ] 9.5 Implement `GRAPH`, `GRAPH!` and direct mode; annotate `[[openspec:sparql-query#Graph scope semantics]]`
- [ ] 9.6 Implement `SERVICE <meta:datalog>`; annotate `[[openspec:sparql-query#Datalog through SERVICE]]`
- [ ] 9.7 Wire the W3C test suites into CI; annotate `[[openspec:sparql-query#Conformance]]`
- [ ] 9.8 Write a test per scenario in `specs/sparql-query/spec.md`, each annotated with its `@tg: verifies::` id

## 10. API, benchmarks and freeze

- [ ] 10.1 Implement the Rust API with `sparql()` and `datalog()` entry points and the C FFI ([[16-api#Rust API]], [[16-api#C FFI]])
- [ ] 10.2 Run [[17-benchmarks#Inline literal range scan]], [[17-benchmarks#Durability commit latency]] and [[17-benchmarks#Dictionary sweep cost]]
- [ ] 10.3 Freeze the file format ([[18-roadmap#Format freeze]]) only after 11.5 and 13.5 are done

## 11. Formal model (Lean 4) — runs alongside groups 3–7

- [x] 11.1 Create the `formal/` Lake project (core Lean 4.36, no Mathlib) with `MetaCore.Id`, `MetaCore.Store`, `MetaCore.Nesting`, `MetaCore.Durability`, zero `sorry` ([[19-verification#Formal model]])
- [x] 11.2 Add the axiom audit `formal/scripts/lean-audit.sh` and the proof ledger [[20-proofs]] with `proves::` edges
- [x] 11.3 Add the executable oracle `lake exe oracle <seed> <n>` emitting JSON-lines traces ([[19-verification#Refinement bridge]])
- [ ] 11.4 Write `crates/meta-core/tests/refinement.rs`: replay seeds 1..64 × 2,000 operations through the public API and compare results and the final state through the abstraction function
- [ ] 11.5 Close PO-CASCADE-FUEL, PO-ID-STRING and PO-ID-DOUBLE before the format freeze (10.3)
- [ ] 11.6 Model hyperedges (`MetaCore.Hyperedge`: arity, role validation, delete guard) and prove the guard keeps `RefClosed` for roles
- [ ] 11.7 Model Datalog stratification and prove PO-STRAT and PO-SEMINAIVE (`MetaCore.Datalog`)
- [ ] 11.8 Add a `Debug` assertion in the Rust store for each `<assumption>` in [[20-proofs]]

## 12. End-to-end and crash tests

- [ ] 12.1 Create `crates/meta-core/tests/e2e/` with a temp-dir fixture that opens, closes and reopens a real file through the public API only
- [ ] 12.2 Implement every scenario of [[21-e2e-tests]], each annotated `@tg: verifies:: [[21-e2e-tests#<Scenario>]] {level: e2e}`, then set `lat: require-code-mention: true` in that file
- [ ] 12.3 Build the crash harness: a child writer killed at every I/O step, checked against `MetaCore.Durability.recover` outcomes, on Linux, macOS and Windows
- [ ] 12.4 Add the cross-OS golden-file job and the C FFI lifecycle test under ASan and LSan
- [ ] 12.5 Add the nightly 30-minute soak against the model

## 13. Benchmark harness and regression gates

- [ ] 13.1 Create `bench/` (`meta-bench` binary): dataset generator, agent-memory mix, HDR histograms, open-loop driver, JSON results ([[17-benchmarks#Harness and method]])
- [ ] 13.2 Add criterion and `iai-callgrind` micro benches for the cases of [[17-benchmarks#Micro benchmarks]]; gate PRs on +3% instructions ([[adr:0020]])
- [ ] 13.3 Add adapters for SQLite (WAL), Oxigraph and tiramemsu behind the harness trait
- [ ] 13.4 Wire LUBM, WatDiv and BSBM ([[17-benchmarks#Standard SPARQL workloads]]) and the SNB-style traversal workload
- [ ] 13.5 Set up the fixed nightly runner, run the scaling sweep, reader scaling, open and recovery time, and space under churn, and store baselines in `bench/results/`
- [ ] 13.6 Turn each "to be measured" target into a number in a decision record once two nightly runs agree within their confidence intervals

## 14. Documentation and traceability

- [ ] 14.1 Record any new decision or constraint from implementation in lat.md as typed sections (`type: Decision`, `type: Constraint`) with edges from `tg ontology` only (`motivated_by`, `constrains`, `follows_from`)
- [ ] 14.2 Add `openspec: [<capability>]` frontmatter to the lat.md spec file that explains each capability (term-identity → `02-id-encoding`, `03-dictionary`; statement-store → `01-data-model`, `04-physical-layout`; transactions → `06-transactions-durability`; named-graphs → `09-graphs`; hyperedges → `08-hyperedges`; sparql-query → `11-sparql`; datalog-query → `13-datalog`)
- [ ] 14.3 Run `cargo test`, `lake build`, `formal/scripts/lean-audit.sh`, `tg check`, `tg ontology check` and `tg trace term-identity statement-store transactions named-graphs hyperedges sparql-query datalog-query --gaps` and close every gap

## 15. Architecture review follow-ups (ADR 0021–0030)

Format-affecting items 15.1–15.4 must be done before the freeze (10.3).

- [ ] 15.1 Implement the prefix-coded inline literal layout ([[adr:0021]], [[02-id-encoding#Inline literals]]) and the `id_layout_matches_model` and `double_magnitude_order` property tests; keep `MetaCore.Id` in step
- [ ] 15.2 Implement `plist`, the sort-key `propidx` and their invariant checks ([[adr:0022]], [[04-physical-layout#Tables]])
- [ ] 15.3 Implement the `dt` table, the namespace cap, owned literals and the mark-and-sweep with the kept set ([[adr:0023]], [[03-dictionary#Owned literals]], [[03-dictionary#Sweep]])
- [ ] 15.4 Implement membership operations, `moved` tombstones, drop that keeps shared items and per-store uniqueness ([[adr:0024]], [[09-graphs#Membership operations]])
- [ ] 15.5 Implement one engine transaction per batch with per-request overlays and check-then-apply ([[adr:0025]], [[06-transactions-durability#Group commit]]); measure bytes per request at batch sizes 1, 8 and 64
- [ ] 15.6 Implement the core API, one-shot write requests, compare-and-set and the in-process writer time limit ([[adr:0026]], [[16-api#Core API]], [[16-api#Write requests]])
- [ ] 15.7 Implement the index nested-loop join and the BFS path kernel ([[adr:0027]], [[14-planner-execution#Join selection]], [[14-planner-execution#Path evaluation]])
- [ ] 15.8 Implement `functional` predicates and hyperedge literal attributes ([[adr:0028]], [[01-data-model#Statements]], [[08-hyperedges#Literal attributes]])
- [ ] 15.9 Implement the set-valued `in_star` with ancestor recomputation on edge removal, and test a lattice of 40 stacked diamonds ([[adr:0029]], [[09-graphs#Nesting DAG]])
- [ ] 15.10 In the spike's first week, measure redb crash-repair time at 10M and 100M statements and confirm whether redb allows a savepoint after writes ([[18-roadmap#Spike: engine bake-off]])
- [ ] 15.11 (v1) Backward/Forward maintenance and the scope-aware vector index with RaBitQ, re-ranking and list splits ([[adr:0029]], [[adr:0030]], [[15-vector-index#Search modes]])
- [ ] 15.12 Write a test per new scenario in `specs/term-identity`, `specs/statement-store`, `specs/named-graphs`, `specs/transactions` and `specs/hyperedges`, each annotated with its `@tg: verifies::` id

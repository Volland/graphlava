---
type: Change
status: proposed
specs: ["adr:0013", "adr:0014", "adr:0018", "adr:0002", "adr:0009", "adr:0015", "adr:0003", "adr:0010"]
---
# Roadmap

Milestones from the engine bake-off spike to v2, in the build order of [[adr:0013]]: core plus Datalog and SPARQL 1.2 first, Cypher and vectors next, compression and partition automation last.

- cites:: [[adr:0013]] {intent: usesConclusionsFrom}

Each milestone is a `Change` node; requirements point at the milestone that brings them in with `introduced_by`. A milestone is done when its requirements are implemented and verified (`tg trace`) and its benchmarks have run.

## Spike: engine bake-off

Measurement-only work that gates v0: the bake-off harness behind a minimal `Storage` trait. No other code is written until the specs are reviewed.

- cites:: [[17-benchmarks#Engine bake-off]] {intent: usesMethodIn}

Deliverables: `spikes/bakeoff/` with redb, libmdbx and Sanakirja adapters, the dataset generator, the agent-memory mix, and a results report. Exit: [[17-benchmarks#Engine bake-off]] and [[17-benchmarks#Ordering count comparison]] have run at 10M statements, and the redb choice ([[adr:0002]]) is confirmed or reopened.

## v0

The embeddable core: storage, data model, graphs, hyperedges, the Datalog IR, and two standalone query surfaces, SPARQL 1.2 and Datalog. Rust API and C FFI.

- cites:: [[adr:0014]] {intent: usesConclusionsFrom}

| Area | Scope | Spec |
| --- | --- | --- |
| Storage | `Storage` trait, redb adapter, v1 encoding (plain tables, SPO/POS/OSP with `id` values), `stmt`, `props`, value indexes, on-demand orderings | [[05-storage-trait#Trait API]], [[04-physical-layout#Encoding v1]] |
| Terms | ID encoding, dictionary, canonical forms, background sweep | [[02-id-encoding#Bit layout]], [[03-dictionary#Sweep]] |
| Data model | statements, inline properties, promotion, annotations, cascade delete | [[01-data-model#Statements]], [[06-transactions-durability#Cascade delete]] |
| Graphs | chunk membership per `(graph, kind)`, `gmem`, transitive scope, DAG nesting with `in_star`, graph drop | [[09-graphs#Chunk membership]], [[09-graphs#Nesting DAG]] |
| Hyperedges | record, incidence index, role triples (virtual) | [[08-hyperedges#Record format]] |
| Transactions | `sync`, `ordered` (group commit), `relaxed` | [[06-transactions-durability#Durability levels]] |
| IR and planner | shared IR, statistics, Leapfrog Triejoin, merge/hash joins, vectorized batches, plan cache | [[10-ir#Operators]], [[14-planner-execution#Join selection]] |
| Datalog | **Datalog as a standalone query surface** (Nemo-compatible syntax), stratification, `@on_demand` rules (magic sets, semi-naive) | [[13-datalog#Surface syntax]], [[13-datalog#On-demand evaluation]] |
| SPARQL | SPARQL 1.2 query and update, reifiers, transitive `GRAPH`, local `SERVICE <meta:datalog>`, W3C test suites | [[11-sparql#Scope]], [[11-sparql#Datalog service]], [[11-sparql#Conformance]] |
| API | Rust API with `sparql()` and `datalog()` entry points, C FFI | [[16-api#Rust API]], [[16-api#C FFI]] |
| Platforms | builds and tests on Linux, macOS, Windows; iOS and Android build only | [[07-platforms#Windows]] |

Exit: all v0 requirements implemented and verified; W3C SPARQL 1.1 suite passing in `direct` mode with listed skips; [[17-benchmarks#Inline literal range scan]], [[17-benchmarks#Durability commit latency]] and [[17-benchmarks#Dictionary sweep cost]] run; [[18-roadmap#Format freeze]] done.

## v1

Second surfaces and built-ins: Cypher subset, materialized rules, vector index, server crate, remote SERVICE, mobile bindings.

- cites:: [[adr:0009]] {intent: usesConclusionsFrom}

| Area | Scope | Spec |
| --- | --- | --- |
| Cypher | Cypher/GQL subset over the same IR, multi predicates | [[12-cypher#Subset]] |
| Datalog | `@materialized` rules: counting, DRed with budget, `stale` fallback, derived table | [[13-datalog#Materialized maintenance]] |
| Vectors | IVF-PQ index and IR operator | [[15-vector-index#IVF-PQ layout]] |
| Server | `meta-core-server`: SPARQL Protocol, Datalog endpoint, local IPC socket | [[16-api#Server crate]] |
| Federation | remote `SERVICE` via HTTP resolver | [[11-sparql#Datalog service]] |
| Mobile | Swift and Kotlin bindings, suspend/resume, iOS extension IPC | [[16-api#Mobile bindings]] |

Exit: [[17-benchmarks#Mobile]], [[17-benchmarks#Datalog versus Nemo]], [[17-benchmarks#Vector search recall and latency]], [[17-benchmarks#Versus tiramemsu]] and [[17-benchmarks#Versus SQLite and Turso]] run.

## v2

Performance work that does not change the data model: block-compressed encoding with SIMD decode, automatic partitioned graphs, hyperedge star-pattern collapse.

- cites:: [[adr:0003]] {intent: usesConclusionsFrom}

| Area | Scope | Spec |
| --- | --- | --- |
| Encoding | v2 block values: ~512 sorted tuples per value, delta and bit packing, SIMD decode (AVX2/NEON) | [[04-physical-layout#Encoding v2]] |
| Graphs | automatic promotion of large graphs to partitioned tables, online | [[09-graphs#Partitioned graphs]] |
| Hyperedges | star-pattern collapse into a single hyperedge relation scan | [[08-hyperedges#Star-pattern collapse]] |
| Platforms | hardening from v1 mobile and Windows results | [[07-platforms#iOS]] |

The v2 encoding is a new feature flag in the file ([[adr:0018]]); v1-encoded files stay readable and are migrated on request.

## Format freeze

The point after which the on-disk format changes only with a version bump, a feature flag and a migration ([[adr:0018]]). Happens at the end of v0, before any user data is kept.

- cites:: [[adr:0018]] {intent: usesConclusionsFrom}

Frozen at v0: the ID layout and tag table ([[02-id-encoding#Bit layout]]), inline literal encodings and canonical forms ([[02-id-encoding#Canonical forms]]), table names and key layouts of the v1 encoding ([[04-physical-layout#Tables]]), the `meta` format-version record and feature-flag set. Not frozen: statistics formats, plan cache, anything rebuilt on open.

A file whose feature flags include one the build does not know is refused on open, never read.

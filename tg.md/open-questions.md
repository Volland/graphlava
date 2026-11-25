---
type: OpenQuestion
status: open
owner: Volodia
specs: ["adr:0011", "adr:0013", "adr:0014", "adr:0005", "adr:0006", "adr:0009", "adr:0010", "adr:0024", "adr:0029", "adr:0030"]
---
# Open questions

Design questions the specs leave open, each with options and a recommendation. Volodia decides; a decision becomes an ADR and the question gets a `resolved_by` edge.

Questions the grilling session already settled are in [[resolved-questions]].

## Value index drop policy

Auto-created value indexes cost writes; when and how they are dropped is not defined. Gate: v1.

- blocks:: [[01-data-model#Inline properties]]
- blocks:: [[04-physical-layout#Tables]]

<option id="value-index-drop-policy-usage-decay" outcome="recommended">

Drop an auto index after N days without use when its predicate's write rate is high; declared indexes are never dropped.

</option>

<option id="value-index-drop-policy-never" outcome="rejected">

Never drop: write cost grows without bound on hot predicates.

</option>

<option id="value-index-drop-policy-manual" outcome="deferred">

Only manual drop through the API.

</option>

## Large drops on mobile

Dropping a large graph in one transaction can exceed the iOS background time budget and grow the file. Gate: v1.

- blocks:: [[09-graphs#Graph drop]]

<option id="large-drops-on-mobile-chunked-drop" outcome="recommended">

Mark the graph dropped in one small transaction, hide it from queries, and delete its ranges in background batches.

</option>

<option id="large-drops-on-mobile-one-tx" outcome="rejected">

One transaction: simple, but can be killed mid-way on iOS and blocks the writer.

</option>

## IR textual syntax

Whether the IR has a stable text form for tests, debugging and persisted plan caches. Gate: v1.

- blocks:: [[10-ir#Operators]]
- blocks:: [[14-planner-execution#Plan cache]]

<option id="ir-textual-syntax-debug-only" outcome="recommended">

A text form for tests and `EXPLAIN` only, not stable across versions; plan caches stay in memory.

</option>

<option id="ir-textual-syntax-stable" outcome="deferred">

A stable, versioned text form: needed only for on-disk plan caches.

</option>

## Cypher null and comparison semantics

How far Cypher's three-valued logic and value comparison are emulated over RDF terms. Gate: v1.

- blocks:: [[10-ir#Modes]]
- blocks:: [[12-cypher#Mapping]]

<option id="cypher-null-and-comparison-semantics-flags" outcome="recommended">

Front-end flags in the IR (`null_semantics`, `compare_mode`) with Cypher semantics for the v1 subset; document gaps.

</option>

<option id="cypher-null-and-comparison-semantics-rdf" outcome="rejected">

Use SPARQL semantics for Cypher: surprising results for Cypher users.

</option>

## Cypher relationship type namespace

How Cypher type names, labels and property keys map to IRIs. Gate: v1.

- blocks:: [[12-cypher#Mapping]]

<option id="cypher-relationship-type-namespace-base" outcome="recommended">

A per-database base IRI plus the name; an explicit prefix map in the meta-model overrides it.

</option>

<option id="cypher-relationship-type-namespace-local" outcome="rejected">

Bare local names in a fixed namespace: collides across tenants.

</option>

## Cypher multi default

Whether predicates created through Cypher `CREATE` default to `multi`. Gate: v1.

- blocks:: [[12-cypher#Mapping]]

<option id="cypher-multi-default-unique" outcome="recommended">

Unique by default (ADR-0004); Cypher users declare `multi` types in the meta-model.

</option>

<option id="cypher-multi-default-multi" outcome="rejected">

Multi for Cypher-created predicates: matches Cypher, but two key shapes appear silently.

</option>

## Vector SPARQL syntax

How SPARQL calls the vector search operator. Gate: v1.

- blocks:: [[15-vector-index#IR operator]]

<option id="vector-sparql-syntax-service" outcome="recommended">

`SERVICE <meta:vector> { ... }`, the same pattern as Datalog.

</option>

<option id="vector-sparql-syntax-function" outcome="rejected">

A custom function in `FILTER`/`BIND`: cannot drive the join order.

</option>

<option id="vector-sparql-syntax-datalog-only" outcome="rejected">

Only through Datalog.

</option>

## Vector recall target

The recall@10 and p99 latency targets that define done for the vector index. Gate: v1.

- blocks:: [[15-vector-index#Training and rebalancing]]

<option id="vector-recall-target-measure-first" outcome="recommended">

Set targets after a first benchmark on a public dataset at 1M and 10M vectors; record them in the spec.

</option>

## Hot predicate threshold

How the overwrite rate is measured and which rate makes a predicate unstable for materialization. Gate: v1.

- blocks:: [[13-datalog#Materialized maintenance]]
- blocks:: [[14-planner-execution#Statistics]]

<option id="hot-predicate-threshold-ewma" outcome="recommended">

An exponentially weighted overwrite rate per predicate in `meta`, with a configurable threshold; to be measured.

</option>

<option id="hot-predicate-threshold-static" outcome="rejected">

Mark hot predicates manually only.

</option>

## Maintenance budget default

The default maintenance budget (Backward/Forward and counting) and whether it counts per rule or per transaction. Gate: v1.

- blocks:: [[13-datalog#Materialized maintenance]]

<option id="maintenance-budget-default-per-tx" outcome="recommended">

Per transaction, as a count of touched derived facts, default to be measured.

</option>

<option id="maintenance-budget-default-per-rule" outcome="rejected">

Per rule: one transaction can still touch many rules.

</option>

## Server authentication

Which authentication methods the server crate supports first. Gate: v1.

- blocks:: [[16-api#Server crate]]

<option id="server-authentication-local-token" outcome="recommended">

Unix socket with file permissions plus a bearer token for TCP; OIDC later.

</option>

<option id="server-authentication-none" outcome="rejected">

No authentication: unsafe outside localhost.

</option>

## Partitioned graph per file

Whether a partitioned graph may live in its own redb file instead of its own tables in the shared file. Gate: v1.

- blocks:: [[09-graphs#Partitioned graphs]]
- blocks:: [[adr:0002]]

<option id="partitioned-graph-per-file-tables" outcome="recommended">

Keep partitions as tables in one file (current spec) unless the spike shows crash repair or writer contention that grows with file size. One file keeps cross-graph transactions atomic.

</option>

<option id="partitioned-graph-per-file-files" outcome="deferred">

One redb file per partitioned graph (tenant or agent): independent crash repair, drop by unlinking the file, and one writer per file. A transaction that spans files loses atomicity unless a commit protocol is added; scopes that span files union several engines.

</option>

## Exact search threshold

The candidate-count × dimension crossover below which the vector operator searches exactly instead of through IVF lists. Gate: v1.

- blocks:: [[15-vector-index#Search modes]]

<option id="exact-search-threshold-measure" outcome="recommended">

Start at 2 × 10^6 and set it from [[17-benchmarks#Vector search recall and latency]] on desktop and mobile separately.

</option>

<option id="exact-search-threshold-fixed" outcome="rejected">

A fixed candidate count independent of dimension: wrong by a factor of two between 384 and 768 dimensions.

</option>

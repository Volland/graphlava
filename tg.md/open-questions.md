---
type: OpenQuestion
status: open
owner: Volodia
specs: ["adr:0011", "adr:0013", "adr:0014", "adr:0005", "adr:0006", "adr:0009", "adr:0010"]
---
# Open questions

Design questions the specs leave open, each with options and a recommendation. Volodia decides; a decision becomes an ADR and the question gets a `resolved_by` edge.

Questions the grilling session already settled are in [[resolved-questions]].

## Project name

The final name of the project and crate; `meta-core` is a working name. Gate: v0.

- blocks:: [[18-roadmap#v0]]

<option id="project-name-keep" outcome="rejected">

Keep `meta-core`: no work, but generic and hard to search.

</option>

<option id="project-name-new" outcome="recommended">

Pick a distinctive name before the first crate is published, because crate names, the file magic and the IRI namespace carry it.

</option>

## Multiple reifiers per triple

RDF 1.2 allows several reifiers for one triple; meta-core makes the statement ID the reifier, which allows one. Gate: v0.

- blocks:: [[01-data-model#Annotations]]
- blocks:: [[11-sparql#Reifiers]]

<option id="multiple-reifiers-per-triple-one" outcome="recommended">

One reifier per statement (the ID). A second reifier IRI becomes an alias that maps to the same ID. Fast and simple; differs from RDF 1.2 when two reifiers need separate annotations.

</option>

<option id="multiple-reifiers-per-triple-many" outcome="rejected">

Separate reifier nodes linked by virtual `rdf:reifies`: full RDF 1.2, but every annotation lookup needs a join and the zero-join reifier binding is lost.

</option>

<option id="multiple-reifiers-per-triple-multi-pred" outcome="deferred">

Allow several reifiers only for predicates declared `multi` (each parallel statement is its own reifier).

</option>

## Unasserted triple terms

Whether a reifier may refer to a triple that is not asserted in the store, as RDF 1.2 allows. Gate: v0.

- blocks:: [[01-data-model#Annotations]]
- blocks:: [[11-sparql#Reifiers]]

<option id="unasserted-triple-terms-flag" outcome="recommended">

Store it as a statement with an `unasserted` flag in `stmt`; orderings skip it for normal matching, reifier lookups see it. Keeps one ID scheme.

</option>

<option id="unasserted-triple-terms-reject" outcome="rejected">

Reject annotations on unasserted triples: simplest, but breaks RDF 1.2 data such as claims and quotes.

</option>

## System namespace IRI

The IRI namespace for meta-model terms (`meta:datalog`, `:in`, system graph). Gate: v0.

- blocks:: [[01-data-model#Meta-model]]

<option id="system-namespace-iri-urn" outcome="recommended">

A URN namespace such as `urn:meta-core:` (rename with the project): no DNS dependency, stable.

</option>

<option id="system-namespace-iri-http" outcome="rejected">

An `https://` namespace on a project domain: dereferenceable, but tied to domain ownership.

</option>

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

## Language tag table

Size and growth policy of the 16-bit language-tag table used by inline and dictionary language strings. Gate: v0.

- blocks:: [[02-id-encoding#Inline literals]]
- blocks:: [[03-dictionary#Layout]]

<option id="language-tag-table-table16" outcome="recommended">

A persistent append-only table of up to 65,535 canonical (lower-cased BCP 47) tags; overflow tags make the literal a dictionary literal with the full tag.

</option>

<option id="language-tag-table-iri" outcome="rejected">

Intern tags as dictionary IRIs: unbounded, but the 16-bit inline slot cannot hold them.

</option>

## Large literal storage

How literals of many kilobytes or megabytes (documents, transcripts) are stored in the dictionary. Gate: v1.

- blocks:: [[03-dictionary#Layout]]

<option id="large-literal-storage-overflow" outcome="recommended">

Values above a threshold (for example 4 KiB) move to an overflow table in chunks, keyed by a content hash for dedupe; `dict_fwd` keys on the hash.

</option>

<option id="large-literal-storage-inline" outcome="rejected">

Store in place: large keys in `dict_fwd` bloat the tree and slow lookups.

</option>

<option id="large-literal-storage-external" outcome="deferred">

Keep blobs outside the database file: breaks the single-file and portability rules.

</option>

## Inline double precision

An `xsd:double` has 64 bits, the inline payload 56; only doubles with 8 zero low mantissa bits fit. Gate: v0.

- blocks:: [[02-id-encoding#Inline literals]]

<option id="inline-double-precision-subset" outcome="recommended">

Inline only exact-fit doubles; others are dictionary literals with range filters served by value indexes (current spec).

</option>

<option id="inline-double-precision-float32" outcome="rejected">

Store doubles as lossy 32/56-bit floats: violates value identity.

</option>

<option id="inline-double-precision-wide" outcome="rejected">

Widen IDs to 128 bits: doubles every key.

</option>

## Literal predicate scan without value index

A pattern `?x :age ?v` with no value index has no index to drive it, because literals are not in the orderings. Gate: v0.

- blocks:: [[04-physical-layout#Tables]]
- blocks:: [[14-planner-execution#Join selection]]

<option id="literal-predicate-scan-without-value-index-pred-index" outcome="recommended">

Keep a cheap per-predicate node list `(pred, node)` for inline properties, always on, so any literal predicate scan is a range; value order still needs a value index.

</option>

<option id="literal-predicate-scan-without-value-index-full-scan" outcome="rejected">

Scan all property records: O(nodes) for any such pattern.

</option>

<option id="literal-predicate-scan-without-value-index-auto-only" outcome="deferred">

Rely on auto value indexes: first queries are slow.

</option>

## Large property records

Nodes with very many literal properties spill to `props_ext`; the threshold and layout need measurement. Gate: v0.

- blocks:: [[04-physical-layout#Encoding v1]]

<option id="large-property-records-spill256" outcome="recommended">

Spill above 256 entries or 2 KiB, whichever comes first; tune by benchmark.

</option>

<option id="large-property-records-always-split" outcome="rejected">

One key per property: loses the one-lookup read of a node.

</option>

## Page size on redb

redb's page size and its handling of values larger than a page affect the 16 KB target and the v2 block size. Gate: v0.

- blocks:: [[04-physical-layout#Encoding v2]]
- blocks:: [[05-storage-trait#redb adapter]]

<option id="page-size-on-redb-measure" outcome="recommended">

Measure in the bake-off; pick the v2 block size so a block fits one redb page.

</option>

<option id="page-size-on-redb-fork" outcome="deferred">

Patch redb for a configurable page size: maintenance burden.

</option>

## Page size on 16 KB devices

Android devices with 16 KB OS pages and Apple Silicon may prefer 16 KB database pages. Gate: v0.

- blocks:: [[05-storage-trait#redb adapter]]

<option id="page-size-on-16-kb-devices-measure" outcome="recommended">

Run the mobile benchmark on a 16 KB-page Android device before choosing anything engine-specific.

</option>

<option id="page-size-on-16-kb-devices-ignore" outcome="rejected">

Assume 4 KB pages are fine everywhere.

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

## Visibility before durability

With `relaxed` or `ordered`, readers can see a commit before it is durable, and a crash can then lose what they read. Gate: v0.

- blocks:: [[06-transactions-durability#Durability levels]]

<option id="visibility-before-durability-accept" outcome="recommended">

Accept and document it; acknowledgment to the writer still waits for durability under `ordered`. Offer a `durable_snapshot` read option later.

</option>

<option id="visibility-before-durability-hide" outcome="rejected">

Hide non-durable commits from readers: needs two snapshots and delays visibility for every reader.

</option>

## Crash repair time on large files

redb may repair allocator state after an unclean shutdown; the time on a 50+ GB file is unknown. Gate: v0.

- blocks:: [[06-transactions-durability#Crash recovery]]

<option id="crash-repair-time-on-large-files-measure" outcome="recommended">

Measure in the bake-off at 100M statements; if too slow, prefer clean shutdown hooks on mobile and document.

</option>

<option id="crash-repair-time-on-large-files-own-pager" outcome="deferred">

Own pager with a WAL: fast recovery, large effort.

</option>

## Delete with object-position references

A statement whose object is the deleted statement (`(:note, :about, stmt#42)`) is not an annotation; its fate is undefined. Gate: v0.

- blocks:: [[06-transactions-durability#Cascade delete]]

<option id="delete-with-object-position-references-block" outcome="recommended">

Block the delete unless `DETACH`, as for hyperedge roles; dangling object IDs are never created silently.

</option>

<option id="delete-with-object-position-references-cascade" outcome="rejected">

Cascade: deletes data the user may want to keep.

</option>

<option id="delete-with-object-position-references-dangle" outcome="rejected">

Keep with a dangling object: breaks the no-dangling rule of ADR-0006.

</option>

## Hyperedge role fillers that are literals

Whether a hyperedge role can hold a literal (a `:when` date) rather than a node. Gate: v0.

- blocks:: [[08-hyperedges#Record format]]

<option id="hyperedge-role-fillers-that-are-literals-allow" outcome="recommended">

Allow: the record stores the literal ID; the incidence index skips literal fillers and a value index covers lookups by value.

</option>

<option id="hyperedge-role-fillers-that-are-literals-forbid" outcome="rejected">

Forbid: forces a separate property on the hyperedge for every literal role.

</option>

## Moving an item to another graph

An item's home graph is fixed by its chunk; moving it needs a `gmem` override and a hidden home membership. Gate: v0.

- blocks:: [[09-graphs#Extra membership]]
- blocks:: [[08-hyperedges#Record format]]

<option id="moving-an-item-to-another-graph-override" outcome="recommended">

Record a move as `gmem` entry plus a `moved` tombstone for the home membership; scans subtract tombstones.

</option>

<option id="moving-an-item-to-another-graph-reallocate" outcome="rejected">

Give the item a new ID in the target chunk: breaks the stable-ID rule (D5).

</option>

## Built-in materialized rules in v0

v0 allows only on-demand user rules, but `in_star` must be materialized for scope expansion. Gate: v0.

- blocks:: [[09-graphs#Nesting DAG]]
- blocks:: [[13-datalog#Materialized maintenance]]

<option id="built-in-materialized-rules-in-v0-exempt" outcome="recommended">

Built-in rules (`in_star`) are materialized in v0 with hand-written counting maintenance; user `@materialized` waits for v1.

</option>

<option id="built-in-materialized-rules-in-v0-on-demand" outcome="rejected">

Compute `in_star` on demand: every scoped query pays a closure.

</option>

## SPARQL 1.2 draft pin

Which W3C SPARQL 1.2 / RDF 1.2 document version v0 implements and tests against. Gate: v0.

- blocks:: [[11-sparql#Conformance]]

<option id="sparql-1-2-draft-pin-latest-cr" outcome="recommended">

Pin the latest Candidate Recommendation at the start of v0 and record its date; re-pin only at milestones.

</option>

<option id="sparql-1-2-draft-pin-track" outcome="rejected">

Track the editor's draft: constant churn.

</option>

## Reifier IRI form

How a statement-ID reifier appears in results: an IRI `urn:meta-core:stmt:{id}` or a blank node. Gate: v0.

- blocks:: [[11-sparql#Reifiers]]

<option id="reifier-iri-form-iri" outcome="recommended">

A stable IRI built from the ID: round-trips through clients and can be used in later queries.

</option>

<option id="reifier-iri-form-bnode" outcome="rejected">

Blank node: RDF-idiomatic, but clients cannot refer to it again.

</option>

## Reifier on existing statement

An update names a new IRI reifier for a triple that already has an ID. Gate: v0.

- blocks:: [[11-sparql#Reifiers]]

<option id="reifier-on-existing-statement-alias" outcome="recommended">

Record the IRI as an alias of the existing ID (one statement, see Multiple reifiers per triple).

</option>

<option id="reifier-on-existing-statement-error" outcome="rejected">

Reject the update.

</option>

## Default write graph

Where `INSERT DATA` without `GRAPH` writes, given there is no `g` column. Gate: v0.

- blocks:: [[11-sparql#Graph semantics]]

<option id="default-write-graph-default-graph" outcome="recommended">

A fixed default graph created with the file; transaction option can override it.

</option>

<option id="default-write-graph-system" outcome="rejected">

The system graph: mixes user data with the meta-model.

</option>

## SPARQL parser reuse

Reuse `spargebra` (Oxigraph) or write an own parser for SPARQL 1.2 syntax and the extensions. Gate: v0.

- blocks:: [[11-sparql#Scope]]

<option id="sparql-parser-reuse-spargebra" outcome="recommended">

Reuse `spargebra` and extend it for `GRAPH!` and the in-place update; contribute RDF 1.2 support upstream where missing.

</option>

<option id="sparql-parser-reuse-own" outcome="rejected">

Own parser: full control, large effort.

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

## Unbound predicate cost

Patterns with an unbound predicate union three sources (orderings, props, hyperedge roles). Gate: v0.

- blocks:: [[10-ir#Virtual relations]]

<option id="unbound-predicate-cost-summary" outcome="recommended">

Keep a per-node bitmap or list of predicates present in `props` to prune; measure.

</option>

<option id="unbound-predicate-cost-union" outcome="rejected">

Always union all three: correct, slow for `?s ?p ?o` around a node.

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

## Raw vector retention

Whether raw vectors are kept next to PQ codes. Gate: v1.

- blocks:: [[15-vector-index#IVF-PQ layout]]

<option id="raw-vector-retention-optional" outcome="recommended">

Off by default on mobile, on by default on desktop; needed for re-ranking and retraining.

</option>

<option id="raw-vector-retention-always" outcome="rejected">

Always: doubles index size on phones.

</option>

## Vector recall target

The recall@10 and p99 latency targets that define done for the vector index. Gate: v1.

- blocks:: [[15-vector-index#Training]]

<option id="vector-recall-target-measure-first" outcome="recommended">

Set targets after a first benchmark on a public dataset at 1M and 10M vectors; record them in the spec.

</option>

## Datalog writes to base data

Whether and how a Datalog program asserts base facts, not only derived ones. Gate: v0.

- blocks:: [[13-datalog#Surface syntax]]

<option id="datalog-writes-to-base-data-assert" outcome="recommended">

Allow facts and rule heads marked as assertions (for example an `@assert` directive) inside a write transaction; unmarked heads stay derived.

</option>

<option id="datalog-writes-to-base-data-readonly" outcome="rejected">

Datalog is read-only: writes go through SPARQL Update or the API.

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

The default DRed budget and whether it counts per rule or per transaction. Gate: v1.

- blocks:: [[13-datalog#Materialized maintenance]]

<option id="maintenance-budget-default-per-tx" outcome="recommended">

Per transaction, as a count of touched derived facts, default to be measured.

</option>

<option id="maintenance-budget-default-per-rule" outcome="rejected">

Per rule: one transaction can still touch many rules.

</option>

## Datalog query syntax

Whether standalone Datalog accepts `?-` goal queries or only Nemo `@output` predicates. Gate: v0.

- blocks:: [[13-datalog#Surface syntax]]

<option id="datalog-query-syntax-both" outcome="recommended">

Accept Nemo `@output` and a `?- goal.` shorthand that compiles to it.

</option>

<option id="datalog-query-syntax-nemo-only" outcome="rejected">

Only Nemo syntax: less convenient for interactive queries.

</option>

## Missing ordering for cyclic patterns

A cyclic query whose best variable order needs a missing ordering. Gate: v0.

- blocks:: [[14-planner-execution#On-demand orderings]]

<option id="missing-ordering-for-cyclic-patterns-fallback" outcome="recommended">

Run with the best available plan, count the miss, and build the ordering in the background when misses repeat.

</option>

<option id="missing-ordering-for-cyclic-patterns-sync-build" outcome="rejected">

Build synchronously: one query can stall the writer for minutes.

</option>

## Statistics drift threshold

The drift that invalidates cached plans and triggers a statistics rebuild. Gate: v0.

- blocks:: [[14-planner-execution#Plan cache]]

<option id="statistics-drift-threshold-factor" outcome="recommended">

Invalidate when a cardinality used by the plan drifts by a factor (default 4, to be tuned).

</option>

<option id="statistics-drift-threshold-time" outcome="rejected">

Invalidate on a timer: ignores the actual data change.

</option>

## Extension write queue

Whether iOS extensions may queue writes in a side file when the owner app is not running. Gate: v1.

- blocks:: [[16-api#Mobile bindings]]

<option id="extension-write-queue-queue" outcome="recommended">

Append to a small per-extension journal file in the app group; the app applies it on next start.

</option>

<option id="extension-write-queue-fail" outcome="rejected">

Fail the write: simple, but extensions lose data.

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

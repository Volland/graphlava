---
type: OpenQuestion
status: decided
owner: Volodia
specs: ["adr:0002", "adr:0003", "adr:0006", "adr:0004", "adr:0017", "adr:0007", "adr:0008", "adr:0018", "adr:0009", "adr:0016", "adr:0005", "adr:0012", "adr:0015", "adr:0010", "adr:0013", "adr:0014", "adr:0011", "adr:0021", "adr:0022", "adr:0023", "adr:0024", "adr:0025", "adr:0028", "adr:0030", "adr:0031", "adr:0032", "adr:0033", "adr:0034", "adr:0035", "adr:0036", "adr:0037", "adr:0038", "adr:0039", "adr:0040", "adr:0041", "adr:0043"]
---
# Resolved questions

Design questions from the handover and the grilling session of 2026-11-24, each answered by an ADR.

## Multi-process access

Must several OS processes open one database file at once? Decided: no, one owner process with IPC.

- resolved_by:: [[adr:0002]] {date: 2026-11-24}

## Graph scoping cost

How scoped queries avoid O(database) scans without a graph column. Decided: ID-range plans for small graphs, partitioned tables for large ones.

- resolved_by:: [[adr:0003]] {date: 2026-11-24}

## Deleting annotated statements

What happens to annotations when their statement is deleted. Decided: cascade, with hyperedge-role block and KEEP ANNOTATIONS.

- resolved_by:: [[adr:0006]] {date: 2026-11-24}

## Parallel statements

Whether several statements may share one (s,p,o). Decided: unique by default, multi predicates opt in.

- resolved_by:: [[adr:0004]] {date: 2026-11-24}

## Membership statement IDs

Whether graph-membership triples carry IDs. Decided: lazily, on annotation.

- resolved_by:: [[adr:0017]] {date: 2026-11-24}

## GRAPH semantics

Whether GRAPH scope is direct or transitive. Decided: transitive by default, GRAPH! for direct.

- resolved_by:: [[adr:0007]] {date: 2026-11-24}

## Graph nesting shape

Tree, DAG or cyclic nesting. Decided: DAG with write-time cycle rejection and a soft depth limit.

- resolved_by:: [[adr:0008]] {date: 2026-11-24}

## File portability

Whether one file moves between desktop and phone. Decided: one portable, versioned format.

- resolved_by:: [[adr:0018]] {date: 2026-11-24}

## Derived fact IDs

Whether materialized facts get statement IDs and where they live. Decided: lazy IDs, separate derived table, budgeted maintenance.

- resolved_by:: [[adr:0009]] {date: 2026-11-24}

## Dictionary GC

Reference counts or sweep for unused terms. Decided: background sweep.

- resolved_by:: [[adr:0016]] {date: 2026-11-24}

## Property demotion

Whether a promoted property returns inline when annotations go. Decided: never, with routing by term kind.

- resolved_by:: [[adr:0005]] {date: 2026-11-24}

## Durability on redb

How sync, ordered and relaxed map onto redb. Decided: by guarantee, ordered is group commit.

- resolved_by:: [[adr:0012]] {date: 2026-11-24}

## Ordering count

How many orderings are always maintained. Decided: SPO, POS, OSP, with the rest on demand.

- resolved_by:: [[adr:0015]] {date: 2026-11-24}

## Hyperedge and graph ID ranges

How per-type and per-graph ID ranges coexist. Decided: chunks leased per (graph, kind).

- resolved_by:: [[adr:0010]] {date: 2026-11-24}

## Query surface order

Which language comes first. Decided: SPARQL 1.2 and standalone Datalog in v0, Cypher in v1.

- resolved_by:: [[adr:0013]] {date: 2026-11-24}

## Datalog access

How Datalog is reached. Decided: standalone language plus SERVICE <meta:datalog> inside SPARQL.

- resolved_by:: [[adr:0014]] {date: 2026-11-24}

## ID layout

The 64-bit layout and literal encoding. Decided: 4-bit kind, 40+20 chunks, order-preserving inline literals.

- resolved_by:: [[adr:0011]] {date: 2026-11-24}

## Literal predicate scan without value index

How `?x :p ?v` is driven without a value index. Decided: an always-on `plist (pred, node)` table that overwrites never touch.

- resolved_by:: [[adr:0022]] {date: 2026-11-24}

## Inline double precision

Which doubles are inline. Decided: a prefix-coded layout gives doubles 62 payload bits, so every double with exponent in [-255, 254] is inline and exact.

- resolved_by:: [[adr:0021]] {date: 2026-11-24}

## Large literal storage

Where long literals live and how they are reclaimed. Decided: literals over 256 bytes are owned by one slot, not interned, and deleted with it.

- resolved_by:: [[adr:0023]] {date: 2026-11-24}

## Visibility before durability

Whether readers may see commits that a crash can lose. Decided: a batch is one engine transaction, so `sync` and `ordered` writes become visible only when durable.

- resolved_by:: [[adr:0025]] {date: 2026-11-24}

## Moving an item to another graph

How an item changes graphs without a new ID. Decided: an extra membership plus a `moved` tombstone on the home membership; drop keeps shared items.

- resolved_by:: [[adr:0024]] {date: 2026-11-24}

## Hyperedge role fillers that are literals

Whether a role can hold a literal. Decided: no; literal values are attributes stored as inline properties of the hyperedge.

- resolved_by:: [[adr:0028]] {date: 2026-11-24}

## Raw vector retention

Whether raw vectors are kept. Decided: yes, in `f16` by default, for exact search over small scopes and re-ranking.

- resolved_by:: [[adr:0030]] {date: 2026-11-24}

## Project name

The final name of the project and crate; `meta-core` is a working name. Decided: `graphlava`, matching the repository and graphlava.org.

- resolved_by:: [[adr:0031]] {date: 2026-11-24}

## System namespace IRI

The IRI namespace for meta-model terms (`meta:datalog`, `:in`, system graph). Decided: `https://graphlava.org/ns#` (prefix `meta:`).

- resolved_by:: [[adr:0031]] {date: 2026-11-24}

## Reifier IRI form

How a statement-ID reifier appears in results: an IRI `urn:meta-core:stmt:{id}` or a blank node. Decided: a stable IRI `https://graphlava.org/ns/stmt/{id}`.

- resolved_by:: [[adr:0031]] {date: 2026-11-24}

## Multiple reifiers per triple

RDF 1.2 allows several reifiers for one triple; graphlava makes the statement ID the reifier, which allows one. Decided: one reifier per statement (its ID); further reifier IRIs are aliases.

- resolved_by:: [[adr:0032]] {date: 2026-11-24}

## Reifier on existing statement

An update names a new IRI reifier for a triple that already has an ID. Decided: the new IRI becomes an alias of the existing ID.

- resolved_by:: [[adr:0032]] {date: 2026-11-24}

## Unasserted triple terms

Whether a reifier may refer to a triple that is not asserted in the store, as RDF 1.2 allows. Decided: stored as a statement with an `unasserted` flag; orderings skip it, reifier lookups see it.

- resolved_by:: [[adr:0032]] {date: 2026-11-24}

## Language tag table

Size and growth policy of the 16-bit language-tag table used by inline and dictionary language strings. Decided: a 16-bit append-only table of canonical tags; overflow goes to the dictionary.

- resolved_by:: [[adr:0033]] {date: 2026-11-24}

## Large property records

Nodes with very many literal properties spill to `props_ext`; the threshold and layout need measurement. Decided: spill above 256 entries or 2 KiB, tuned by benchmark.

- resolved_by:: [[adr:0034]] {date: 2026-11-24}

## Page size on redb

redb's page size and its handling of values larger than a page affect the 16 KB target and the v2 block size. Decided: measured in the bake-off; v2 blocks sized to one redb page.

- resolved_by:: [[adr:0035]] {date: 2026-11-24}

## Page size on 16 KB devices

Android devices with 16 KB OS pages and Apple Silicon may prefer 16 KB database pages. Decided: measured on a 16 KB-page Android device before any engine-specific choice.

- resolved_by:: [[adr:0035]] {date: 2026-11-24}

## Delete with object-position references

A statement whose object is the deleted statement (`(:note, :about, stmt#42)`) is not an annotation; its fate is undefined. Decided: blocked unless `DETACH`, as for hyperedge roles.

- resolved_by:: [[adr:0036]] {date: 2026-11-24}

## Built-in materialized rules in v0

v0 allows only on-demand user rules, but `in_star` must be materialized for scope expansion. Decided: built-in rules (`in_star`) are materialized in v0; user `@materialized` waits for v1.

- resolved_by:: [[adr:0037]] {date: 2026-11-24}

## SPARQL 1.2 draft pin

Which W3C SPARQL 1.2 / RDF 1.2 document version v0 implements and tests against. Decided: pin the latest dated W3C TR snapshot (a Working Draft until a CR exists); re-pin at milestones.

- resolved_by:: [[adr:0038]] {date: 2026-11-24}

## SPARQL parser reuse

Reuse `spargebra` (Oxigraph) or write an own parser for SPARQL 1.2 syntax and the extensions. Decided: reuse and extend `spargebra`.

- resolved_by:: [[adr:0038]] {date: 2026-11-24}

## Default write graph

Where `INSERT DATA` without `GRAPH` writes, given there is no `g` column. Decided: a fixed default graph created with the file, overridable per transaction.

- resolved_by:: [[adr:0038]] {date: 2026-11-24}

## Unbound predicate cost

Patterns with an unbound predicate union three sources (orderings, props, hyperedge roles). Decided: a per-node predicate summary prunes the sources.

- resolved_by:: [[adr:0039]] {date: 2026-11-24}

## Datalog writes to base data

Whether and how a Datalog program asserts base facts, not only derived ones. Decided: heads and facts marked `@assert` write base data; others stay derived.

- resolved_by:: [[adr:0040]] {date: 2026-11-24}

## Datalog query syntax

Whether standalone Datalog accepts `?-` goal queries or only Nemo `@output` predicates. Decided: both Nemo `@output` and a `?- goal.` shorthand.

- resolved_by:: [[adr:0040]] {date: 2026-11-24}

## Missing ordering for cyclic patterns

A cyclic query whose best variable order needs a missing ordering. Decided: fall back to the best available plan and build the ordering in the background when misses repeat.

- resolved_by:: [[adr:0041]] {date: 2026-11-24}

## Statistics drift threshold

The drift that invalidates cached plans and triggers a statistics rebuild. Decided: invalidate a cached plan when a cardinality it used drifts by a factor of 4 (tunable).

- resolved_by:: [[adr:0041]] {date: 2026-11-24}

## Crash repair time on large files

How long redb repairs a file after an unclean shutdown. Measured: 3.4-15.5 s at 10M statements and 105-221 s at 100M; with quick repair about 10 ms, at twice the durable-commit latency.

- resolved_by:: [[22-implementation-decisions#Quick repair on mobile]] {date: 2026-11-24}

## Extension write queue

Whether iOS extensions may queue writes in a side file when the owner app is not running. Decided: yes, each extension appends to its own journal in the app group, and the owner applies every complete frame exactly once on its next open.

- resolved_by:: [[adr:0043]] {date: 2026-11-24}

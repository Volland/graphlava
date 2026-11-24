---
type: OpenQuestion
status: decided
owner: Volodia
specs: ["adr:0002", "adr:0003", "adr:0006", "adr:0004", "adr:0017", "adr:0007", "adr:0008", "adr:0018", "adr:0009", "adr:0016", "adr:0005", "adr:0012", "adr:0015", "adr:0010", "adr:0013", "adr:0014", "adr:0011"]
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

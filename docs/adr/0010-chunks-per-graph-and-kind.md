---
status: accepted
date: 2026-11-24
decision-makers: [Volodia]
---
# Lease ID chunks per (home graph, kind)

## Context and Problem Statement

Statement IDs are leased in chunks per home graph so a graph scan is a set of ID ranges ([ADR-0003](0003-graph-scoping-hybrid.md)). Hyperedge IDs were to be allocated per type so a type scan is a range. One ID cannot sit in both a per-graph chunk and a per-type range.

## Considered Options

<option id="graph-kind-chunks" outcome="chosen">

A chunk lease (about 2^20 IDs) is tagged `(graph, kind)`: kind is `stmt` for statements and the hyperedge type for hyperedges. A type scan is the union of that type's chunks across graphs; a graph scan is the union of that graph's chunks across kinds. A `kind_chunk` secondary table maps kind to chunks; the planner caches it.

</option>

<option id="separate-hyperedge-space" outcome="rejected">

Separate hyperedge ID space with `gmem` for all hyperedge membership: loses the zero-byte membership for hyperedges.

</option>

<option id="scan-incidence" outcome="rejected">

Drop per-type ranges and find a type through `incidence`: type scans become index scans instead of ranges.

</option>

## Decision Outcome

Chosen option: "graph-kind-chunks", because both type and graph scans stay range-driven without per-row membership entries.

### Consequences

<consequence kind="good">

Hyperedges and statements share one allocation scheme; graph drop and type scan are range operations.

</consequence>

<consequence kind="bad">

Sparse `(graph, kind)` pairs waste part of a chunk. The planner reads a small `kind_chunk` table when expanding ranges. Moving an item to another graph needs a `gmem` override, because its ID stays in the old chunk (see [ADR-0003](0003-graph-scoping-hybrid.md)).

</consequence>

## More Information

Refines handover sections 5.1 and 5.2.

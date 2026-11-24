---
status: accepted
date: 2026-11-24
decision-makers: [Volodia]
---
# Scope named graphs by ID range for small graphs and partitioned tables for large ones

## Context and Problem Statement

Statements carry no `g` column (handover D6). `GRAPH :g { ?s :p ?o }` would scan a global ordering and filter by ID, so scoped query cost grows with database size, not graph size. The workload has both many small graphs (episodes, documents) and a few large ones (tenants, agents).

## Considered Options

<option id="hybrid" outcome="chosen">

Small graphs: drive the scan from the graph's statement-ID ranges (sequential `stmt` reads, then probe the orderings). Large graphs: partitioned graphs with their own tables, promoted automatically above a size threshold. Membership is `gchunk` by default plus `gmem` overrides, so moving a statement keeps its ID.

</option>

<option id="g-column" outcome="rejected">

Leading `g` key component in all six orderings: fast scoped scans, but a 4th key column everywhere, cross-graph queries need a union, and nested graphs make membership transitive anyway.

</option>

<option id="filter-only" outcome="rejected">

Pure ID-range filter on global scans (original draft): O(database) for any scoped query on a large graph.

</option>

## Decision Outcome

Chosen option: "hybrid", because it makes scoped cost proportional to graph size on both ends while keeping the common unscoped path free of any graph column.

### Consequences

<consequence kind="good">

Primary membership costs zero bytes per statement; graph drop is range deletion; large tenants get isolation and fast drop.

</consequence>

<consequence kind="bad">

The planner must choose between range-driven, filter-driven and partitioned plans. Promotion of a graph into partitioned tables is a migration the engine must run online. Cross-graph queries over partitioned graphs are slower.

</consequence>


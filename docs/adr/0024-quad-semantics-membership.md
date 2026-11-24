---
status: accepted
date: 2026-11-24
decision-makers: [Volodia]
---
# One statement, many graphs: inserts add memberships, drops rehome shared items, uniqueness is per store

## Context and Problem Statement

Statements are unique per `(s,p,o)` ([ADR-0004](0004-unique-statements-multi-opt-in.md)) and belong to a home graph by ID chunk ([ADR-0010](0010-chunks-per-graph-and-kind.md)). The specs did not say what inserting an existing triple into a second graph does, plain `DROP GRAPH` deleted items still in other graphs, and partitioned graphs ([ADR-0003](0003-graph-scoping-hybrid.md)) broke global uniqueness because their statements are absent from the global orderings.

## Considered Options

<option id="membership-quads" outcome="chosen">

- Insert of an existing `(s,p,o)` into graph g adds a `gmem` membership of g (no-op if already a member). The statement keeps its ID.
- Delete of `(s,p,o)` inside `GRAPH g` removes only the membership in g. The statement is deleted, with its cascade, when its last membership goes. A home membership is removed with a `moved` tombstone in `gmem`.
- Plain drop of g removes every membership in g. Items of g that are still members elsewhere survive with a tombstone; the others are deleted.
- Uniqueness of `(s,p,o)` holds per store: the global tables, or one partitioned graph's tables. A scan that unions stores deduplicates non-multi statements by `(s,p,o)`; a reifier pattern can bind one ID per store.
- Per-graph provenance hangs off the promoted membership `(stmt :in g)` ([ADR-0017](0017-lazy-membership-ids.md)).

</option>

<option id="statement-per-graph" outcome="rejected">

A separate statement per graph (true quads): matches SPARQL datasets exactly, but duplicates orderings per graph and breaks one ID per fact.

</option>

<option id="probe-all-partitions" outcome="rejected">

Global uniqueness by probing every partition on insert: write cost grows with the partition count.

</option>

## Decision Outcome

Chosen option: "membership-quads", because it gives SPARQL dataset behaviour for insert, delete and drop without per-graph copies, keeps one ID per fact, and makes partition writes local.

### Consequences

<consequence kind="good">

No data loss on drop; the same fact can be asserted by several episodes and annotated per episode through its memberships.

</consequence>

<consequence kind="bad">

Drop must check `gmem_rev` for each item, so it is no longer a pure range delete (cost O(items in g) probes). Cross-store unions pay a merge-dedupe. A triple stored in a partition and in the global store has two IDs.

</consequence>

## More Information

Amends [ADR-0003](0003-graph-scoping-hybrid.md), [ADR-0004](0004-unique-statements-multi-opt-in.md), [ADR-0010](0010-chunks-per-graph-and-kind.md). Resolves open question "Moving an item to another graph".

---
status: accepted
date: 2026-11-24
decision-makers: [Volodia]
---
# `GRAPH :g { P }` matches transitively by default; direct membership is explicit

## Context and Problem Statement

Graphs nest (`(child, :in, parent)`). A query scoped to a graph must say whether statements in nested graphs count. The memory hierarchy (agent -> episode) is the main use, and users expect scoping to an outer graph to include its contents.

## Considered Options

<option id="transitive-default" outcome="chosen">

`GRAPH :g { P }` matches statements in `:g` and all its descendants. The planner expands it to the union of descendant ID ranges (roaring bitmap) using the materialized `in_star` rule. Direct-only scope is explicit: `GRAPH! :g { P }` in SPARQL, `:in` (without star) in Cypher/Datalog. With an unbound `?g`, the variable binds the direct home graph (and `gmem` extras), never an ancestor.

</option>

<option id="direct-default" outcome="rejected">

Standard SPARQL semantics, cheapest per query, but the common hierarchy case needs the explicit form every time.

</option>

## Decision Outcome

Chosen option: "transitive-default", because scoping to a parent graph should include its children without extra syntax.

### Consequences

<consequence kind="good">

Hierarchical memory queries are one `GRAPH` clause. Set semantics make a statement reachable by several paths count once.

</consequence>

<consequence kind="bad">

Deviates from standard SPARQL `GRAPH` semantics, so standard-conformance tests need a `direct` compatibility mode. Deep or wide nesting makes the range union larger; the planner needs a cost cut-over to filter-driven plans. `in_star` must be maintained on every `:in` change, and cycles must be controlled (see open question on nesting shape).

</consequence>

## More Information

Resolves handover open question 6. Builds on [ADR-0003](0003-graph-scoping-hybrid.md).

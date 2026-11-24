---
status: accepted
date: 2026-11-24
decision-makers: [Volodia]
---
# Route literal objects to inline per-node properties, with auto value indexes and no demotion

## Context and Problem Statement

Agent-memory writes are mostly value overwrites. Storing every literal-valued statement in six orderings makes an overwrite touch seven trees. A per-node packed property record touches one, but splits data across two sources and weakens value-driven lookups.

## Considered Options

<option id="route-by-kind" outcome="chosen">

Literal object -> `props` record; node/statement/graph object -> orderings. Value indexes (`propidx_<pred>`) are created automatically when the planner sees repeated value-driven lookups, or declared in the meta-model. A promoted (annotated) property gets a statement ID and moves to the orderings; its `props` slot keeps a `(pred -> stmt_id)` pointer. No demotion.

</option>

<option id="all-in-orderings" outcome="rejected">

Uniform and fully indexed, but 7x write cost on overwrites.

</option>

<option id="per-property-choice" outcome="rejected">

User picks inline or indexed per property: more knobs, easy to misconfigure, planner must handle every mix.

</option>

## Decision Outcome

Chosen option: "route-by-kind", because the routing rule is deterministic, overwrites touch one tree, and the planner knows statically which source serves a pattern.

### Consequences

<consequence kind="good">

Overwrite cost is one tree. Reading a node's properties is one lookup, including promoted ones.

</consequence>

<consequence kind="bad">

Patterns with an unbound predicate must union `props` and the orderings. Auto-created value indexes add write cost to hot predicates and need a drop policy. Promotion is one-way, so annotated-then-cleaned properties stay in the orderings.

</consequence>

## More Information

Resolves handover open question 10. Refines handover D7 and section 3.4.

Amended by [ADR-0022](0022-value-index-sort-keys.md) (2026-11-24): value indexes are one `propidx` table keyed `(pred, sort_key, node, value_id)`, and an always-on `plist` `(pred, node)` drives literal predicate scans.

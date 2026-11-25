---
status: accepted
date: 2026-11-24
decision-makers: [Volodia]
---
# Per-node predicate summaries prune unbound-predicate patterns

## Context and Problem Statement

Patterns with an unbound predicate union three sources: orderings, inline properties and hyperedge roles.

## Considered Options

<option id="summary" outcome="chosen">

Keep a per-node summary (bitmap or list) of the predicates present in `props` and use it to prune sources for `?p` patterns; measure its cost.

</option>

<option id="always-union" outcome="rejected">

Always union all three sources: correct, slow for `?s ?p ?o` around a node.

</option>

## Decision Outcome

Chosen option: "summary", because `?s ?p ?o` around a node is a common agent-memory query.

### Consequences

<consequence kind="good">

Describe-style queries touch only sources that can match.

</consequence>

<consequence kind="bad">

The summary must be kept in step with `props` on every write.

</consequence>

## More Information

Resolves open question "Unbound predicate cost".

---
status: accepted
date: 2026-11-24
decision-makers: [Volodia]
---
# Index nested-loop join is the default for selective patterns; paths use a BFS kernel

## Context and Problem Statement

The planner chose Leapfrog Triejoin for cyclic patterns and merge or hash joins otherwise. With only SPO, POS and OSP ([ADR-0015](0015-three-base-orderings.md)), a 2-hop traversal such as `:alice :knows ?x . ?x :worksAt ?c` has no merge order (it needs PSO), and a hash join scans every `:worksAt` statement. Property paths compiled to semi-naive fixpoints over sorted vectors are heavy for single-source reachability.

## Considered Options

<option id="inlj-default" outcome="chosen">

- Index nested-loop join (bind join): the outer batch is sorted on the join key and each key is a forward `seek` into the ordering whose prefix it binds. It is the default for acyclic joins whose outer side is estimated small relative to the inner relation.
- Merge join when both sides arrive sorted and are large; hash join when neither ordering fits and both are large; Leapfrog Triejoin for cyclic patterns.
- `Path` with a bound end runs a BFS kernel with a roaring visited set, bidirectional when both ends are bound; `Fixpoint` remains for general recursion.

</option>

<option id="merge-hash-only" outcome="rejected">

Merge or hash joins only for acyclic patterns: full predicate scans on every traversal hop.

</option>

<option id="ltj-everywhere" outcome="deferred">

One generic-join kernel for every pattern (Free Join, Wang et al., SIGMOD 2023): fewer operators, but a larger first implementation. Revisit after v0.

</option>

## Decision Outcome

Chosen option: "inlj-default", because traversal cost becomes proportional to the frontier, not to the predicate, which is what makes three base orderings sufficient for the agent-memory mix.

### Consequences

<consequence kind="good">

2–3 hop traversals cost O(frontier · log n) seeks; the three-ordering layout holds without PSO for the mix.

</consequence>

<consequence kind="bad">

One more physical operator and a cost-model crossover (outer size versus inner size) to measure.

</consequence>

## More Information

Supports [ADR-0015](0015-three-base-orderings.md).

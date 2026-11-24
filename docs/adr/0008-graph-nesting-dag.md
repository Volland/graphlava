---
status: accepted
date: 2026-11-24
decision-makers: [Volodia]
---
# Graph nesting is a DAG with write-time cycle rejection

## Context and Problem Statement

Graphs nest through `(child, :in, parent)`. Transitive scope ([ADR-0007](0007-graph-scope-transitive-default.md)) needs a well-defined closure, and the memory model needs shared sub-graphs (one episode under both an agent and a project).

## Considered Options

<option id="dag" outcome="chosen">

Several parents allowed. Adding `(a, :in, b)` fails if `b` is already a descendant of `a`, checked against `in_star`. Soft depth limit of 64 (configurable). A statement reachable by several paths counts once. Dropping a graph deletes its own statements and its `:in` edges; descendants are never deleted by a plain drop, and `DROP RECURSIVE` is the explicit form.

</option>

<option id="tree" outcome="rejected">

One parent per graph: simplest closure and range handling, but blocks sharing.

</option>

<option id="cyclic" outcome="rejected">

Arbitrary nesting: `in_star` becomes a general fixpoint and graph drop is ambiguous.

</option>

## Decision Outcome

Chosen option: "dag", because it allows sharing while keeping the closure acyclic and cheap to check.

### Consequences

<consequence kind="good">

`in_star` is a plain DAG closure; cycle checks reuse it; scope unions dedupe naturally.

</consequence>

<consequence kind="bad">

Every `:in` insert pays a closure lookup and a closure update. Deleting an edge in a DAG with shared paths needs counting maintenance for `in_star`. Orphaned descendants after a plain drop are possible and must stay reachable (they become roots).

</consequence>

## More Information

Refines [ADR-0003](0003-graph-scoping-hybrid.md) and [ADR-0007](0007-graph-scope-transitive-default.md).

Amended by [ADR-0029](0029-incremental-maintenance-algorithms.md) (2026-11-24): `in_star` stores the closure as a set; edge removal recomputes ancestor sets instead of subtracting path counts.

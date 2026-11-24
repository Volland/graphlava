---
status: accepted
date: 2026-11-24
decision-makers: [Volodia]
---
# Three always-on orderings (SPO, POS, OSP); the other three on demand

## Context and Problem Statement

Each statement write touches every ordering. Six orderings (SPO, SOP, PSO, POS, OSP, OPS) give the best coverage for Leapfrog Triejoin but multiply write cost and index size, and redb has no prefix compression. With unique statements ([ADR-0004](0004-unique-statements-multi-opt-in.md)) an entry is `(a,b,c) -> id`.

## Considered Options

<option id="three-plus-on-demand" outcome="chosen">

SPO, POS and OSP are always on; the value is the statement `id` in each, so reifier binding needs no extra join. PSO, SOP and OPS are created online when statistics show a repeated pattern that needs them, or when declared in the meta-model, and can be dropped. Without a fitting ordering the planner uses a merge or hash join, or builds the ordering if the pattern is hot. A base write touches three orderings plus `stmt`. The bake-off compares this layout against all six always on.

</option>

<option id="all-six" outcome="deferred">

Best join coverage; highest write cost and size. Kept as the comparison baseline in the bake-off.

</option>

## Decision Outcome

Chosen option: "three-plus-on-demand", because it cuts write amplification and index size roughly in half while three prefixes already cover every one- and two-bound pattern.

### Consequences

<consequence kind="good">

Four trees per base write instead of seven; smaller files; a hot pattern can still get its ordering.

</consequence>

<consequence kind="bad">

Cyclic patterns whose best variable order needs a missing ordering run slower until it exists. The planner needs usage statistics and online index build and drop. Plans depend on which orderings exist at the time.

</consequence>

## More Information

Amends handover D8 and section 4. Same on-demand principle as value indexes in [ADR-0005](0005-inline-properties-routing.md).

Amended by [ADR-0027](0027-join-kernels.md) (2026-11-24): the planner's default join for selective acyclic patterns is an index nested-loop join, which is what lets three orderings serve 2–3 hop traversals.

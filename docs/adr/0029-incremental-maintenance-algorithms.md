---
status: accepted
date: 2026-11-24
decision-makers: [Volodia]
---
# Recursive rules use Backward/Forward maintenance; the graph closure stores sets, not path counts

## Context and Problem Statement

[ADR-0009](0009-datalog-maintenance-policy.md) maintains recursive `@materialized` rules with DRed, whose over-deletion phase is what exhausts the per-transaction budget. The `in_star` graph closure ([ADR-0008](0008-graph-nesting-dag.md)) stores a `u32` path count per pair; path counts grow exponentially in a DAG (about 32 stacked diamonds overflow `u32`, well within the depth limit of 64).

## Considered Options

<option id="bf-and-set-closure" outcome="chosen">

- Recursive materialised rules use the Backward/Forward algorithm (Motik, Nenov, Piro, Horrocks, AAAI 2015): a fact touched by a deletion is checked for an alternative derivation backwards before it is deleted, so over-deletion is avoided. Non-recursive rules keep counting.
- `in_star` stores the closure as a set. Adding edge `(a, b)` inserts `(d, u)` for every `d` in `desc(a) ∪ {a}` and `u` in `anc(b) ∪ {b}`. Removing an edge recomputes `anc(d) = ∪ (anc(p) ∪ {p})` over parents `p` for every `d` in `desc(a) ∪ {a}` in topological order and writes the difference.

</option>

<option id="dred-and-counts" outcome="rejected">

DRed and `u32` path counts (as before): budget blow-ups on deletes and a silent overflow.

</option>

<option id="u64-counts" outcome="rejected">

`u64` saturating counts: overflow becomes rare, but a saturated count can never be decremented correctly.

</option>

## Decision Outcome

Chosen option: "bf-and-set-closure", because both remove a failure mode (budget blow-up, overflow) at a known, bounded cost: edge removal costs O(|desc(a)| · depth) reads.

### Consequences

<consequence kind="good">

No arithmetic overflow; deletes touch far fewer derived facts; `in_star` values are empty.

</consequence>

<consequence kind="bad">

Backward/Forward is more complex to implement than DRed. Edge removal in `in_star` reads parent closures instead of subtracting counts.

</consequence>

## More Information

Amends [ADR-0009](0009-datalog-maintenance-policy.md) and [ADR-0008](0008-graph-nesting-dag.md).

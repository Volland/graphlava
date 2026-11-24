---
status: accepted
date: 2026-11-24
decision-makers: [Volodia]
---
# Materialized Datalog rules are opt-in, budgeted, and stored apart from base data

## Context and Problem Statement

Agent-memory writes are mostly value overwrites, each a delete plus an insert in the single writer transaction. Incremental maintenance (counting, DRed) at commit can touch a large part of a derived closure for one overwrite and stall the writer.

## Considered Options

<option id="budgeted-opt-in" outcome="chosen">

- Untagged rules are `@on_demand` (magic sets, semi-naive).
- `@materialized` is accepted only over stable predicates (node-valued edge predicates; never inline properties or predicates with a high measured overwrite rate). Hot-predicate rules are rejected unless `@materialized(force)`.
- Non-recursive rules: counting maintenance at commit. Recursive rules: DRed with a per-transaction budget on touched derived facts. Over budget, the rule is marked `stale`, reads fall back to on-demand, and a background rebuild restores it.
- Derived facts live in a separate derived table, not in the six orderings, and have no statement ID until annotated (lazy).

</option>

<option id="always-sync" outcome="rejected">

Maintain every `@materialized` rule synchronously without a budget: simplest semantics, but one overwrite can stall the writer.

</option>

## Decision Outcome

Chosen option: "budgeted-opt-in", because it bounds writer latency and keeps base indexes and the ID space clean.

### Consequences

<consequence kind="good">

Bounded commit latency; base orderings never hold derived data; stale rules degrade to correct but slower answers.

</consequence>

<consequence kind="bad">

Planner must union base and derived sources and track per-rule staleness. Hot-predicate detection needs overwrite-rate statistics. A rebuilder needs scheduling on mobile. Built-in rule sets (RDFS/OWL-RL) must be declared with the same policy.

</consequence>

## More Information

Resolves handover open question 8. Refines handover D10 and sections 6.6.

Amended by [ADR-0029](0029-incremental-maintenance-algorithms.md) (2026-11-24): recursive materialised rules use Backward/Forward maintenance instead of DRed; the budget and stale fallback are unchanged.

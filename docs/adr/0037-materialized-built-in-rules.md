---
status: accepted
date: 2026-11-24
decision-makers: [Volodia]
---
# Built-in rules such as in_star are materialized in v0

## Context and Problem Statement

v0 allows only on-demand user rules ([ADR-0009](0009-datalog-maintenance-policy.md)), but scope expansion needs `in_star` on every scoped query.

## Considered Options

<option id="exempt-built-ins" outcome="chosen">

Built-in rules (`in_star`) are materialized in v0 with hand-written maintenance (the set-valued `in_star` of [ADR-0029](0029-incremental-maintenance-algorithms.md)); user `@materialized` rules wait for v1.

</option>

<option id="on-demand" outcome="rejected">

Compute `in_star` on demand: every scoped query pays a closure.

</option>

## Decision Outcome

Chosen option: "exempt-built-ins", because scoped queries are the common case and the nesting DAG changes rarely.

### Consequences

<consequence kind="good">

Scope expansion is a lookup.

</consequence>

<consequence kind="bad">

Hand-written maintenance code that must be proved or property-tested against the closure (`Nesting.scope_trans`).

</consequence>

## More Information

Resolves open question "Built-in materialized rules in v0".

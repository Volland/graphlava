---
status: accepted
date: 2026-11-24
decision-makers: [Volodia]
---
# One statement per (s,p,o) by default; parallel statements are per-predicate opt-in

## Context and Problem Statement

The draft put the statement ID as the 4th key component of all six orderings so parallel `(s,p,o)` statements (Cypher multi-edges) could exist. That widens every key, adds a duplicate level to Leapfrog Triejoin tries, and serves a case agent-memory workloads rarely have.

## Considered Options

<option id="unique-default" outcome="chosen">

Ordering entry is `(a,b,c) -> id`. Predicates declared `multi` in the meta-model use `(a,b,c,id)` keys. Repeated `CREATE` on a non-multi predicate returns the existing ID (MERGE-like).

</option>

<option id="parallel-everywhere" outcome="rejected">

`id` in every key: uniform and Cypher-faithful, but larger keys, dedupe in every join, higher write cost.

</option>

## Decision Outcome

Chosen option: "unique-default", because it keeps the hot path narrow and join-friendly and pays for multi-edges only where declared.

### Consequences

<consequence kind="good">

Narrower keys, no dedupe step in joins, ID is a value rather than part of the scan order.

</consequence>

<consequence kind="bad">

Two key shapes to implement and test. Cypher `CREATE` deviates on non-multi predicates (documented in the Cypher spec). A predicate switched to `multi` needs a rewrite of its entries.

</consequence>

## More Information

Resolves handover open question 4. Amends handover D8 and section 3.3.

Amended by [ADR-0024](0024-quad-semantics-membership.md) and [ADR-0028](0028-functional-predicates-literal-roles.md) (2026-11-24): inserting an existing triple into another graph adds a membership; uniqueness is per store; predicates may also be declared `functional`.

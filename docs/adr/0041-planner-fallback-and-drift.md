---
status: accepted
date: 2026-11-24
decision-makers: [Volodia]
---
# Missing orderings fall back; plans are invalidated at 4x drift

## Context and Problem Statement

Only SPO, POS and OSP are always present ([ADR-0015](0015-three-base-orderings.md)); a cyclic query may want another. Cached plans need an invalidation rule.

## Considered Options

<option id="fallback-and-factor" outcome="chosen">

When the best variable order needs a missing ordering, run with the best available plan, count the miss, and build the ordering in the background when misses repeat. A cached plan is invalidated, and statistics rebuilt, when a cardinality it used drifts by a factor (default 4, tunable).

</option>

<option id="sync-build" outcome="rejected">

Build the ordering synchronously: one query can stall the writer for minutes.

</option>

<option id="timer" outcome="rejected">

Invalidate plans on a timer: ignores actual data change.

</option>

## Decision Outcome

Chosen option: "fallback-and-factor", because queries never stall the writer and plans follow the data.

### Consequences

<consequence kind="good">

Predictable latency; the store adapts to the query mix.

</consequence>

<consequence kind="bad">

A background builder and miss counters to maintain; the factor needs tuning.

</consequence>

## More Information

Resolves open questions "Missing ordering for cyclic patterns" and "Statistics drift threshold".

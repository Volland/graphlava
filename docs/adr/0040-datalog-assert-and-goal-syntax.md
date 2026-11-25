---
status: accepted
date: 2026-11-24
decision-makers: [Volodia]
---
# Datalog asserts base facts with @assert and accepts ?- goals

## Context and Problem Statement

The standalone Datalog surface ([ADR-0009](0009-datalog-maintenance-policy.md)) needed a rule for writing base facts and a query syntax.

## Considered Options

<option id="assert-and-goal" outcome="chosen">

Facts and rule heads marked `@assert` write base data inside a write transaction; unmarked heads stay derived. Programs accept Nemo `@output` and a `?- goal.` shorthand that compiles to it.

</option>

<option id="read-only" outcome="rejected">

Datalog is read-only: writes only through SPARQL Update or the API.

</option>

<option id="nemo-only" outcome="rejected">

Only Nemo syntax: less convenient for interactive queries.

</option>

## Decision Outcome

Chosen option: "assert-and-goal", because agents write and query memory from one surface, and Nemo programs still run unchanged.

### Consequences

<consequence kind="good">

Datalog is a complete surface for agents.

</consequence>

<consequence kind="bad">

`@assert` programs need a write transaction and the same guards as other writes.

</consequence>

## More Information

Resolves open questions "Datalog writes to base data" and "Datalog query syntax".

---
status: accepted
date: 2026-11-24
decision-makers: [Volodia]
---
# SPARQL 1.2 pin, spargebra parser and the default write graph

## Context and Problem Statement

The SPARQL surface ([ADR-0014](0014-sparql12-scope-and-service.md)) needed a pinned spec version, a parser and a target graph for writes without `GRAPH`.

## Considered Options

<option id="pin-spargebra-default-graph" outcome="chosen">

Pin the latest dated W3C TR snapshot of SPARQL 1.2 / RDF 1.2 at the start of the SPARQL work and record its URL and date in [[11-sparql#Conformance]]; re-pin only at milestones and move to the Candidate Recommendation when one is published (on 2026-11-24 the latest was the Working Draft of 2026-06-25). Parse with `spargebra` (Oxigraph), extended for `GRAPH!` and the in-place update; contribute RDF 1.2 support upstream where missing. `INSERT DATA` without `GRAPH` writes to a fixed default graph created with the file; a transaction option can override it.

</option>

<option id="track-editors-draft" outcome="rejected">

Track the editor's draft: constant churn.

</option>

<option id="own-parser" outcome="rejected">

Write an own parser: full control, large effort.

</option>

<option id="system-graph" outcome="rejected">

Write to the system graph: mixes user data with the meta-model.

</option>

## Decision Outcome

Chosen option: "pin-spargebra-default-graph", because it bounds churn, reuses a maintained parser and keeps user data out of the meta-model.

### Consequences

<consequence kind="good">

A fixed test target; less parser code to own.

</consequence>

<consequence kind="bad">

Extensions such as `GRAPH!` need a fork or a pre-pass of spargebra until upstream accepts them; no CR exists yet, so the pin is a Working Draft.

</consequence>

## More Information

Resolves open questions "SPARQL 1.2 draft pin", "SPARQL parser reuse" and "Default write graph".

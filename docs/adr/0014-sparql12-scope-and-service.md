---
status: accepted
date: 2026-11-24
decision-makers: [Volodia]
---
# SPARQL 1.2 and Datalog are both first-class surfaces; `SERVICE` embeds Datalog in SPARQL; a network service follows in v1

## Context and Problem Statement

SPARQL 1.2 is the first text surface ([ADR-0013](0013-build-order-sparql-datalog-first.md)). Its scope, its conformance targets, and the way Datalog is reached from SPARQL need fixing. The owner also wants meta-core to run as a service with Datalog available.

## Considered Options

<option id="service-both" outcome="chosen">

**v0 scope**
- Query: `SELECT`, `ASK`, `CONSTRUCT`; BGP, `OPTIONAL`, `UNION`, `MINUS`, `FILTER`, `BIND`, `VALUES`, subqueries, aggregates, property paths, `GRAPH` (transitive default, [ADR-0007](0007-graph-scope-transitive-default.md); `GRAPH!` for direct), `ORDER BY`, `LIMIT`, `OFFSET`.
- RDF 1.2: reifiers, triple terms, `{| ... |}` annotations, in-place update extension (`DELETE` + `INSERT` with the same reifier keeps the ID).
- Update: `INSERT DATA`, `DELETE DATA`, `DELETE/INSERT ... WHERE`, `CREATE`, `DROP`, `CLEAR`.
- **Datalog as a standalone language:** Datalog programs (rules, facts, queries) run directly through their own parser entry point, without SPARQL, over the same IR, planner, plan cache and transaction API. It is a peer of SPARQL, not a sub-feature.
- **Datalog inside SPARQL via `SERVICE`:** `SERVICE <meta:datalog> { ... }` evaluates a named or inline rule through the shared IR. Remote `SERVICE` goes through a `ServiceResolver` trait; the v0 resolver knows only the local Datalog service.
- Deferred: `DESCRIBE`, remote `LOAD`, full-text and geo functions.

**v1**
- `meta-core-server` crate: SPARQL Protocol (query and update) plus a standalone Datalog endpoint, reusing the owner-process IPC; HTTP dependencies stay out of the embedded core.
- Remote `SERVICE`: HTTP resolver, bind-join with batching, timeouts, and a rule for remote calls inside a read transaction.

**Conformance**
- W3C SPARQL 1.1 test suite in `direct` mode (transitive `GRAPH` off); expected skips are listed with reasons.
- W3C RDF 1.2 / SPARQL 1.2 tests against a named draft version recorded in the spec.
- Own tests for the deviations: transitive `GRAPH`, `GRAPH!`, same-reifier update, unique `(s,p,o)`.

</option>

<option id="call-extension" outcome="rejected">

A custom `CALL` clause for Datalog: not standard syntax; parsers and tools reject it.

</option>

<option id="embedded-only" outcome="rejected">

No network service: smaller surface, but no SPARQL Protocol clients and no IPC for other processes ([ADR-0002](0002-redb-single-owner-process.md)).

</option>

## Decision Outcome

Chosen option: "service-both", because Datalog is a full language of its own for rule-heavy work, `SERVICE` is standard syntax that gives Datalog a first-class place in SPARQL, and the network service reuses the owner-process boundary without burdening the embedded core.

### Consequences

<consequence kind="good">

Datalog is reachable from any SPARQL client; federation hooks exist from day one; the embedded build stays small.

</consequence>

<consequence kind="bad">

`SERVICE <meta:datalog>` is a reserved IRI whose semantics must be specified (variable binding, dataset, errors). The planner must treat a Datalog service as a local operator, not a remote call. SPARQL 1.2 is a moving target, so the tracked draft version must be pinned.

</consequence>

## More Information

Refines handover sections 6.2 and 6.4. Amended 2026-11-24: Datalog is a standalone surface in v0, not only reachable through `SERVICE`.

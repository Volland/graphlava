---
status: accepted
date: 2026-11-24
decision-makers: [Volodia]
---
# Build order: core + Datalog IR + SPARQL 1.2 first; Cypher later

## Context and Problem Statement

The handover scopes SPARQL 1.2, Cypher/GQL, Datalog, a vector index, four metagraph kinds, two encodings and five platforms. Specs and implementation need an order. The owner wants SPARQL and Datalog as the focus.

## Considered Options

<option id="sparql-datalog-first" outcome="chosen">

- **v0:** `Storage` trait on redb, ID encoding, dictionary, `stmt`, `props`, six orderings (v1 multimap encoding), graphs (chunks, `in_star`), hyperedges with incidence, cascade delete, Datalog IR with Rust API, SPARQL 1.2 as the first text surface, Leapfrog Triejoin, simple planner, `@on_demand` rules only.
- **v1:** Cypher/GQL subset, `@materialized` rules (counting, DRed, stale fallback), IVF-PQ vector index, plan cache, Swift/Kotlin bindings, mobile benchmarks.
- **v2:** block-compressed encoding with SIMD decode, partitioned-graph auto-promotion, hyperedge star-pattern collapse, platform hardening.
- All specs are written now: v0 at byte/algorithm depth; v1 and v2 at interface and open-question depth. The engine bake-off spike gates v0.

</option>

<option id="cypher-first" outcome="rejected">

Cypher as first surface fits agent memory naturally, but the owner prioritises SPARQL and Datalog, and the RDF-based model (reifiers, named graphs) maps to SPARQL directly.

</option>

<option id="everything-v0" outcome="rejected">

All surfaces and features in v0: no usable milestone for a long time and high rework risk.

</option>

## Decision Outcome

Chosen option: "sparql-datalog-first", because it follows the owner's priority and the data model, and keeps v0 testable end to end.

### Consequences

<consequence kind="good">

SPARQL conformance suites give an early correctness target. Cypher-specific features (multi-edges, edge isomorphism) can wait, so [ADR-0004](0004-unique-statements-multi-opt-in.md) multi predicates are low priority in v0.

</consequence>

<consequence kind="bad">

SPARQL 1.2 is still evolving and needs a tracked target version. Cypher-driven IR features (edge-isomorphism mode, bag/set flags) must still be designed into the IR so that v1 doesn't force a rewrite.

</consequence>

## More Information

Refines handover D1 and section 12 deliverable `18-roadmap.md`.

Amended by [ADR-0029](0029-incremental-maintenance-algorithms.md) and [ADR-0030](0030-vector-index-scoped-rabitq.md) (2026-11-24): v1 maintenance uses Backward/Forward instead of DRed; the v1 vector index is IVF with RaBitQ codes and scope-aware exact search instead of IVF-PQ.

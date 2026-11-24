# Design

## Context

The full design lives in `tg.md/` (specs 00–21) and the decisions in `docs/adr/` (0002–0041). This slice of the former `v0-core` change summarises only what shapes its own work.

## Decisions

| Decision | ADR |
| --- | --- |
| Build order: core + Datalog IR + SPARQL first | [[adr:0013]] |
| SPARQL 1.2 query + update, `SERVICE <meta:datalog>` | [[adr:0014]] |
| Join kernels: Leapfrog Triejoin, index nested-loop, BFS paths | [[adr:0027]] |
| Reifier IRIs `https://graphlava.org/ns/stmt/{id}`, namespace `https://graphlava.org/ns#` | [[adr:0031]] |
| One reifier per statement, aliases, `unasserted` statements | [[adr:0032]] |
| Pinned dated TR snapshot, `spargebra`, fixed default write graph | [[adr:0038]] |
| Per-node predicate summary for `?p` | [[adr:0039]] |
| `@assert` and `?- goal.` | [[adr:0040]] |
| Missing-ordering fallback, 4x drift invalidation | [[adr:0041]] |

## Formal model

Task 11.7 adds `MetaCore.Datalog` and proves PO-STRAT and PO-SEMINAIVE. SPARQL has no L0 model; it is covered by the W3C suites (L5) and differential tests against Oxigraph.

## Risks / Trade-offs

- [SPARQL 1.2 is still a Working Draft] → pinned dated snapshot, re-pinned at milestones ([[adr:0038]]).
- [spargebra lacks `GRAPH!`] → pre-pass or fork until upstream accepts it.

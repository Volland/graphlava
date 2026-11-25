# Design

## Context

The full design lives in `tg.md/` (specs 00–21) and the decisions in `docs/adr/` (0002–0041). This slice of the former `v0-core` change summarises only what shapes its own work.

## Decisions

| Decision | ADR |
| --- | --- |
| Graph scoping: range-driven for small graphs, partitioned for large | [[adr:0003]] |
| `GRAPH` transitive by default, `GRAPH!` direct | [[adr:0007]] |
| Nesting is a DAG with cycle rejection and depth 64 | [[adr:0008]] |
| Chunks per `(graph, kind)` | [[adr:0010]] |
| Lazy membership IDs | [[adr:0017]] |
| Quad semantics: membership operations, `moved` tombstones, drop keeps shared items | [[adr:0024]] |
| Literal hyperedge values are inline properties of the hyperedge | [[adr:0028]] |
| Set-valued `in_star` with ancestor recomputation on edge removal | [[adr:0029]] |
| Built-in `in_star` is materialized in v0 | [[adr:0037]] |
| Hyperedge roles block delete unless detach | [[adr:0006]] |

## Formal model

`MetaCore.Nesting` already proves `addEdge_acyclic` and `scope_trans`. Task 11.6 adds `MetaCore.Hyperedge` (arity, role validation, delete guard) and proves the guard keeps `RefClosed` for roles. The oracle gains graph and hyperedge operations so the replay of `v0-store-core` covers them (v0.1 in the design table).

## Risks / Trade-offs

- [Ancestor recomputation on edge removal is expensive on deep lattices] → tested on 40 stacked diamonds (task 15.9).
- [Large drops on mobile] → gate v1 ([[open-questions#Large drops on mobile]]).

# Proposal

## Why

Nested named graphs and n-ary hyperedges are what make graphlava a metagraph store and not a triple store. This second slice of the former `v0-core` change builds them on the store core of `v0-store-core`.

## What Changes

- Chunk membership and `gmem`, transitive and direct scope with the set-valued, materialized `in_star` ([[adr:0007]], [[adr:0029]], [[adr:0037]]), DAG nesting with cycle and depth checks ([[adr:0008]]), lazy membership IDs ([[adr:0017]]), membership operations with `moved` tombstones and drop that keeps shared items ([[adr:0024]]).
- Hyperedges as records with type validation, an incidence index, virtual role triples, per-(graph, type) chunks, role promotion, literal attributes as inline properties ([[adr:0010]], [[adr:0028]]), and the role guard on delete ([[adr:0006]]).

## Capabilities

### New Capabilities

- `named-graphs`: graph membership, transitive and direct scope, DAG nesting and graph drop.
- `hyperedges`: typed n-ary hyperedges, their roles, their graph membership and the delete guard.

### Modified Capabilities

None.

## Impact

- **Code:** `crates/graphlava` graphs and hyperedge modules.
- **lat.md sections:** [[08-hyperedges]], [[09-graphs]].
- **File format:** adds `gmem`, `in_star`, incidence and hyperedge tables before the freeze.
- **Verification:** named-graphs L0 (`MetaCore.Nesting.addEdge_acyclic`, `scope_trans`), L2, L3; hyperedges L0 (new `MetaCore.Hyperedge`, task 11.6), L2, L3.

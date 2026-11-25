# Tasks

Numbers in parentheses are the task ids of the former `v0-core` change.

## 1. Formal model

- [x] 1.1 Model hyperedges (`MetaCore.Hyperedge`: arity, role validation, delete guard) and prove the guard keeps `RefClosed` for roles (11.6)
- [x] 1.2 Extend the oracle and the refinement replay with graph and hyperedge operations

## 2. named-graphs

- [x] 2.1 Implement chunk membership and `gmem`; annotate `[[openspec:named-graphs#Home graph membership]]` (6.1)
- [x] 2.2 Implement membership operations, `moved` tombstones and per-store uniqueness ([[adr:0024]]); annotate `[[openspec:named-graphs#One statement in several graphs]]` (15.4)
- [x] 2.3 Implement scope expansion (transitive and direct) with the set-valued, materialized `in_star` and ancestor recomputation on edge removal; test a lattice of 40 stacked diamonds; annotate `[[openspec:named-graphs#Transitive graph scope by default]]` (6.2, 15.9)
- [x] 2.4 Implement cycle and depth checks; annotate `[[openspec:named-graphs#Acyclic nesting]]` (6.3)
- [x] 2.5 Implement lazy membership IDs; annotate `[[openspec:named-graphs#Annotatable membership]]` (6.4)
- [x] 2.6 Implement plain and recursive drop that keeps shared items; annotate `[[openspec:named-graphs#Graph drop]]` (6.5, 15.4)
- [x] 2.7 Write a test per scenario in `specs/named-graphs/spec.md`, each annotated with its `@tg: verifies::` id (6.6, 15.12)

## 3. hyperedges

- [x] 3.1 Implement records and type validation; annotate `[[openspec:hyperedges#Typed hyperedges with roles]]` (7.1)
- [x] 3.2 Implement literal attributes as inline properties of the hyperedge; annotate `[[openspec:hyperedges#Literal attributes of hyperedges]]` (15.8)
- [x] 3.3 Implement the incidence index and virtual role triples; annotate `[[openspec:hyperedges#Role triples are queryable]]` (7.2)
- [x] 3.4 Implement per-(graph, type) chunks and type scans; annotate `[[openspec:hyperedges#Hyperedges in graphs and type scans]]` (7.3)
- [x] 3.5 Implement role promotion; annotate `[[openspec:hyperedges#Annotating a role]]` (7.4)
- [x] 3.6 Implement the role guard on delete and detach; annotate `[[openspec:hyperedges#Hyperedge roles block delete]]` (4.5)
- [x] 3.7 Write a test per scenario in `specs/hyperedges/spec.md`, each annotated with its `@tg: verifies::` id (7.5, 15.12)

## 4. Documentation and checks

- [x] 4.1 Add `openspec:` frontmatter: named-graphs → `09-graphs`; hyperedges → `08-hyperedges` (14.2)
- [x] 4.2 Run the full check list and `tg trace named-graphs hyperedges --gaps`, and close every gap (14.3)

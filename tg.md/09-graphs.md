---
type: Requirement
openspec: [named-graphs]
status: draft
priority: must
specs: ["adr:0003", "adr:0007", "adr:0008", "adr:0010", "adr:0017", "adr:0024", "adr:0029"]
---
# Named and nested graphs

Graphs as nodes: membership through ID chunks and extra memberships, transitive scope over a nesting DAG, scope expansion in the planner, partitioned graphs, graph drop and per-graph statistics.

- follows_from:: [[adr:0003]]
- follows_from:: [[adr:0007]]
- follows_from:: [[adr:0008]]
- follows_from:: [[adr:0017]]
- follows_from:: [[adr:0024]]
- follows_from:: [[adr:0029]]
- depends_on:: [[02-id-encoding#Allocation]] {strength: hard}

## Purpose

Agent memory has many small graphs (episodes, documents) and a few large ones (agents, tenants). Scoped queries must cost in proportion to the graph, not the database, and nesting must not make every query a union.

## Chunk membership

Every statement and hyperedge has one home graph, fixed by the chunk its ID was allocated from. Primary membership costs zero bytes per item ([[adr:0010]]).

| Table | Key | Value |
| --- | --- | --- |
| `gchunk` | `chunk_no: u64` | `(graph_id: u64, kind: u64)` |
| `kind_chunk` | `(kind, chunk_no)` | none |
| `graph_chunks` | `(graph_id, kind, chunk_no)` | none |

<rule id="gr-home-graph" strength="must">

The home graph of item X is `gchunk[chunk(X)].graph_id`. It is set at allocation and never changes, even when X is later moved: a move is an extra membership plus a `moved` tombstone on the home membership ([[09-graphs#Membership operations]]).

</rule>

<rule id="gr-graph-ranges" strength="must">

The direct items of graph g are the union of ID ranges `[chunk << 20, (chunk + 1) << 20)` over `graph_chunks` prefix `(g)`, plus extra members from `gmem`, minus items with a `moved` tombstone under g. The ranges are kept as a set of chunk numbers, so testing an ID is one lookup of `id >> 20`; extras and tombstones are a small roaring bitmap.

</rule>

<rule id="gr-default-graph" strength="must">

Items written without a graph go to the default graph, a fixed graph created with the file next to the system graph. SPARQL's default graph maps to it.

</rule>

## Extra membership

An item can belong to graphs other than its home graph. Each extra membership is one entry in `gmem` and one in `gmem_rev`, exposed as a virtual `:in` triple.

| Table | Key | Value |
| --- | --- | --- |
| `gmem` | `(g, item)` | `flags: u8` (bit 0 `home`, bit 1 `moved`), promoted membership statement ID `u64` or 0 |
| `gmem_rev` | `(item, g)` | none |

<rule id="gr-extra-pair" strength="must">

`gmem` and `gmem_rev` hold the same pairs and change in the same transaction.

</rule>

<rule id="gr-membership-virtual" strength="must">

`(X, :in, g)` is a virtual triple: true when g is X's home graph and `gmem(g, X)` is not a `moved` tombstone, or when `gmem(g, X)` is an extra membership. It has no statement ID until annotated ([[adr:0017]]).

</rule>

<rule id="gr-membership-promote" strength="must">

Annotating a membership allocates a statement ID in a statement chunk of X's home graph. For an extra membership the ID is stored as the `gmem` value; for home membership it is stored in `gmem` under `(home, X)` with the `home` flag set, so the pointer has one place to live. Deleting the membership deletes the promoted statement and cascades ([[06-transactions-durability#Cascade delete]]).

</rule>

## Membership operations

How inserts, deletes and moves act on an item that can be in several graphs: one statement, many memberships, dataset semantics for every graph clause ([[adr:0024]]).

<rule id="gr-insert-existing" strength="must">

Inserting `(s,p,o)` into graph g when a non-multi statement with that triple already exists in the same store adds a membership of g (an extra membership, or removal of a `moved` tombstone when g is its home) and returns the existing ID. It is a no-op when the statement is already a direct member of g.

</rule>

<rule id="gr-delete-in-graph" strength="must">

Deleting `(s,p,o)` inside a graph clause for g removes only the membership in g. Deleting a statement without a graph clause, or removing its last membership, deletes the statement and cascades ([[06-transactions-durability#Cascade delete]]). Removing a home membership writes a `moved` tombstone.

</rule>

<rule id="gr-move" strength="must">

Moving X from g to h adds a membership of h and removes the membership of g in one transaction. X keeps its ID, its home graph (by chunk) and its annotations.

</rule>

Per-graph provenance (which episode asserted a fact, with what confidence) is written as annotations of the promoted membership `<< X :in g >>`, so one statement can carry different metadata per graph.

## Scope expansion

`GRAPH :g { P }` matches items in g and in every graph nested under g; `GRAPH! :g { P }` matches only direct items ([[adr:0007]]). The planner turns a scope into an ID filter or an ID-range scan.

- depends_on:: [[11-sparql#Graph semantics]] {strength: hard}
- depends_on:: [[14-planner-execution#Join selection]] {strength: hard}

<rule id="gr-scope-transitive" strength="must">

The scope of `GRAPH :g` is the union of the direct items of g and of every graph d with `in_star(d, g)`. An item reachable through several paths is counted once.

</rule>

<rule id="gr-scope-unbound" strength="must">

In `GRAPH ?g { P }`, `?g` binds each direct graph of the matched item (home graph and `gmem` extras), never an ancestor.

</rule>

<rule id="gr-scope-direct-mode" strength="must">

A connection flag `graph_semantics = direct` makes `GRAPH` behave as `GRAPH!`, for the W3C conformance suites ([[11-sparql#Conformance]]).

</rule>

### Plan choice

For each scoped pattern the planner picks one of three access paths from the scope size S (items in scope) and the pattern selectivity, using per-graph statistics.

| Plan | When | Access | Cost |
| --- | --- | --- | --- |
| range-driven | S small relative to the pattern's matches | read `stmt` over the scope's ID ranges, filter by pattern, probe orderings for the rest | O(S) |
| filter-driven | pattern selective, S large | scan the ordering for the pattern; test each statement ID against the scope bitmap | O(matches) |
| partitioned | g is a partitioned graph | scan g's own orderings | O(matches in g) |

<rule id="gr-plan-threshold" strength="should">

The planner chooses range-driven when the estimated S is below the estimated number of pattern matches across the whole database. The crossover constant is to be measured ([[17-benchmarks#Engine bake-off]]). A filter-driven plan tests each statement ID against the chunk set of the scope and then against the extras and tombstones bitmap.

</rule>

## Nesting DAG

Graphs nest through `(child, :in, parent)`. Nesting forms a DAG: several parents are allowed, cycles are rejected, and depth has a soft limit ([[adr:0008]]).

| Table | Key | Value |
| --- | --- | --- |
| `gparent` | `(child, parent)` | none |
| `gchild` | `(parent, child)` | none |
| `in_star` | `(descendant, ancestor)` | none |
| `in_star_rev` | `(ancestor, descendant)` | none |

<rule id="gr-dag-cycle" strength="must">

Adding `(a, :in, b)` fails with `GraphCycle` when `a = b` or `in_star(b, a)` holds. The check is one point read.

</rule>

<rule id="gr-dag-depth" strength="should">

Adding `(a, :in, b)` fails with `GraphDepthExceeded` when the resulting longest path above any descendant of a exceeds the depth limit (default 64, configurable per database).

</rule>

<rule id="gr-in-star-closure" strength="must">

`in_star` holds the transitive closure as a set of pairs, with no path counts ([[adr:0029]]). Adding an edge `(a, b)` inserts `(d, u)` for every d in `desc(a) ∪ {a}` and every u in `anc(b) ∪ {b}`. Removing an edge recomputes, in topological order, `anc(d)` as the union of `anc(p) ∪ {p}` over the remaining parents p of every d in `desc(a) ∪ {a}`, and writes the difference. Cost: O(|desc(a)| · |anc(b)|) to add, O(|desc(a)| · depth) reads to remove.

</rule>

Path counts are avoided because they grow exponentially in a DAG: about 32 stacked diamonds overflow a `u32`, well within the depth limit.

`in_star` is a built-in materialized rule, maintained inside the writer transaction. In v0 user rules are on demand only ([[adr:0013]]); whether built-in rules are exempt is [[open-questions#Built-in materialized rules in v0]].

## Partitioned graphs

A partitioned graph has its own set of tables in the same file: orderings, `stmt`, `props`, `plist`, `hedge`, `incidence`. It gives isolation and fast drop for large graphs ([[adr:0003]]).

<rule id="gr-partition-tables" strength="must">

The tables of a partitioned graph g are named with g's ID as a suffix. Statements whose home graph is g live only there. Global orderings hold no entries for them.

</rule>

<rule id="gr-partition-unique" strength="must">

Uniqueness of `(s,p,o)` holds per store: the global tables, or the tables of one partitioned graph ([[adr:0024]]). An insert probes only the store it writes to, so the same triple may exist once in the global store and once in each partition, with different IDs.

</rule>

<rule id="gr-partition-declare" strength="must">

In v0 and v1 a graph is partitioned only by an explicit declaration at creation. From v2, a graph whose direct statement count passes a configurable threshold is promoted online: its items are copied into new tables in background batches, and a switch-over transaction removes them from the global tables.

</rule>

<rule id="gr-partition-cross" strength="must">

A query whose scope covers a partitioned graph and other graphs unions the partitioned tables with the global tables, deduplicating non-multi statements by `(s,p,o)` with a merge on the shared ordering; a pattern that binds the statement ID yields one row per store. A nested child of a partitioned graph is not partitioned unless it is declared so itself.

</rule>

## Graph drop

`DROP GRAPH g` removes g's direct items and its nesting edges; `DROP RECURSIVE GRAPH g` also drops every descendant not reachable from another surviving parent ([[adr:0008]]).

<rule id="gr-drop-plain" strength="must">

A plain drop removes every membership in g: all `gmem` entries with graph g, and the home membership of every item whose home graph is g. An item that is still a member of another graph survives, with a `moved` tombstone under g; every other item of g is deleted. g's `gparent` and `gchild` edges are deleted with `in_star` maintenance. Children that lose their last parent become roots and keep their items.

</rule>

<rule id="gr-drop-cascade" strength="must">

Deleting g's items cascades to their annotations, also in other graphs, and respects the hyperedge-role block ([[06-transactions-durability#Cascade delete]]). The cascade limit does not apply to the items of g itself, only to annotations outside g.

</rule>

<rule id="gr-drop-recursive" strength="must">

`DROP RECURSIVE` drops g and every descendant d for which all paths to a root pass through g.

</rule>

Dropping g costs O(items in g) tree deletes in the global tables plus one `gmem_rev` probe per item to find survivors; for a partitioned graph with no shared items it is dropping its tables, O(1) in table count plus redb's page freeing (to be measured).

## Statistics

Per graph the engine keeps the direct item count, the scope item count for graphs with descendants, and per-predicate counts, to choose between the plans above ([[14-planner-execution#Statistics]]).

## Failure behaviour

Membership, nesting, `in_star` and drop changes are part of the writer transaction; a crash leaves all or none of them. Online partition promotion (v2) keeps a progress record in `meta` and restarts or rolls back on open.

## Platform notes

No platform-specific behaviour. Large drops run in one write transaction, which on mobile can be long; see [[07-platforms#iOS]].

## Test plan

Property and conformance tests for membership and scope.

- Scope equivalence: for random DAGs up to depth 10, `GRAPH :g` results equal a naive closure computation.
- `in_star` closure: random edge insert and delete sequences keep it equal to a recomputed closure, including a lattice of 40 stacked diamonds.
- Membership operations: inserting an existing triple into a second graph, deleting it in one graph, moving it, and dropping either graph; the statement survives exactly while it has a membership.
- Partitions: the same triple in the global store and in a partition is returned once by an unscoped `SELECT` and with two IDs by a reifier pattern.
- Cycle and depth rejection cases.
- Drop: plain, recursive, shared children, annotations in other graphs, hyperedge block.
- Plan choice: the same query over a 100-statement graph and a 10M-statement graph picks range-driven and filter-driven respectively.
- Benchmark: scoped query latency against graph size and database size, to be measured in [[17-benchmarks#Engine bake-off]].

## Open items

Questions this spec leaves open; each is an `OpenQuestion` in [[open-questions]].

- [[open-questions#Built-in materialized rules in v0]]
- [[open-questions#Partitioned graph per file]]
- [[open-questions#Large drops on mobile]]

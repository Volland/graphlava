---
type: Requirement
openspec: [named-graphs]
status: draft
priority: must
specs: ["adr:0003", "adr:0007", "adr:0008", "adr:0010", "adr:0017"]
---
# Named and nested graphs

Graphs as nodes: membership through ID chunks and extra memberships, transitive scope over a nesting DAG, scope expansion in the planner, partitioned graphs, graph drop and per-graph statistics.

- follows_from:: [[adr:0003]]
- follows_from:: [[adr:0007]]
- follows_from:: [[adr:0008]]
- follows_from:: [[adr:0017]]
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

The home graph of item X is `gchunk[chunk(X)].graph_id`. It is set at allocation and never changes, even when X is later moved ([[open-questions#Moving an item to another graph]]).

</rule>

<rule id="gr-graph-ranges" strength="must">

The direct items of graph g are the union of ID ranges `[chunk << 20, (chunk + 1) << 20)` over `graph_chunks` prefix `(g)`, plus extra members from `gmem`. This set is computed as a roaring bitmap of ranges.

</rule>

<rule id="gr-default-graph" strength="must">

Items written without a graph go to the default graph, a fixed graph created with the file next to the system graph. SPARQL's default graph maps to it.

</rule>

## Extra membership

An item can belong to graphs other than its home graph. Each extra membership is one entry in `gmem` and one in `gmem_rev`, exposed as a virtual `:in` triple.

| Table | Key | Value |
| --- | --- | --- |
| `gmem` | `(g, item)` | promoted statement ID `u64`, or 0 |
| `gmem_rev` | `(item, g)` | none |

<rule id="gr-extra-pair" strength="must">

`gmem` and `gmem_rev` hold the same pairs and change in the same transaction.

</rule>

<rule id="gr-membership-virtual" strength="must">

`(X, :in, g)` is a virtual triple: true when g is X's home graph or `(g, X)` is in `gmem`. It has no statement ID until annotated ([[adr:0017]]).

</rule>

<rule id="gr-membership-promote" strength="must">

Annotating a membership allocates a statement ID in a statement chunk of X's home graph. For an extra membership the ID is stored as the `gmem` value; for home membership it is stored in `gmem` under `(home, X)` with a home flag in the high bit of the value, so the pointer has one place to live. Deleting the membership deletes the promoted statement and cascades ([[06-transactions-durability#Cascade delete]]).

</rule>

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

The planner chooses range-driven when the estimated S is below the estimated number of pattern matches across the whole database. The crossover constant is to be measured ([[17-benchmarks#Engine bake-off]]).

</rule>

## Nesting DAG

Graphs nest through `(child, :in, parent)`. Nesting forms a DAG: several parents are allowed, cycles are rejected, and depth has a soft limit ([[adr:0008]]).

| Table | Key | Value |
| --- | --- | --- |
| `gparent` | `(child, parent)` | none |
| `gchild` | `(parent, child)` | none |
| `in_star` | `(descendant, ancestor)` | path count `u32` |
| `in_star_rev` | `(ancestor, descendant)` | none |

<rule id="gr-dag-cycle" strength="must">

Adding `(a, :in, b)` fails with `GraphCycle` when `a = b` or `in_star(b, a)` holds. The check is one point read.

</rule>

<rule id="gr-dag-depth" strength="should">

Adding `(a, :in, b)` fails with `GraphDepthExceeded` when the resulting longest path above any descendant of a exceeds the depth limit (default 64, configurable per database).

</rule>

<rule id="gr-in-star-counting" strength="must">

`in_star` holds the transitive closure with a path count per pair. Adding an edge `(a, b)` adds, for every descendant d of a (including a) and every ancestor u of b (including b), the product of path counts to `(d, u)`. Removing an edge subtracts the same and deletes pairs that reach 0. Cost: O(|desc(a)| · |anc(b)|) per edge change.

</rule>

`in_star` is a built-in materialized rule, maintained by counting inside the writer transaction. In v0 user rules are on demand only ([[adr:0013]]); whether built-in rules are exempt is [[open-questions#Built-in materialized rules in v0]].

## Partitioned graphs

A partitioned graph has its own set of tables in the same file: orderings, `stmt`, `props`, `hedge`, `incidence`. It gives isolation and fast drop for large graphs ([[adr:0003]]).

<rule id="gr-partition-tables" strength="must">

The tables of a partitioned graph g are named with g's ID as a suffix. Statements whose home graph is g live only there. Global orderings hold no entries for them.

</rule>

<rule id="gr-partition-declare" strength="must">

In v0 and v1 a graph is partitioned only by an explicit declaration at creation. From v2, a graph whose direct statement count passes a configurable threshold is promoted online: its items are copied into new tables in background batches, and a switch-over transaction removes them from the global tables.

</rule>

<rule id="gr-partition-cross" strength="must">

A query whose scope covers a partitioned graph and other graphs unions the partitioned tables with the global tables. A nested child of a partitioned graph is not partitioned unless it is declared so itself.

</rule>

## Graph drop

`DROP GRAPH g` removes g's direct items and its nesting edges; `DROP RECURSIVE GRAPH g` also drops every descendant not reachable from another surviving parent ([[adr:0008]]).

<rule id="gr-drop-plain" strength="must">

A plain drop deletes every item whose home graph is g, removes all `gmem` entries with graph g, and deletes g's `gparent` and `gchild` edges with `in_star` maintenance. Children that lose their last parent become roots and keep their items.

</rule>

<rule id="gr-drop-cascade" strength="must">

Deleting g's items cascades to their annotations, also in other graphs, and respects the hyperedge-role block ([[06-transactions-durability#Cascade delete]]). The cascade limit does not apply to the items of g itself, only to annotations outside g.

</rule>

<rule id="gr-drop-recursive" strength="must">

`DROP RECURSIVE` drops g and every descendant d for which all paths to a root pass through g.

</rule>

Range deletion over g's chunks is O(items in g) tree deletes in the global tables; for a partitioned graph it is dropping its tables, O(1) in table count plus redb's page freeing (to be measured).

## Statistics

Per graph the engine keeps the direct item count, the scope item count for graphs with descendants, and per-predicate counts, to choose between the plans above ([[14-planner-execution#Statistics]]).

## Failure behaviour

Membership, nesting, `in_star` and drop changes are part of the writer transaction; a crash leaves all or none of them. Online partition promotion (v2) keeps a progress record in `meta` and restarts or rolls back on open.

## Platform notes

No platform-specific behaviour. Large drops run in one write transaction, which on mobile can be long; see [[07-platforms#iOS]].

## Test plan

Property and conformance tests for membership and scope.

- Scope equivalence: for random DAGs up to depth 10, `GRAPH :g` results equal a naive closure computation.
- `in_star` counting: random edge insert and delete sequences keep it equal to a recomputed closure.
- Cycle and depth rejection cases.
- Drop: plain, recursive, shared children, annotations in other graphs, hyperedge block.
- Plan choice: the same query over a 100-statement graph and a 10M-statement graph picks range-driven and filter-driven respectively.
- Benchmark: scoped query latency against graph size and database size, to be measured in [[17-benchmarks#Engine bake-off]].

## Open items

Questions this spec leaves open; each is an `OpenQuestion` in [[open-questions]].

- [[open-questions#Moving an item to another graph]]
- [[open-questions#Built-in materialized rules in v0]]
- [[open-questions#Large drops on mobile]]

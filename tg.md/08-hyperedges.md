---
type: Requirement
openspec: [hyperedges]
status: draft
priority: must
specs: ["adr:0005", "adr:0006", "adr:0010", "adr:0015"]
---
# Hyperedges

N-ary relations stored as one record plus an incidence index, exposed to queries as virtual role triples, with promotion of role edges on annotation and star-pattern collapse in the planner.

- follows_from:: [[adr:0010]]
- follows_from:: [[adr:0006]]
- depends_on:: [[01-data-model#Metagraph kinds]] {strength: hard}
- depends_on:: [[02-id-encoding#Allocation]] {strength: hard}

## Purpose

A meeting with an agent, a target and a time is one fact with three roles. Storing it as three statements costs three times three orderings; a hyperedge stores it once and indexes each role filler.

## Hyperedge types

Every hyperedge has a type declared in the meta-model with its roles. The type fixes the arity when all roles are single-valued, which enables the fixed-width record.

- depends_on:: [[01-data-model#Meta-model]] {strength: hard}

<rule id="he-type-declared" strength="must">

A hyperedge type T is declared in the system graph with an ordered list of roles. Each role has an IRI, a cardinality (`one` or `many`) and an optional node kind. Writing a hyperedge whose roles do not match its type fails with `HyperedgeShapeMismatch`.

</rule>

<rule id="he-type-fixed" strength="must">

T is fixed-arity when every role has cardinality `one`. Fixed-arity records omit role IDs; the slot position is the role's index in the declaration.

</rule>

<rule id="he-type-change" strength="should">

Adding an optional role to T is allowed and rewrites no records (the missing slot reads as null). Removing a role or changing its cardinality requires a rewrite of all records of T, run as an online migration.

</rule>

## Record format

One `hedge` entry per hyperedge, keyed by its ID. Hyperedge IDs come from chunks leased per (home graph, type), so all hyperedges of a type are a small set of ID ranges ([[adr:0010]]).

| Table | Key | Value |
| --- | --- | --- |
| `hedge` | `e: u64` (big-endian) | record below |

Fixed-arity record (k roles):

| Offset | Size | Field |
| --- | --- | --- |
| 0 | 1 | format version, `1` |
| 1 | 1 | flags: bit 0 fixed-arity, bit 1 has promoted roles |
| 2 | 2 | reserved, zero |
| 4 | 4 | k, role count (u32 LE) |
| 8 | 8 | type ID T (u64 LE) |
| 16 | 8·k | node IDs in declared role order (u64 LE, 0 = null) |

Variable record (roles with cardinality `many`):

| Offset | Size | Field |
| --- | --- | --- |
| 0..16 | 16 | as above, flags bit 0 clear, k = number of (role, node) pairs |
| 16 | 16·k | pairs `(role ID, node ID)`, sorted by role then node (u64 LE each) |

<rule id="he-record-type" strength="must">

The type stored in the record equals the kind of the chunk that the hyperedge ID belongs to. A reader may take the type from `gchunk` without decoding the record.

</rule>

<rule id="he-no-orderings" strength="must">

Role fillers of a hyperedge are not stored in the SPO, POS or OSP orderings. A k-role hyperedge write touches `hedge` once and `incidence` k times.

</rule>

## Incidence index

`incidence` answers "which hyperedges does node n fill, in which role and type" by prefix scan. It is the only reverse index for hyperedges.

| Table | Key (32 bytes, big-endian fields) | Value |
| --- | --- | --- |
| `incidence` | `(node, role, type, e)` | none |

<rule id="he-incidence-complete" strength="must">

For every hyperedge e of type T and every non-null filler n of role r, `incidence` contains exactly one key `(n, r, T, e)`, and no other keys exist. Writes to `hedge` and `incidence` happen in the same transaction.

</rule>

| Lookup | Access | Cost |
| --- | --- | --- |
| hyperedge by ID | `hedge` point read | O(log n) |
| all hyperedges of type T | `kind_chunk` ranges over `hedge` | O(log n + result) |
| hyperedges where n fills any role | `incidence` prefix `(n)` | O(log n + result) |
| hyperedges where n fills role r of type T | `incidence` prefix `(n, r, T)` | O(log n + result) |

### Update in place

Changing one role filler keeps the hyperedge ID: the record slot is rewritten, one `incidence` key is deleted and one inserted. Annotations on e stay attached ([[adr:0006]]).

## Role triples

Queries see a hyperedge as triples `(e, rdf:type, T)` and `(e, r, n)` for each filled role. These triples are virtual: they are produced from `hedge` and `incidence`, never stored.

- depends_on:: [[10-ir#Virtual relations]] {strength: hard}

<rule id="he-virtual-role" strength="must">

The IR relation `role(e, r, n)` and `hedge_T(e, n1..nk)` for each fixed-arity type T are virtual relations. A pattern `?e :agent ?a` with `?e` unbound is answered from `incidence`; with `?e` bound, from `hedge`.

</rule>

<rule id="he-role-no-id" strength="must">

A role triple has no statement ID until it is annotated. SPARQL reifier patterns over a role triple (`<< ?e :agent ?a ~ ?r >>`) bind `?r` only for promoted role edges.

</rule>

### Promotion of role edges

Annotating a role triple gives it a statement ID, in a statement chunk of the hyperedge's home graph. The record sets flag bit 1, and `hedge_promo` stores the pointer.

| Table | Key | Value |
| --- | --- | --- |
| `hedge_promo` | `(e, role, node)` | statement ID `u64` |

<rule id="he-promote" strength="must">

A promoted role edge stays served from `hedge` and `incidence`; its statement ID lives in `hedge_promo` and in the `stmt` record, and it is not added to the orderings. Changing the role's filler keeps the statement ID and moves the `hedge_promo` key. No demotion ([[adr:0005]]).

</rule>

## Star-pattern collapse

From v2 the planner rewrites a star of role patterns on one variable into one scan of `hedge_T`. In v0 the same pattern runs as a join of virtual role relations.

- depends_on:: [[14-planner-execution#Join selection]] {strength: soft}

<example>

<input>

`?e a :Meeting ; :agent ?a ; :target ?t ; :when ?w`

</input>

<output>

One range scan of `hedge` over the `:Meeting` chunks producing `(e, a, t, w)` rows; no join. With `?a` bound, one `incidence` prefix scan on `(a, :agent, :Meeting)` and one `hedge` point read per result.

</output>

</example>

<rule id="he-star-collapse" strength="should">

When every pattern in a star on `?e` names a role of the same fixed-arity type T (or `?e a T` is present), the v2 planner replaces the star by `hedge_T`. Patterns over roles outside T stay as joins.

</rule>

## Deletion

Deleting a hyperedge removes its record, its incidence keys and its `hedge_promo` entries, then cascades to annotations of e and of its promoted role edges ([[06-transactions-durability#Cascade delete]]).

<rule id="he-delete-filler" strength="must">

Deleting a node or statement that fills a role fails unless `DETACH` is given; with `DETACH` the whole hyperedge is deleted. A filler is never set to null implicitly.

</rule>

## Statistics

Per hyperedge type the engine keeps the count, and per role the number of distinct fillers and the fill rate, as characteristic sets for cardinality estimation ([[14-planner-execution#Statistics]]).

## Failure behaviour

All hyperedge writes are inside one write transaction, so a crash leaves either the whole hyperedge (record, incidence, promotion entries) or none of it.

## Platform notes

The record is little-endian and the incidence key big-endian, like every other table ([[04-physical-layout#Key byte order]]). No platform-specific behaviour.

## Test plan

Property tests and benchmarks for the record and index invariants.

- Invariant check `he-incidence-complete` after random insert, update, delete sequences (10^6 operations).
- Shape validation: every mismatch case of `he-type-declared`.
- Promotion: annotate a role edge, change its filler, check the ID is kept and annotations follow.
- Delete with and without `DETACH`.
- Benchmark: write cost of a k-role hyperedge versus k statements, and star queries with and without collapse, to be measured in [[17-benchmarks#Engine bake-off]].

## Open items

Questions this spec leaves open; each is an `OpenQuestion` in [[open-questions]].

- [[open-questions#Hyperedge role fillers that are literals]]
- [[open-questions#Moving an item to another graph]]

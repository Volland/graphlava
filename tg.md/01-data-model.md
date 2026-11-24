---
type: Requirement
openspec: [statement-store]
status: draft
priority: must
specs: ["adr:0004", "adr:0005", "adr:0006", "adr:0007", "adr:0008", "adr:0009", "adr:0010", "adr:0011", "adr:0017"]
---
# Data model

The logical model of meta-core: terms, statements with stable IDs, inline properties and their promotion, annotations, and how the four metagraph kinds and the meta-model map onto it.

- follows_from:: [[adr:0004]]
- follows_from:: [[adr:0005]]
- follows_from:: [[adr:0017]]
- depends_on:: [[02-id-encoding#Bit layout]] {strength: hard}

## Purpose

This spec fixes what a query or API caller can observe, independent of storage. Physical tables are in [[04-physical-layout]]; ID bits in [[02-id-encoding]]. The model is RDF 1.2 compatible where it can be, with deviations listed here.

## Terms

A term is anything that can appear in a statement position. Every term is one 64-bit ID whose top 4 bits give its kind; there is one ID space for all kinds.

- depends_on:: [[02-id-encoding#Bit layout]] {strength: hard}

| Kind | Can be subject | Can be predicate | Can be object | Stored where as object |
| --- | --- | --- | --- | --- |
| IRI | yes | yes | yes | orderings |
| blank node | yes | no | yes | orderings |
| statement | yes | no | yes | orderings |
| graph | yes | no | yes | orderings |
| hyperedge | yes | no | yes | orderings |
| inline literal | no | no | yes | inline property |
| dictionary literal | no | no | yes | inline property |

<rule id="dm-term-positions" strength="must">

A predicate is always an IRI. A literal is never a subject. Any other term kind may be a subject or an object.

</rule>

<rule id="dm-blank-scope" strength="must">

A blank-node label is scoped to one request (one SPARQL update, one load, one API batch). The same label in two requests yields two different blank-node IDs.

</rule>

## Statements

A **Statement** is a subject–predicate–object fact with a 64-bit statement ID. The ID stays the same when the object changes, so annotations stay attached across overwrites.

- follows_from:: [[adr:0004]]
- follows_from:: [[adr:0006]]

<rule id="dm-unique-spo" strength="must">

For a predicate that is not a **Multi predicate**, at most one statement exists per `(s, p, o)`. Inserting an existing `(s, p, o)` returns the existing statement ID and changes nothing.

</rule>

<rule id="dm-multi-predicate" strength="must">

A predicate declared `multi` in the meta-model allows several statements with the same `(s, p, o)`, each with its own ID. Changing a predicate between multi and non-multi is a schema migration that rewrites its entries ([[04-physical-layout#Tables]]).

</rule>

<rule id="dm-update-in-place" strength="must">

Updating the object of a statement keeps its ID. The old `(s, p, o)` disappears from every index and the new one appears in the same transaction. If the new `(s, p, o)` already exists on a non-multi predicate, the update fails with a conflict error and changes nothing.

</rule>

<rule id="dm-delete-cascade" strength="must">

Deleting a statement deletes, in the same transaction and transitively, every statement whose subject is its ID. The details and guards are in [[06-transactions-durability#Cascade delete]].

</rule>

<rule id="dm-id-stable" strength="must">

A statement ID is never reused after deletion ([[02-id-encoding#Allocation]]).

</rule>

<example>

<input>

`stmt#42 = (alice, :worksAt, acme)` with annotation `stmt#77 = (stmt#42, :confidence, 0.9)`; then the object of `#42` is set to `globex`.

</input>

<output>

`stmt#42 = (alice, :worksAt, globex)`; `stmt#77` still annotates `#42`. A scan for `(alice, :worksAt, acme)` finds nothing.

</output>

</example>

### Operations

The data model offers five write operations; every surface lowers to them. Costs are counted in B-tree operations on the base tables of [[04-physical-layout#Tables]].

| Operation | Effect | Base-table writes (node-valued object) | Base-table writes (literal object) |
| --- | --- | --- | --- |
| `insert(s,p,o)` | add a statement, return its ID | 3 orderings + `stmt` = 4 | 1 (`props`) |
| `set(s,p,o)` | replace all values of `(s,p)` with `o` | delete old + insert new | 1 (`props`) |
| `update(id,o)` | change the object of statement `id` | 3 deletes + 3 inserts + `stmt` = 7 | promoted: same as node-valued |
| `delete(id)` | remove a statement and cascade | 4 per statement in the cascade | 1 |
| `annotate(id,p,o)` | insert `(id,p,o)`; promotes `id` first if needed | as `insert` | as `insert` |

Each count adds on-demand orderings and value indexes that exist for the predicate. Costs in time are to be measured ([[17-benchmarks#Engine bake-off]]).

## Inline properties

An **Inline property** is a statement whose object is a literal. It lives in its subject's property record, not in the orderings, so an overwrite touches one tree. To queries it is an ordinary statement.

- follows_from:: [[adr:0005]]

<rule id="dm-route-by-kind" strength="must">

A statement whose object is an inline or dictionary literal is stored as an inline property of its subject, unless it is promoted. A statement whose object is any other kind is stored in the orderings. There is no per-property choice.

</rule>

<rule id="dm-props-virtual" strength="must">

Inline properties are visible to every query surface as statements. A pattern with an unbound predicate or an unbound object kind reads both the property records and the orderings ([[10-ir#Virtual relations]]).

</rule>

<rule id="dm-props-no-id" strength="must">

An inline property has no statement ID until it is promoted. A query that binds the ID of an unpromoted inline property (for example a SPARQL reifier variable) gets no binding for it, as if it had no reifier.

</rule>

<rule id="dm-value-index-auto" strength="should">

The planner creates a value index on a predicate when it sees repeated value-driven lookups on it; the meta-model can also declare one. A value index never changes query results, only cost ([[14-planner-execution#Statistics]]).

</rule>

Several values per `(subject, predicate)` are allowed on inline properties (for example two `:alias` strings). `set` replaces all of them; `insert` adds one.

## Promotion

**Promotion** moves an inline property into the orderings with its own statement ID, when something annotates it. It is one-way: a promoted property is never demoted.

- follows_from:: [[adr:0005]]

<rule id="dm-promote-on-annotate" strength="must">

Annotating an inline property, or asking for its statement ID through any API that must return one, promotes it in the same transaction: a statement ID is allocated in a statement chunk of the subject's home graph, the statement is written to `stmt` and the orderings, and the property slot is replaced by a pointer to the ID.

</rule>

<rule id="dm-promote-pointer" strength="must">

A promoted property's slot in the property record holds its statement ID (kind tag `statement`), so reading a node's properties is still one record read, followed by one `stmt` read per promoted value.

</rule>

<rule id="dm-no-demotion" strength="must">

Removing all annotations of a promoted property leaves it promoted. Later value updates keep its ID ([[adr:0005]]).

</rule>

The same promotion pattern applies to hyperedge roles ([[08-hyperedges#Role triples]]), graph memberships ([[09-graphs#Extra membership]], [[adr:0017]]) and derived facts ([[13-datalog#Materialized maintenance]], [[adr:0009]]): each is virtual until annotated.

## Annotations

An annotation is a statement whose subject is a statement ID. Because statement IDs are ordinary terms, statements about statements need no special storage, and annotations of annotations nest to any depth.

<rule id="dm-annotation-is-statement" strength="must">

An annotation is stored like any statement: in the orderings if its object is a node, as an inline property of the annotated statement if its object is a literal.

</rule>

<rule id="dm-annotation-targets" strength="must">

The subject of an annotation can be a stored statement, a promoted inline property, a promoted hyperedge role, a promoted graph membership or a promoted derived fact. Annotating any of the last four kinds while it is still virtual promotes it first.

</rule>

### Reifiers in RDF 1.2

In SPARQL the statement ID is the reifier ([[11-sparql#Reifiers]]). RDF 1.2 allows things this model does not represent directly; they are open:

- several distinct reifiers for the same triple ([[open-questions#Multiple reifiers per triple]]);
- a reifier for a triple that is not asserted ([[open-questions#Unasserted triple terms]]).

## Metagraph kinds

meta-core supports four kinds of metagraph structure. Each maps onto statements and IDs, with a dedicated physical structure where the generic mapping would be too slow.

| Kind | Logical form | Physical form | Spec |
| --- | --- | --- | --- |
| statements about statements | `(stmt#42, :confidence, 0.9)` | ordinary statement or inline property | [[01-data-model#Annotations]] |
| n-ary **Hyperedge** | role triples `(e, :role, node)` | one record + incidence entries; roles virtual | [[08-hyperedges]] |
| nested **Graph** | `(stmt, :in, g)`, `(g, :in, parent)` | **Chunk** membership + `gmem`; DAG closure `in_star` | [[09-graphs]] |
| meta-model | schema, shapes, rules as statements | statements in the system graph | [[01-data-model#Meta-model]] |

<rule id="dm-scope-transitive" strength="must">

The **Scope** of a graph is the graph and all graphs nested under it; graph clauses match the scope by default and direct membership only when asked ([[adr:0007]], [[09-graphs#Scope expansion]]).

</rule>

<rule id="dm-nesting-dag" strength="must">

Graph nesting forms a DAG: a graph may have several parents and an `:in` edge that would close a cycle is rejected ([[adr:0008]], [[09-graphs#Nesting DAG]]).

</rule>

## Meta-model

The meta-model is the schema of a database stored as statements in the system graph. The planner, the write path and the rule engine read it; changing it is an ordinary write transaction with extra validation.

<rule id="dm-system-graph" strength="must">

The system graph has the fixed graph ID of [[02-id-encoding#Allocation]] and holds all meta-model statements. Its IRIs use a reserved namespace ([[open-questions#System namespace IRI]]).

</rule>

| Meta-model entry | Declares | Read by |
| --- | --- | --- |
| predicate declaration | `multi` flag, value index, on-demand orderings, datatype | write path, planner |
| hyperedge type | arity, role names, role ranges, fixed width | write path, planner |
| class and label | class IRIs used as Cypher labels | Cypher front end (v1) |
| shape | constraints checked at commit (optional) | write path |
| rule | Datalog rule text, `@materialized` or `@on_demand` | rule engine |
| index declaration | value indexes, on-demand orderings, vector indexes | planner, write path |

<rule id="dm-metamodel-validate" strength="must">

A write to the system graph is validated before commit: a predicate cannot become `multi` while a hyperedge type or rule depends on it being single, a hyperedge type's arity cannot shrink while records of that type exist, and a rule that fails stratification is rejected.

</rule>

<non-goal>

A full SHACL or OWL engine in v0. Shapes are a small checked subset; reasoning profiles come through rules ([[13-datalog]]).

</non-goal>

## Failure behaviour

All model operations are transactional. A failure inside a transaction (conflict on update, cascade over its limit, meta-model validation error) aborts the whole transaction and leaves no partial state.

## Test plan

The model is tested through the Rust API, independent of any query surface, so every surface's tests can assume it.

- Property tests: random sequences of the five operations against a naive in-memory model, compared after each transaction.
- Promotion tests: annotate, update, delete and re-annotate inline properties, hyperedge roles and memberships; IDs stay stable.
- Uniqueness tests: duplicate inserts on single and multi predicates; update into an existing `(s,p,o)` fails.
- Meta-model tests: every validation rule in `dm-metamodel-validate` has a rejecting case.

## Open items

Questions this spec leaves open; each is an `OpenQuestion` in [[open-questions]].

- [[open-questions#Multiple reifiers per triple]]
- [[open-questions#Unasserted triple terms]]
- [[open-questions#System namespace IRI]]
- [[open-questions#Value index drop policy]]

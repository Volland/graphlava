---
type: Requirement
status: draft
priority: could
specs: ["adr:0013", "adr:0004", "adr:0005", "adr:0006"]
---
# Cypher subset

The Cypher/GQL subset that ships in v1: the supported clauses, how nodes, relationships, labels and properties map onto statements, and the semantics the shared IR must carry for it.

- follows_from:: [[adr:0013]]
- follows_from:: [[adr:0004]]
- depends_on:: [[10-ir#Modes]] {strength: hard}

## Purpose

Cypher is the second text surface, scheduled for v1 ([[adr:0013]]). This spec exists in v0 so that the IR carries edge isomorphism, bag semantics and Cypher nulls from the start and v1 needs no IR rewrite.

The target dialect is openCypher 9 for clause semantics, with GQL (ISO/IEC 39075:2024) spellings accepted where they are a superset of openCypher and cheap to parse. Detailed GQL conformance is not a goal.

## Subset

The clauses and features in the v1 subset, and what is explicitly out. Anything outside the subset is a compile error that names the clause.

| Feature | v1 | Notes |
| --- | --- | --- |
| `MATCH`, `OPTIONAL MATCH` | yes | edge isomorphism per `MATCH` |
| `WHERE`, `RETURN`, `WITH`, `ORDER BY`, `SKIP`, `LIMIT`, `DISTINCT` | yes | |
| `UNWIND` | yes | |
| `CREATE` | yes | non-multi deviation below |
| `MERGE` with `ON CREATE` / `ON MATCH` | yes | |
| `SET`, `REMOVE` (properties, labels) | yes | |
| `DELETE`, `DETACH DELETE` | yes | cascade semantics |
| Variable-length paths `-[:T*1..5]->` | yes | `Path` operator |
| Shortest path `shortestPath`, `allShortestPaths` | yes | BFS kernel |
| Aggregates `count`, `sum`, `avg`, `min`, `max`, `collect` | yes | |
| List and map expressions, `CASE` | yes | |
| `CALL` procedures | only `meta.datalog(program, args)` and `meta.vector.search` | |
| `FOREACH`, `LOAD CSV`, `CALL { … } IN TRANSACTIONS` | no | |
| Schema DDL (`CREATE INDEX`, constraints) | no | use the meta-model |
| Multiple graphs (`USE`) | `USE g` maps to graph scope | transitive, as SPARQL `GRAPH` |

<non-goal>

`LOAD CSV`, `FOREACH`, `CALL { } IN TRANSACTIONS`, user procedures and Cypher schema DDL are out of scope. Bulk import uses the Rust API.

</non-goal>

## Mapping

How the property-graph model of Cypher maps onto meta-core's statements, inline properties and hyperedges. The mapping is fixed so that Cypher and SPARQL see the same data.

- depends_on:: [[01-data-model#Statements]] {strength: hard}
- depends_on:: [[01-data-model#Inline properties]] {strength: hard}

| Cypher | meta-core | Notes |
| --- | --- | --- |
| node | any non-literal term (IRI, blank node, statement, graph, hyperedge) | `id(n)` returns the u64 ID |
| relationship `-[r:T]->` | statement `(a, T, b)` with ID `r` | `T` is a predicate IRI |
| relationship type name `T` | IRI `T` in the connection's default namespace, or a declared mapping | |
| node label `:Person` | statement `(n, rdf:type, :Person)` | see rule below |
| node property `n.name` | inline property `(n, :name, literal)` | |
| relationship property `r.since` | inline property of the statement `r` | annotations on statement IDs |
| `null` property | property absent | no stored nulls |
| hyperedge | node with role relationships `-[:role]->` | role triples are virtual ([[08-hyperedges#Role triples]]) |

<rule id="cy-label-type" strength="must">

A label is a statement `(n, rdf:type, L)` where `L` is the label's IRI. `MATCH (n:Person)` scans POS with `p = rdf:type, o = :Person`. The label predicate is configurable per database in the meta-model; the default is `rdf:type`, so Cypher labels and RDF classes are the same thing.

</rule>

<rule id="cy-rel-id" strength="must">

A relationship variable binds the statement ID. `id(r)` returns it, and the same value is the SPARQL reifier of that statement ([[11-sparql#Reifiers]]).

</rule>

<rule id="cy-rel-literal" strength="must">

A relationship can only connect two nodes. A Cypher property is never exposed as a relationship, and a statement whose object is a literal is a property, not a relationship ([[adr:0005]]).

</rule>

<rule id="cy-create-non-multi" strength="must">

`CREATE (a)-[:T]->(b)` on a non-multi predicate `T` when `(a, T, b)` already exists binds the existing statement and creates nothing. This is a documented deviation from Cypher ([[adr:0004]]). On a `multi` predicate every `CREATE` makes a new statement with a new ID.

</rule>

<rule id="cy-delete-cascade" strength="must">

`DELETE r` deletes the statement and cascades to its annotations ([[06-transactions-durability#Cascade delete]]). `DELETE n` on a node with relationships fails as in Cypher; `DETACH DELETE n` deletes all statements where `n` is subject or object, their annotations, and any hyperedge that has `n` in a role.

</rule>

<rule id="cy-edge-iso" strength="must">

Within one `MATCH`, two relationship variables never bind the same statement ID (edge isomorphism). The compiler sets `edge_iso = on` on the `MATCH`'s join ([[10-ir#Modes]]).

</rule>

<rule id="cy-null-semantics" strength="must">

Cypher three-valued logic and null propagation apply: a missing property reads as `null`, comparisons with `null` are `null`, and `WHERE` keeps only `true`. The compiler sets `null_semantics = cypher_null` and `type_errors = null`.

</rule>

<example>

<input>

```cypher
MATCH (a:Person {name: 'Alice'})-[r:WORKS_AT]->(c)
WHERE r.confidence > 0.8
RETURN c.name, id(r)
```

</input>

<output>

`Scan(triple, p=rdf:type, o=:Person) ⋈ Scan(prop, p=:name, v="Alice") ⋈ Scan(triple, p=:WORKS_AT) -> (a, c, r)`, then `Scan(prop, n=r, p=:confidence)` with the filter, then `Scan(prop, n=c, p=:name)`. `id(r)` is the statement ID, no decode.

</output>

</example>

## Algorithms and complexity

How a Cypher query is compiled. The pipeline mirrors SPARQL so that plans are shared and cached in the same way.

1. Parse (linear); clauses are a pipeline where each `WITH` closes a scope.
2. Resolve names: labels, types and property keys to IRIs, then to IDs (one dictionary lookup each).
3. Translate each `MATCH` to an n-ary `Join` with `edge_iso`, `OPTIONAL MATCH` to `LeftJoin`, `WITH` to `Project` plus optional `Aggregate`, variable-length patterns to `Path` with bounds.
4. Updates compile to `Write` ops ([[10-ir#Operators]]) run in one writer transaction.

Variable-length paths without an upper bound use a BFS kernel with a visited set; cost O(reachable nodes + edges) per start node. Cypher's relationship-isomorphism for paths (no relationship repeated within one path) is enforced by the kernel.

## Failure behaviour

Errors follow Cypher conventions where they exist (for example, deleting a node that still has relationships). Every update query is atomic.

<rule id="cy-update-atomic" strength="must">

One Cypher query is one transaction; any error rolls back all its writes.

</rule>

## Test plan

Tests that prove the subset and its mapping, reusing the openCypher TCK where the subset allows.

- openCypher TCK scenarios for the supported clauses; skipped scenarios are listed with a reason (for example, `CREATE` duplicates on non-multi types).
- Cross-surface tests: data written by Cypher is read by SPARQL and the other way round, with equal results for labels, relationships, properties and relationship IDs.
- Edge-isomorphism tests on cyclic patterns (triangles, squares).

## Open items

Questions this spec leaves open; each is an `OpenQuestion` in the open-questions file.

- [[open-questions#Cypher relationship type namespace]]: how type names, labels and property keys map to IRIs.
- [[open-questions#Cypher null and comparison semantics]]: how far `cypher_value` comparison is emulated over RDF terms.
- [[open-questions#Cypher multi default]]: whether predicates created through Cypher default to `multi`.

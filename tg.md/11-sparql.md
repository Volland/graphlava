---
type: Requirement
openspec: [sparql-query]
status: draft
priority: must
specs: ["adr:0014", "adr:0013", "adr:0007", "adr:0008", "adr:0004", "adr:0006", "adr:0017", "adr:0005"]
---
# SPARQL 1.2

The primary query surface of meta-core: the v0 scope, how RDF 1.2 reifiers map to statement IDs, graph semantics, the Datalog service, documented deviations and the conformance targets.

- follows_from:: [[adr:0014]]
- follows_from:: [[adr:0013]]
- depends_on:: [[10-ir#Operators]] {strength: hard}
- depends_on:: [[01-data-model#Annotations]] {strength: hard}

## Purpose

SPARQL 1.2 is the first text surface ([[adr:0013]]). The parser produces an algebra tree as in the SPARQL 1.1 spec, section 18, which the compiler maps to the shared IR; nothing in this layer executes.

The parser target is a named W3C SPARQL 1.2 draft recorded in [[11-sparql#Conformance]]. Parsing can reuse an existing Rust SPARQL parser (for example `spargebra` from Oxigraph) if its RDF 1.2 support matches the pinned draft; this is checked in v0, not assumed.

## Scope

What v0 supports, what is deferred and what is out of scope. Anything not listed here is rejected with an "unsupported feature" error naming the construct.

| Area | v0 | v1 | Out of scope |
| --- | --- | --- | --- |
| Query forms | `SELECT`, `ASK`, `CONSTRUCT` | `DESCRIBE` | |
| Patterns | BGP, `OPTIONAL`, `UNION`, `MINUS`, `FILTER`, `BIND`, `VALUES`, subqueries, `EXISTS` / `NOT EXISTS` | | |
| Paths | all property path forms | path step to a reifier (deviation 3) | |
| Graphs | `GRAPH :g`, `GRAPH ?g`, `GRAPH! :g`, `FROM`, `FROM NAMED` | | |
| Solution modifiers | `DISTINCT`, `REDUCED`, `ORDER BY`, `LIMIT`, `OFFSET`, `GROUP BY`, `HAVING`, aggregates | | |
| RDF 1.2 | triple terms, reifiers `~`, `<< … >>`, annotations `{\| … \|}`, `TRIPLE`, `SUBJECT`, `PREDICATE`, `OBJECT`, `isTRIPLE` | | |
| Update | `INSERT DATA`, `DELETE DATA`, `DELETE/INSERT … WHERE`, `DELETE WHERE`, `CREATE`, `DROP`, `CLEAR` | `COPY`, `MOVE`, `ADD`, `LOAD` of local files | remote `LOAD` |
| Federation | `SERVICE <meta:datalog>` | remote `SERVICE` over HTTP | |
| Functions | SPARQL 1.1 builtins and XSD casts | | full-text and GeoSPARQL functions |

<non-goal>

Remote `LOAD`, full-text search and GeoSPARQL are not part of the core. They may become extensions behind the `ServiceResolver` trait or a separate crate.

</non-goal>

<rule id="sparql-unsupported-error" strength="must">

An unsupported construct is a compile error with the construct name and its source position. It is never silently ignored or approximated.

</rule>

## Reifiers

RDF 1.2 reifiers are statement IDs. The reifier of a triple is the ID of the statement with that `(s,p,o)`, so binding a reifier is free in every ordering scan and `rdf:reifies` is never stored.

<rule id="sparql-reifier-is-id" strength="must">

For a pattern `<< ?s ?p ?o ~ ?r >>` the compiler emits one `Scan(triple)` that binds `?r` to the statement `id` ([[10-ir#Virtual relations]]). No join with an `rdf:reifies` relation is generated.

</rule>

<rule id="sparql-reifier-unique" strength="must">

Because `(s,p,o)` is unique for non-multi predicates ([[adr:0004]]), a triple has at most one reifier. Inserting `<< :a :p :b ~ :r2 >>` when the statement already has ID `#42` binds `:r2` to `#42` only if `:r2` is a fresh blank node; if `:r2` is an IRI that is not `#42`, the update fails with "reifier mismatch". For `multi` predicates each reifier names its own statement.

</rule>

<rule id="sparql-reifier-iri" strength="should">

Reifiers are returned to clients as IRIs of the form `<urn:meta-core:stmt:{id}>`. Clients may use these IRIs in later queries and updates; the parser maps them back to the ID without a dictionary lookup.

</rule>

<rule id="sparql-annotation-syntax" strength="must">

`:a :p :b {| :confidence 0.9 |}` compiles to: insert or find `(:a, :p, :b)` with ID `r`, then insert `(r, :confidence, 0.9)`. The annotation's object is a literal, so it is stored as an inline property of `r` ([[adr:0005]]). If `(:a :p :b)` is itself an inline property, it is promoted first ([[01-data-model#Promotion]]).

</rule>

<rule id="sparql-triple-term-lazy" strength="must">

A triple-term value (`<<( :a :p :b )>>`) is built only when a query projects it or compares it with another term. In all other cases the plan carries the statement ID.

</rule>

<example>

<input>

```sparql
SELECT ?who ?c WHERE {
  << ?who :worksAt :acme ~ ?r >> .
  ?r :confidence ?c .
  FILTER(?c > 0.8)
}
```

</input>

<output>

`Join(Scan(triple, p=:worksAt, o=:acme) -> (who, r), Scan(prop, n=r, p=:confidence) -> c)` with `Filter(c > 0.8)` pushed into the property read. Two scans, no `rdf:reifies` join.

</output>

</example>

## Graph semantics

How `GRAPH`, `FROM` and `FROM NAMED` map to graph scope. The default is transitive scope over the nesting DAG; `GRAPH!` is direct membership only.

- follows_from:: [[adr:0007]]
- follows_from:: [[adr:0008]]
- depends_on:: [[09-graphs#Scope expansion]] {strength: hard}

| Syntax | Meaning | IR |
| --- | --- | --- |
| `GRAPH :g { P }` | statements in `:g` or any graph nested under it | `P` joined with `in_star(id, :g)` as a range filter |
| `GRAPH! :g { P }` | statements whose direct graph is `:g` (home or `gmem`) | `P` joined with `in(id, :g)` |
| `GRAPH ?g { P }` | binds `?g` to each direct graph of the matching statement | `in(id, ?g)`; never an ancestor |
| no `GRAPH`, no `FROM` | the default graph is the union of all graphs | no membership filter |
| `FROM :g` | the default graph is the scope of `:g` (transitive) | as `GRAPH :g` around the whole pattern |
| `FROM NAMED :g` | restricts `GRAPH ?g` to `:g` and its descendants | `in(id, ?g)` with `in_star(?g, :g)` |

<rule id="sparql-graph-transitive" strength="must">

`GRAPH :g { P }` with a constant `:g` matches a statement exactly once if it is in the scope of `:g`, whatever the number of paths through the nesting DAG ([[09-graphs#Nesting DAG]]).

</rule>

<rule id="sparql-graph-var-direct" strength="must">

`GRAPH ?g { P }` binds `?g` only to direct graphs (home graph or extra memberships), so a statement in graph `e1` nested under `agent` binds `?g = e1`, not `agent`. A query that needs ancestors writes `GRAPH ?g0 { P } ?g0 :in* ?g`.

</rule>

<rule id="sparql-graph-direct-mode" strength="must">

The connection flag `graph_scope = direct` makes `GRAPH :g` behave as `GRAPH! :g`, which is standard SPARQL semantics. The conformance runner uses it ([[11-sparql#Conformance]]).

</rule>

<example>

<input>

`g_agent` contains `g_ep1`; `(:a :said "hi")` is in `g_ep1`. Query `SELECT ?x WHERE { GRAPH :g_agent { ?x :said ?m } }`.

</input>

<output>

One row `?x = :a`. With `GRAPH! :g_agent` the result is empty. With `GRAPH ?g { ?x :said ?m }` the row has `?g = :g_ep1`.

</output>

</example>

## Datalog service

`SERVICE <meta:datalog> { … }` evaluates Datalog rules inside a SPARQL query through the shared IR. It is a local operator, never a network call, and it sees the same snapshot as the enclosing query.

- follows_from:: [[adr:0014]]
- depends_on:: [[13-datalog#On-demand evaluation]] {strength: hard}

This is the embedding path for Datalog inside SPARQL. Datalog is also a standalone query language with its own entry point ([[13-datalog#Standalone queries]]); both paths share one parser, IR and planner.

<rule id="sparql-service-datalog-named" strength="must">

Inside `SERVICE <meta:datalog>`, a triple pattern whose predicate is the IRI of a declared rule head (for example `?x :reachable ?y`) is evaluated by that rule. Variables bound outside the service are passed in as adornments, so magic sets restrict the evaluation to the relevant part ([[13-datalog#On-demand evaluation]]).

</rule>

<rule id="sparql-service-datalog-inline" strength="must">

A service body may start with a string literal holding a Datalog program in the surface syntax ([[13-datalog#Surface syntax]]). Its rules are compiled for this query only and are not stored. Inline rules may not be `@materialized`.

</rule>

<rule id="sparql-service-snapshot" strength="must">

The service evaluates against the same read snapshot as the outer query, and inside an update request it sees the state before any `Write` op ([[10-ir#Operators]]).

</rule>

<rule id="sparql-service-resolver" strength="must">

Every `SERVICE` IRI goes through a `ServiceResolver` trait. The v0 resolver knows only `<meta:datalog>`; any other IRI is an error unless `SERVICE SILENT` is used, in which case it yields one empty solution as in SPARQL 1.1 Federated Query. Remote resolvers (HTTP, bind-join with batches, timeouts) arrive in v1 with the server crate ([[16-api#Server crate]]).

</rule>

<example>

<input>

```sparql
PREFIX : <http://example.org/>
SELECT ?y WHERE {
  SERVICE <meta:datalog> {
    """ reach(?x, ?y) :- edge(?x, ?y) .
        reach(?x, ?z) :- reach(?x, ?y), edge(?y, ?z) .
        edge(?x, ?y) :- triple(?x, :knows, ?y) . """
    :alice :reach ?y
  }
}
```

</input>

<output>

The inline program is compiled with `reach` adorned `bf` (first argument bound to `:alice`); magic sets make the fixpoint start from `:alice` only.

</output>

</example>

### Mapping of rule heads to IRIs

A rule head predicate is visible from SPARQL under an IRI. Named rules declare their IRI in the meta-model; inline rules use the prefix in scope, or `<meta:rule:{name}>` when the name has no prefix.

## Deviations

Places where meta-core intentionally differs from, or extends, SPARQL 1.2. Each deviation has an id so tests and docs can point at it.

<rule id="sparql-dev-inplace-update" strength="must">

Deviation 1, in-place update. A `DELETE`/`INSERT` request that deletes `(s, p, o)` and inserts `(s, p, o')` with the same reifier variable bound to the same statement ID updates the statement in place: the ID is kept and annotations stay attached ([[01-data-model#Statements]]). Standard SPARQL would create a new triple with no annotations.

</rule>

<example>

<input>

```sparql
DELETE { << :alice :worksAt ?old ~ ?r >> }
INSERT { << :alice :worksAt :globex ~ ?r >> }
WHERE  { << :alice :worksAt ?old ~ ?r >> }
```

</input>

<output>

The statement with ID `?r` now has object `:globex`; `?r :confidence 0.9` still holds. Without the `~ ?r` on both sides the request is a plain delete plus insert, and the delete cascades to the annotations ([[06-transactions-durability#Cascade delete]]).

</output>

</example>

<rule id="sparql-dev-graph-transitive" strength="must">

Deviation 2, transitive `GRAPH`. `GRAPH :g` covers nested graphs; `GRAPH!` is the standard form ([[11-sparql#Graph semantics]]).

</rule>

<rule id="sparql-dev-reifier-path" strength="should">

Deviation 3, path steps across meta levels. In v1 a property path may use `^~` (from a reifier to its subject) and `~` as a path step from a triple to its reifier, so `:alice (:worksAt ~ :source)` reaches the source of the statement. v0 rejects the syntax.

</rule>

<rule id="sparql-dev-datalog" strength="must">

Deviation 4, Datalog. `SERVICE <meta:datalog>` is a reserved local service ([[11-sparql#Datalog service]]).

</rule>

<rule id="sparql-dev-unique" strength="must">

Deviation 5, set semantics and unique statements. Inserting an existing triple is a no-op, as in RDF; for `multi` predicates every `INSERT` creates a new statement, which RDF set semantics do not have.

</rule>

<rule id="sparql-dev-delete-cascade" strength="must">

Deviation 6, cascade. `DELETE` of a statement also deletes its annotations, transitively, unless the request carries the `KEEP ANNOTATIONS` keyword after `DELETE` ([[adr:0006]]). A delete that would remove a statement used as a hyperedge role fails unless the request uses `DELETE DETACH`.

</rule>

<rule id="sparql-dev-graph-default" strength="should">

Deviation 7, graph of inserts. `INSERT DATA` without a `GRAPH` clause inserts into the connection's default write graph, which is the system default graph unless the client sets another. Standard SPARQL inserts into the default graph, which here is the union of all graphs and therefore not writable.

</rule>

## Algorithms and complexity

How a SPARQL request is compiled. Each step is linear in the size of the query except planning, which is bounded by the planner's own limits.

1. Parse to the SPARQL algebra (linear in query length).
2. Normalise: expand prefixes, intern constants to IDs through the dictionary (one lookup per distinct constant; unknown constants make the pattern empty, not an error), fold reifier patterns.
3. Translate to IR with the SPARQL modes ([[10-ir#Modes]]): BGP to n-ary `Join`, `OPTIONAL` to `LeftJoin`, `MINUS` to `AntiJoin`, property paths to `Path`, `GRAPH` to membership joins.
4. Hand to the planner ([[14-planner-execution#Join selection]]); prepared queries cache the result of steps 1–4 ([[14-planner-execution#Plan cache]]).

## Failure behaviour

Errors are reported in the SPARQL 1.1 Protocol shape when served over the network and as typed errors in the Rust API; an update either commits completely or not at all.

<rule id="sparql-update-atomic" strength="must">

One update request is one writer transaction. Any error, including a cascade over the per-transaction limit, rolls back the whole request.

</rule>

## Conformance

The test suites that define "SPARQL done" for v0, and how meta-core's deviations are handled inside them.

<rule id="sparql-conf-11" strength="must">

The W3C SPARQL 1.1 query and update test suites run in `direct` graph mode. Every skipped test is listed in a checked-in manifest with a reason (for example, tests that need duplicate quads or remote `SERVICE`). v0 is not done while any unlisted test fails.

</rule>

<rule id="sparql-conf-12" strength="must">

The W3C RDF 1.2 and SPARQL 1.2 test suites run against a pinned draft. The draft name and date are recorded in the test manifest; moving to a newer draft is an explicit change.

</rule>

<rule id="sparql-conf-deviations" strength="must">

Each deviation rule in this file has at least one own test that proves the meta-core behaviour, and the conformance runner proves the standard behaviour in `direct` mode where one exists.

</rule>

## Test plan

Beyond the conformance suites: differential and property tests that guard the reifier and graph mappings, which are where meta-core differs most from a plain triple store.

- Differential testing against Oxigraph on random SPARQL queries over random datasets with no nested graphs, in `direct` mode (results must match as bags).
- Reifier tests: every annotation pattern returns the same results whether the annotated statement started as an inline property or as a stored statement.
- Graph tests: random nesting DAGs; `GRAPH :g` results equal the union over the descendant closure computed naively, with each statement once.
- Update tests: in-place update keeps IDs and annotations; plain delete plus insert cascades.
- Latency: prepared point lookup and 2-hop queries, see [[17-benchmarks#Engine bake-off]] (to be measured).

## Open items

Questions this spec leaves open; each is an `OpenQuestion` in the open-questions file.

- [[open-questions#SPARQL 1.2 draft pin]]: which W3C draft v0 targets.
- [[open-questions#Reifier IRI form]]: whether reifiers are exposed as `urn:meta-core:stmt:{id}` IRIs or as blank nodes.
- [[open-questions#Reifier on existing statement]]: what happens when an update names a different IRI reifier for an existing triple.
- [[open-questions#Default write graph]]: where `INSERT DATA` without `GRAPH` writes.
- [[open-questions#SPARQL parser reuse]]: reuse `spargebra` or write an own parser for RDF 1.2 syntax.

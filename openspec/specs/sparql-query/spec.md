# sparql-query Specification

## Purpose

SPARQL 1.2 query and update over graphlava, with reifiers bound to statement IDs, transitive graph scope, and Datalog callable as a service. Design: `tg.md/11-sparql.md`.

## Requirements

### Requirement: SPARQL 1.2 query forms
The system SHALL execute `SELECT`, `ASK` and `CONSTRUCT` with basic graph patterns, `OPTIONAL`, `UNION`, `MINUS`, `FILTER`, `BIND`, `VALUES`, subqueries, aggregates, property paths, `ORDER BY`, `LIMIT` and `OFFSET`.

Formal: none — SPARQL has no L0 model; covered by the W3C suites (L5) and e2e tests

#### Scenario: Optional match
- **Level**: e2e
- **WHEN** `SELECT ?p ?e WHERE { ?p a :Person OPTIONAL { ?p :email ?e } }` runs over two people of whom one has an email
- **THEN** two rows return, one with `?e` unbound

### Requirement: SPARQL update
The system SHALL execute `INSERT DATA`, `DELETE DATA`, `DELETE/INSERT ... WHERE`, `CREATE`, `DROP` and `CLEAR`, each request as one transaction.

Formal: none — SPARQL has no L0 model; covered by the W3C update suite (L5) and e2e tests

#### Scenario: Delete-insert is atomic
- **Level**: e2e
- **WHEN** a `DELETE/INSERT ... WHERE` request fails in its insert part
- **THEN** nothing it deleted is gone

#### Scenario: Writes without GRAPH go to the default graph
- **Level**: e2e
- **WHEN** `INSERT DATA { :a :p :b }` runs with no `GRAPH` and no transaction override
- **THEN** the statement is a member of the default graph created with the file, not of the system graph

### Requirement: Reifiers are statement IDs
The system SHALL bind the reifier of a triple in `<< s p o ~ ?r >>` and annotation syntax to its statement ID without an extra join and return it as `https://graphlava.org/ns/stmt/{id}`; a second reifier IRI for that triple SHALL become an alias, and the reifier of an unasserted triple SHALL be an `unasserted` statement seen only by reifier lookups.

Formal: none — query-layer binding (identity: `MetaCore.Store.insert_idempotent`)

#### Scenario: Annotate and query back
- **Level**: e2e
- **WHEN** `INSERT DATA { :alice :worksAt :acme {| :confidence 0.9 |} }` runs, then `SELECT ?c WHERE { << :alice :worksAt :acme ~ ?r >> . ?r :confidence ?c }`
- **THEN** the result is `0.9`

#### Scenario: Reifier round-trips as an IRI
- **Level**: e2e
- **WHEN** a query returns `?r` for statement `#42` and a later query uses `<https://graphlava.org/ns/stmt/42>` as a subject
- **THEN** the later query matches the annotations of `#42`

#### Scenario: Second reifier is an alias
- **Level**: e2e
- **WHEN** `:alice :worksAt :acme` has an ID and an update names the reifier `:r2` for the same triple and annotates it
- **THEN** `:r2` and the statement ID return the same annotations, and no second statement exists

#### Scenario: Unasserted triple is not matched
- **Level**: e2e
- **WHEN** `:bob :claims << :alice :worksAt :globex >>` is inserted and `:alice :worksAt :globex` is not asserted
- **THEN** `SELECT ?o WHERE { :alice :worksAt ?o }` does not return `:globex`, and the reifier of the quoted triple has `:bob :claims` pointing at it

### Requirement: In-place update through the same reifier
A request that deletes a triple and inserts a new triple with the same reifier SHALL update the statement in place and keep its ID and annotations.

Formal: none — the in-place update it lowers to is `MetaCore.Store.update_wf`; the SPARQL collapse is tested at level e2e

#### Scenario: Change employer keeps annotation
- **Level**: e2e
- **WHEN** the reifier `?r` of `:alice :worksAt :acme` is reused to insert `:alice :worksAt :globex` in the same request that deletes the old triple
- **THEN** `?r` still identifies the statement, which now has object `:globex`, and `?r :confidence 0.9` still holds

### Requirement: Graph scope semantics
`GRAPH :g { P }` SHALL match statements in `:g` and its nested graphs; `GRAPH! :g { P }` SHALL match direct members only; a compatibility mode SHALL make `GRAPH` direct for standard conformance.

Formal: `MetaCore.Nesting.scope_trans`, `MetaCore.Graphs.scope_mem` (scope); the SPARQL mapping is tested at levels e2e and conformance

#### Scenario: Standard mode
- **Level**: conformance
- **WHEN** the W3C test suite runs in direct compatibility mode
- **THEN** `GRAPH` behaves as in SPARQL 1.1

### Requirement: Datalog through SERVICE
`SERVICE <meta:datalog> { P }` SHALL evaluate `P` against named or inline Datalog rules in the same transaction as the enclosing query.

Formal: none — the service is query-layer wiring; Datalog evaluation is covered by the datalog-query theorems

#### Scenario: Reachability from SPARQL
- **Level**: e2e
- **WHEN** a rule `reach(X,Y)` is defined and a query uses `SERVICE <meta:datalog> { :a :reach ?y }`
- **THEN** every node reachable from `:a` is returned

### Requirement: Conformance
The system SHALL pass the W3C SPARQL 1.1 query and update test suites in direct mode, except for listed skips with reasons, and the tests of the pinned, dated SPARQL 1.2 / RDF 1.2 TR snapshot.

Formal: none — conformance is measured by the W3C suites (L5)

#### Scenario: Conformance run
- **Level**: conformance
- **WHEN** the conformance suite runs in CI
- **THEN** every non-skipped test passes and the skip list is unchanged unless reviewed

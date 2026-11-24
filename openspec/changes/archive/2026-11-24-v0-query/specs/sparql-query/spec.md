# Spec Delta

## Purpose

SPARQL 1.2 query and update over graphlava, with reifiers bound to statement IDs, transitive graph scope, and Datalog callable as a service. Design: `tg.md/11-sparql.md`.

## ADDED Requirements

### Requirement: SPARQL 1.2 query forms
The system SHALL execute `SELECT`, `ASK` and `CONSTRUCT` with basic graph patterns, `OPTIONAL`, `UNION`, `MINUS`, `FILTER`, `BIND`, `VALUES`, subqueries, aggregates, property paths, `ORDER BY`, `LIMIT` and `OFFSET`.

#### Scenario: Optional match
- **WHEN** `SELECT ?p ?e WHERE { ?p a :Person OPTIONAL { ?p :email ?e } }` runs over two people of whom one has an email
- **THEN** two rows return, one with `?e` unbound

### Requirement: SPARQL update
The system SHALL execute `INSERT DATA`, `DELETE DATA`, `DELETE/INSERT ... WHERE`, `CREATE`, `DROP` and `CLEAR`, each request as one transaction.

#### Scenario: Delete-insert is atomic
- **WHEN** a `DELETE/INSERT ... WHERE` request fails in its insert part
- **THEN** nothing it deleted is gone

#### Scenario: Writes without GRAPH go to the default graph
- **WHEN** `INSERT DATA { :a :p :b }` runs with no `GRAPH` and no transaction override
- **THEN** the statement is a member of the default graph created with the file, not of the system graph

### Requirement: Reifiers are statement IDs
The system SHALL bind the reifier of a triple in `<< s p o ~ ?r >>` and annotation syntax to that statement's ID, without an extra join, and SHALL return it as the IRI `https://graphlava.org/ns/stmt/{id}`. A further reifier IRI named for a triple that already has an ID SHALL become an alias of that ID. A reifier of a triple that is not asserted SHALL create an `unasserted` statement that reifier lookups see and ordinary pattern matching does not.

#### Scenario: Annotate and query back
- **WHEN** `INSERT DATA { :alice :worksAt :acme {| :confidence 0.9 |} }` runs, then `SELECT ?c WHERE { << :alice :worksAt :acme ~ ?r >> . ?r :confidence ?c }`
- **THEN** the result is `0.9`

#### Scenario: Reifier round-trips as an IRI
- **WHEN** a query returns `?r` for statement `#42` and a later query uses `<https://graphlava.org/ns/stmt/42>` as a subject
- **THEN** the later query matches the annotations of `#42`

#### Scenario: Second reifier is an alias
- **WHEN** `:alice :worksAt :acme` has an ID and an update names the reifier `:r2` for the same triple and annotates it
- **THEN** `:r2` and the statement ID return the same annotations, and no second statement exists

#### Scenario: Unasserted triple is not matched
- **WHEN** `:bob :claims << :alice :worksAt :globex >>` is inserted and `:alice :worksAt :globex` is not asserted
- **THEN** `SELECT ?o WHERE { :alice :worksAt ?o }` does not return `:globex`, and the reifier of the quoted triple has `:bob :claims` pointing at it

### Requirement: In-place update through the same reifier
A request that deletes a triple and inserts a new triple with the same reifier SHALL update the statement in place and keep its ID and annotations.

#### Scenario: Change employer keeps annotation
- **WHEN** the reifier `?r` of `:alice :worksAt :acme` is reused to insert `:alice :worksAt :globex` in the same request that deletes the old triple
- **THEN** `?r` still identifies the statement, which now has object `:globex`, and `?r :confidence 0.9` still holds

### Requirement: Graph scope semantics
`GRAPH :g { P }` SHALL match statements in `:g` and its nested graphs; `GRAPH! :g { P }` SHALL match direct members only; a compatibility mode SHALL make `GRAPH` direct for standard conformance.

#### Scenario: Standard mode
- **WHEN** the W3C test suite runs in direct compatibility mode
- **THEN** `GRAPH` behaves as in SPARQL 1.1

### Requirement: Datalog through SERVICE
`SERVICE <meta:datalog> { P }` SHALL evaluate `P` against named or inline Datalog rules in the same transaction as the enclosing query.

#### Scenario: Reachability from SPARQL
- **WHEN** a rule `reach(X,Y)` is defined and a query uses `SERVICE <meta:datalog> { :a :reach ?y }`
- **THEN** every node reachable from `:a` is returned

### Requirement: Conformance
The system SHALL pass the W3C SPARQL 1.1 query and update test suites in direct mode, except for listed skips with reasons, and the tests of the pinned, dated SPARQL 1.2 / RDF 1.2 TR snapshot.

#### Scenario: Conformance run
- **WHEN** the conformance suite runs in CI
- **THEN** every non-skipped test passes and the skip list is unchanged unless reviewed

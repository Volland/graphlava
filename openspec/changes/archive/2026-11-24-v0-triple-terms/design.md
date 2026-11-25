# Design

## Context

Reifiers are statement IDs ([[adr:0031]], [[adr:0032]], rule `sparql-reifier-is-id`); `rdf:reifies` is virtual and a triple term written in update data is stored as the ID of its statement, unasserted when the triple is not asserted. The query engine had no value for a triple term, so `TRIPLE()` failed and the W3C triple-term tests were skipped.

## Decisions

### A triple term is a transient ID unless a statement backs it

`Term::Triple` values computed by a query (`TRIPLE()`, `<<( … )>>` in `VALUES`, the object of a virtual `rdf:reifies`) get a transient ID in the executor, like other computed terms ([[22-implementation-decisions#Transient IDs for computed terms]]). When the triple is a stored statement (asserted or unasserted), scans probe with that statement's ID and joins treat the transient ID and the statement ID as the same value. A triple term with no statement matches no stored data.

- motivated_by:: rule `sparql-triple-term-lazy`
- Rejected: a new ID kind for triple terms. It would change the frozen ID layout ([[adr:0011]]) for a value the store already identifies by statement ID.

### Statement IDs decode as reifier IRIs in graphlava mode

A statement ID bound by a scan stays a statement ID and decodes as `<https://graphlava.org/ns/stmt/{id}>`. `=`, `IN` and `sameTerm` against a triple term convert a reifier IRI to its triple term first, so the term is built only when compared.

- Rejected: decoding every object-position statement ID as a triple term. Existing clients read reifier IRIs there.

### Standard mode shows the RDF 1.2 view

With `QueryOptions::standard()` an object-position statement ID binds as its triple term, a statement that has annotations has a virtual `(r, rdf:reifies, <<( s p o )>>)` in each graph holding one of its annotations, and a statement reifier decodes as its alias IRI or else as a blank node. Several reifiers of one triple remain one statement, so tests that need distinct reifiers stay skipped.

### Ordering and equality

`ORDER BY` puts triple terms after literals and orders them by subject, predicate, object. `=` compares components with `=`; `sameTerm` is structural.

## Formal model

No Lean module changes: the store model has no query values, and no stored state changes. The refinement replay is unaffected.

## Test strategy

L5: the W3C SPARQL 1.2 suite (`tests/w3c.rs`) with the passing tests removed from `skip.txt`. Unit tests in `func.rs` for the five functions, equality and ordering; an e2e test for `TRIPLE()` and triple terms in `VALUES` against stored statements.

## Cost model

No write path changes. Reading a stored triple term costs one `stmt` get per level; matching one costs one `spo`, `spo_m` or `unasserted` point lookup.

## Failure model

No new durable state; errors in triple-term functions are expression errors (unbound results), never request failures.

# Proposal

## Why

The `v0-query` slice passes the W3C SPARQL 1.1 suites but skips 40 SPARQL 1.2 tests because `Term` has no triple-term variant: 36 `eval-triple-terms` tests and 4 `TRIPLE()` expression tests (`crates/graphlava/tests/w3c/skip.txt`). [[openspec:sparql-query#Conformance]] already requires the pinned 1.2 tests to pass or to be listed with a reason, so this change adds triple-term values without changing any requirement text. It carries no spec deltas.

## What Changes

- `Term::Triple(Box<TripleTerm>)`, displayed in N-Triples form `<<( s p o )>>`.
- The executor maps triple terms to IDs: a triple term is a transient ID; where a stored statement exists, joins and scans compare it with that statement's ID; reifier IRIs, aliases and unasserted statements keep their behaviour ([[adr:0032]], rule `sparql-triple-term-lazy`).
- `TRIPLE`, `SUBJECT`, `PREDICATE`, `OBJECT`, `isTRIPLE`; `=`, `sameTerm` and `ORDER BY` on triple terms; triple terms in `VALUES`, `BIND`, `FILTER`, `CONSTRUCT` templates and results, and in update data.
- Standard (conformance) mode shows the RDF 1.2 view of reifiers: object-position statement IDs are triple terms, annotated statements carry a virtual `rdf:reifies` triple, and reifiers without an alias read as blank nodes.
- The C FFI gains `GRAPHLAVA_TERM_TRIPLE`; the W3C runner compares triple terms.

Out of scope: several reifiers for one triple (they stay one statement ID, rule `sparql-reifier-unique`), base direction, and the `spargebra` parser limits already listed.

## Capabilities

### New Capabilities

None.

### Modified Capabilities

None. The behaviour implements the existing [[openspec:sparql-query#Conformance]] and [[openspec:sparql-query#Reifiers are statement IDs]] requirements.

## Impact

- **Code:** `crates/graphlava/src/{term,dict}.rs`, `src/query/{exec,func}.rs`, `src/sparql/{mod,update}.rs`, `src/store.rs` (read-only lookups), `crates/graphlava-ffi`, `tests/w3c.rs`.
- **lat.md sections:** [[11-sparql#Reifiers]], [[11-sparql#Conformance]], [[01-data-model#Reifiers in RDF 1.2]], [[22-implementation-decisions]] (new section amending [[adr:0032]]).
- **File format:** unchanged. Triple terms that are not stored statements live only in the executor; stored ones reuse statement IDs and unasserted statements.
- **Verification:** sparql-query L5 (W3C SPARQL 1.2 suite) and unit tests for the functions; L3 e2e tests stay green.
- **Performance:** graphlava mode keeps carrying statement IDs; triple terms are built only when projected or compared. Standard mode builds them at scan time, which only the conformance runner uses. No benchmark covers standard mode.

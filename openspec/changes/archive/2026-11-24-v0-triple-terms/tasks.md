# Tasks

## 1. Term and functions

- [x] 1.1 Add `Term::Triple(Box<TripleTerm>)` with N-Triples `Display`; update every exhaustive match (dictionary lookup, FFI, W3C runner)
- [x] 1.2 Implement `TRIPLE`, `SUBJECT`, `PREDICATE`, `OBJECT`, `isTRIPLE`, `=`, `sameTerm` and `ORDER BY` on triple terms in `func.rs`, with unit tests

## 2. Executor and SPARQL

- [x] 2.1 Map triple terms to transient IDs and to statement IDs in scans, joins, `DISTINCT` and grouping; match unasserted statements in reifier patterns
- [x] 2.2 Triple terms in `VALUES`, `BIND`, `FILTER`, `CONSTRUCT` templates and results, and in `INSERT DATA` / `DELETE DATA` / template updates
- [x] 2.3 Standard mode: object-position statement IDs as triple terms, virtual `rdf:reifies` for annotated statements, reifiers decoded as alias IRIs or blank nodes
- [x] 2.4 Add an e2e test for `TRIPLE()` and triple terms in `VALUES` and `FILTER`, annotated `@tg: verifies::` the reifier requirement

## 3. FFI and conformance

- [x] 3.1 Add `GRAPHLAVA_TERM_TRIPLE` to `graphlava_term_decode` and regenerate the header
- [x] 3.2 Convert triple terms in `tests/w3c.rs`, run the W3C suites, remove passing entries from `skip.txt` and give each remaining skip an honest reason

## 4. Documentation and checks

- [x] 4.1 Record the ID mapping in [[22-implementation-decisions]] with `amends:: [[adr:0032]]`; update [[11-sparql#Conformance]] and `PINNED.md` skip counts
- [x] 4.2 Run `cargo test --workspace`, `cargo clippy --workspace --all-targets`, `cargo +1.87 check --workspace --all-targets`, `tg check` and `tg trace sparql-query --gaps`

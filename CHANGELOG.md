# Changelog

All notable changes to the graphlava crates. The project follows [Semantic Versioning](https://semver.org); until 1.0, minor versions may break the API, and until the [format freeze](tg.md/18-roadmap.md) any version may change the file format.

## 0.1.0-alpha.1 (2026-11-24)

First public pre-release of `graphlava`, `graphlava-ffi` and `graphlava-cli`. **The file format is not frozen:** a later release may not open files written by this one. Do not keep data you cannot regenerate.

### graphlava

- Store: 64-bit term IDs with order-preserving inline literals, a dictionary with reclamation, statements with stable IDs that survive updates, inline literal properties, promotion on annotation, cascade delete, `multi` and `functional` predicates.
- Transactions: one owner process, snapshot reads, `sync`/`ordered`/`relaxed` durability with group commit, one-shot write requests, interactive transactions with a 100 ms limit, quick repair (default on iOS and Android).
- Named and nested graphs: home-graph membership, extra memberships, transitive `GRAPH` and direct `GRAPH!` scope, DAG nesting, graph drop.
- Hyperedges: typed records with roles, incidence index, virtual role triples, literal attributes, role promotion, delete guard.
- SPARQL 1.2 query and update (reifiers as statement IDs, triple terms, `SERVICE <meta:datalog>`); W3C SPARQL 1.1 suites pass in standard mode with documented skips.
- Datalog: Nemo-compatible syntax, stratified negation and aggregation, magic sets, semi-naive evaluation, `@assert`.
- Planner: statistics, index nested-loop joins, Leapfrog Triejoin for cyclic patterns, bidirectional path search, plan cache with drift invalidation.

### graphlava-ffi

- C API with status codes, checked handles, 1024-row ID batches with on-demand decoding, interactive transactions, core reads, suspend/resume and an iOS extension journal; header generated with cbindgen (ABI version 1).

### graphlava-cli

- The `graphlava` command: query and update, Datalog, import and export of Turtle, N-Triples, N-Quads and TriG (RDF 1.2), an interactive shell, and admin commands.

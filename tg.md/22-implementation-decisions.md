---
type: Decision
status: accepted
specs: ["adr:0002", "adr:0004", "adr:0005", "adr:0006", "adr:0007", "adr:0008", "adr:0010", "adr:0011", "adr:0016", "adr:0017", "adr:0021", "adr:0024", "adr:0025", "adr:0028", "adr:0031", "adr:0032", "adr:0033", "adr:0034", "adr:0027", "adr:0038", "adr:0039", "adr:0040", "adr:0041"]
---
# Implementation decisions

Choices made while implementing the `v0-store-core`, `v0-graphs-hyperedges`, `v0-query` and `v0-triple-terms` slices that the ADRs left open or that refine them. Each is small; any that changes the file format is frozen with it ([[18-roadmap#Format freeze]]).

## redb 2.6.4 with file format v3

The engine is redb pinned at exactly 2.6.4, and new files use redb's file format v3. redb 4.x and 5.x need Rust 1.90 or newer, above the project MSRV of 1.87.

- amends:: [[adr:0002]]

File format v3 is the only format redb 3.x and later write, so moving to a newer redb later needs no file migration. Raising the MSRV to use redb 5 is a separate decision.

## Owned bytes and paged scans in the storage trait

The storage trait returns owned byte vectors and pages its scans, instead of the borrowed slices behind GAT cursors sketched in [[05-storage-trait#Trait API]].

- motivated_by:: [[adr:0025]]

An object-safe trait lets the per-request overlay sit between the store and the engine. Fixed-width tables still map to fixed-width redb types, so the on-disk layout is unchanged.

## Literal objects of multi predicates are stored statements

A statement on a `multi` predicate always gets an ID and lives in the multi tables, even when its object is a literal.

- amends:: [[adr:0005]]
- amends:: [[adr:0004]]

Two identical inline property slots could not be told apart, so the inline route cannot hold parallel statements.

## Blank nodes have no dictionary entry

A blank node is a sequential ID with no `dict_fwd` or `dict_rev` record. Its label is derived from the ID (`b<n>`), and labels written by a caller map to fresh IDs per request.

- amends:: [[adr:0011]]

The sweep therefore never sees blank nodes. A blank node lives exactly as long as statements mention it.

## Language tag tables

Language tags live in `lang_fwd` (tag → `u16`) and `lang_rev` (`u16` BE → tag), append-only, with the count in `meta`.

- amends:: [[adr:0033]]

When the table is full, a new tag is kept in the dictionary literal's body (`text@tag`) with language ID 0.

## Unasserted statements index

An unasserted statement has a `stmt` record with flag bit 6 and an entry in the `unasserted` table keyed `(s, p, o)`. It has no ordering entries.

- amends:: [[adr:0032]]

Asserting the triple later moves it into the orderings and keeps its ID. The cascade finds object references to it with a scan of the `unasserted` table, which is expected to stay small.

## Predicate declarations in meta

`multi` and `functional` flags, and the value-index state of a predicate, are stored in `meta` under `pred:<id>` until the system graph holds meta-model statements ([[01-data-model#Meta-model]]).

- amends:: [[adr:0004]]
- amends:: [[adr:0028]]

Changing a predicate between multi and non-multi is rejected while it has statements. Declaring it functional is rejected while a subject has two objects.

## Sparse commit clock for the sweep window

The writer records `txclock[commit] = unix time` at most once a minute. The sweep turns its time window into a commit counter through that table.

- amends:: [[adr:0016]]

Terms keep `created_tx` as the spec says. The window is conservative by up to one minute.

## Inline strings never end with a NUL byte

A string or language string whose last byte is `0x00` is a dictionary literal, never inline.

- amends:: [[adr:0021]]

Zero padding would make `s` and `s` followed by `0x00` the same ID. The proof of `MetaCore.Id.encStr_injective` needs this precondition ([[20-proofs]]).

## Property records spill at 128 entries

With 16-byte entries, the 2 KiB limit of [[adr:0034]] is reached at 128 entries, before the 256-entry limit. In practice records spill above 127 entries.

- amends:: [[adr:0034]]

Both limits are kept as written until the benchmark tunes them before the freeze.

## Read snapshots borrow the database

`ReadTxn<'db>` borrows the `Database`, as in [[16-api#Rust API]]. A redb read transaction keeps the file open, so a snapshot that outlived its handle would keep the owner lock after close.

- motivated_by:: [[adr:0002]]

## Inline properties belong to their subject's home graph

An unpromoted inline property has no chunk, so it is a member of its subject's home graph: the default graph for IRIs and blank nodes, the item's own graph otherwise.

A literal triple written into any other graph is stored as a promoted statement in a statement chunk of that graph.

- amends:: [[adr:0005]]
- amends:: [[adr:0010]]

Literal annotations and hyperedge attributes stay inline in their graph, so the one-tree overwrite of [[adr:0005]] holds for them. Literal values of IRIs written into named graphs pay the ordering writes of a stored statement.

## Writes without a graph go to the subject's home graph

A write with no graph clause targets its subject's home graph. For IRIs and blank nodes this is the default graph, as rule `gr-default-graph` says. An annotation goes to the graph of the item it annotates.

- amends:: [[adr:0038]]

Without this, a literal annotation of a statement in a named graph would be promoted into the default graph on every write.

## Graph names

A named graph has a graph ID from the system graph's chunks; `gname` maps its name IRI to that ID and `gname_rev` back. A write that names a new graph creates it.

The fixed graphs are named `meta:defaultGraph` and `meta:systemGraph` without table entries.

- amends:: [[adr:0010]]
- amends:: [[adr:0038]]

Decoding a graph ID returns its name IRI. Membership and nesting triples carry graph IDs, so a query surface that joins them with triples about the name IRI maps one to the other when it binds a variable.

## Hyperedge type declarations in meta

Hyperedge types (ordered roles with cardinality, required flag and filler kind, plus literal attributes) are stored in `meta` under `htype:<id>`, like predicate declarations.

While records of a type exist, only optional roles appended at the end and new attributes are accepted.

- amends:: [[adr:0028]]

Roles carry a `required` flag in addition to the cardinality of [[08-hyperedges#Hyperedge types]], because the spec scenario "Missing role rejected" needs it.

## Promoted roles and memberships stay out of the orderings

A promoted role edge or membership has a `stmt` record with flag bit 1 or 2 and no ordering entries; it is served as a virtual triple that carries its ID.

It is not listed as a direct member of any graph. Deleting a promoted membership removes that membership; a promoted role edge is deleted with its hyperedge.

- amends:: [[adr:0006]]
- amends:: [[adr:0017]]

The cascade treats both as dependents of their item, so they go with it whatever the delete mode.

## System IRIs are interned with the file

`meta:in` and `rdf:type` are interned when the file is created, and their IDs are recorded in `meta`. Virtual triples need them for patterns with an unbound predicate. The sweep marks them.

- amends:: [[adr:0011]]

## Nesting depth limit per file

The nesting depth limit is fixed when the file is created (default 64) and counts edges. A lattice of 40 stacked diamonds is 80 edges deep, so its test creates its file with a limit of 128.

- amends:: [[adr:0008]]

At 32 diamonds a `u32` path count overflows while the lattice is exactly at the default limit, which is why [[adr:0029]] keeps `in_star` as a set.

## Partitioned graphs are left to a later slice

- motivated_by:: [[adr:0003]]

The v0 table of [[18-roadmap#v0]] does not list partitioned graphs, and `v0-graphs-hyperedges` has no task for them. With only the global store, per-store uniqueness of [[adr:0024]] is global uniqueness.


## MSRV 1.87 for SPARQL 1.2 parsing

The project MSRV is Rust 1.87. Every `spargebra` release with the `sparql-12` feature needs 1.87; the last release under 1.85 parses only the older RDF-star syntax.

- amends:: [[adr:0038]]

The CI MSRV job checks 1.87. redb stays pinned at 2.6.4.

## Full-width rows in the executor

Rows carry one `u64` slot per query variable, `0` for unbound, and operators pass at most 1024 rows per batch into an index nested-loop probe. Batches are row-major, not columnar, and filters copy instead of using a selection vector.

- amends:: [[adr:0027]]

Full-width rows make SPARQL compatibility checks and `OPTIONAL` a slot comparison. A columnar layout is a performance change for the benchmarks to justify.

## Leapfrog Triejoin over sorted atom scans

A cyclic conjunction reads each atom once from the ordering that covers its constants and sorts the rows by the global variable order. The leapfrog then seeks inside these sorted arrays rather than through B+tree cursors.

- amends:: [[adr:0027]]

The join is still worst-case optimal; each atom costs one ordering scan. Seeking through storage cursors replaces the arrays when the bake-off measures the gap.

## Transient IDs for computed terms

A term computed during a query that is not in the dictionary (a `BIND`, an aggregate, an unknown constant) gets a transient ID in the `System` kind above bit 59. It never reaches the store, and a plan that needed one is not cached.

- amends:: [[adr:0011]]

## Triple terms map to statement IDs

A triple term that is a stored statement, asserted or unasserted, has that statement's ID; a computed one gets a transient ID. Scans probe with the statement ID and joins treat the two IDs as one value.

- amends:: [[adr:0032]]
- motivated_by:: [[adr:0031]]

Writing a triple term stores the statement's ID, creating an unasserted statement when the triple is not asserted. A statement ID bound by a scan stays a statement ID: in graphlava mode it decodes as its reifier IRI, and `=`, `IN` and `sameTerm` against a triple term read the reifier as its triple term (rule `sparql-triple-term-lazy`). `TRIPLE()` results, `<<( … )>>` in `VALUES` and the object of a virtual `rdf:reifies` are triple-term values and project as `Term::Triple`. Inside a triple term built from a statement, an object that is a statement is a nested triple term.

In standard mode (`QueryOptions::standard()`) the store is read as RDF 1.2 data: a statement in the object position of a matched triple binds as its triple term; a statement with at least one annotation has a virtual `(r, rdf:reifies, <<( s p o )>>)` in each graph holding one of its annotations; and a statement used as a reifier decodes as its alias IRI, or else as a blank node `_:stmt<id>`. Several reifiers of one triple stay one statement ([[11-sparql#Reifiers]]), so the conformance tests that need distinct reifiers remain skipped.

## The property record is the predicate summary

The per-node predicate summary of [[adr:0039]] is the node's property record, already sorted by predicate and read in one lookup. The ID's kind prunes the other sources: only hyperedges have role triples, only chunked items have membership triples.

- amends:: [[adr:0039]]

No extra table is kept in step with `props`.

## Statistics are exact counts plus characteristic sets

The writer keeps an exact statement count per predicate in `meta`; `Admin` computes characteristic sets in one pass. Distinct-subject and distinct-object counts are not kept yet; join ordering uses fixed fan-out guesses for bound positions.

- amends:: [[adr:0041]]

The plan cache compares these counts and the set of ready orderings to detect a 4x drift.

## Standard SPARQL mode

`QueryOptions::standard()` gives SPARQL 1.1 dataset semantics: `GRAPH` is direct, and a pattern without `GRAPH` matches only the default graph.

`DELETE DATA` without `GRAPH` then removes only the default-graph membership, and virtual membership triples are hidden.

- amends:: [[adr:0038]]

The graphlava default matches the union of all graphs without `GRAPH` and shows `(x, meta:in, g)` triples.

## GRAPH! through a pre-pass

`spargebra` has no `GRAPH!`. A pre-pass rewrites `GRAPH!` to `GRAPH` and opens its group with a marker triple, which the translator removes and turns into direct scope.

- amends:: [[adr:0038]]

## Datalog accepts Prolog-style variables

Identifiers starting with an uppercase letter are variables and `not` is negation, alongside Nemo's `?x` and `~`, because the spec scenarios are written that way. Lowercase bare identifiers stay Nemo constants.

- amends:: [[adr:0040]]

## Magic sets skip negation and aggregation

Magic sets apply when a goal binds an argument and the rules reachable from it have no negation or aggregation; otherwise the whole program runs. A magic rewrite through negation could make the program unstratified.

- amends:: [[adr:0040]]

## Engine bake-off outcome

The spike of [[17-benchmarks#Engine bake-off]] confirms redb 2.6.4 and the three base orderings, with four changes to the specs. Full numbers are in `spikes/bakeoff/RESULTS.md`.

- motivated_by:: [[adr:0002]]
- motivated_by:: [[adr:0015]]
- motivated_by:: [[adr:0025]]

redb passes the gate of [[adr:0002]]: no engine is 2x better on file size or mix p99. libmdbx is 1.57x better on relaxed p99, under the line, and its Rust crate returned corruption errors to concurrent readers.

Three orderings meet [[adr:0015]] on size (38.8% smaller), relaxed write latency (40% lower p50) and cyclic queries (within 0.93-1.05x of six orderings). They miss the 30% bar only for `sync` writes (2-12% lower), where one full fsync dominates; that bar is waived for `sync`.

The changes: a bounded run of non-durable commits (rule `tx-relaxed-bound`), durable commits for each on-demand build batch, relaxed data lost on an owner-process crash too, and per-request overlays because redb refuses a savepoint once a transaction has written ([[adr:0025]]).

## Quick repair on mobile

After any unclean shutdown redb 2.6.4 repairs the whole file on the next open: 3.4-15.5 s at 10M statements, 105-221 s at 100M. Quick repair cuts that to about 10 ms.

- motivated_by:: [[adr:0002]]
- motivated_by:: [[adr:0012]]

Quick repair doubles durable-commit latency (15 vs 8 ms at 10M, 28 vs 11 ms at 100M), so it is an open option (rule `tx-quick-repair`): on by default for iOS and Android, where every kill by the OS is unclean and app start time matters, off on desktop and servers.

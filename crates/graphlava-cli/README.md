<p align="center"><img src="https://graphlava.org/assets/logo.svg" width="140" alt="graphlava logo: a diamond of baklava whose layers are statements"></p>

<h1 align="center">graphlava-cli</h1>

<p align="center"><em>Layered like baklava. Linked like a graph.</em></p>

<p align="center"><strong>The <code>graphlava</code> command: query, update, import, export and inspect graphlava metagraph databases.</strong><br>
SPARQL&nbsp;1.2 and Datalog from the shell, RDF&nbsp;1.2 in and out, an interactive REPL, and the admin tools for one-file databases.</p>

<p align="center">
<a href="https://graphlava.org">Website</a> ·
<a href="https://graphlava.org/book/">Mini book</a> ·
<a href="https://graphlava.org/articles/">Articles</a> ·
<a href="https://github.com/Volland/graphlava/tree/main/crates/graphlava">Core crate</a> ·
<a href="https://github.com/Volland/graphlava">Repository</a>
</p>

<p align="center">
<a href="https://github.com/Volland/graphlava/actions/workflows/ci.yml"><img src="https://github.com/Volland/graphlava/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
<img src="https://img.shields.io/badge/rust-1.87%2B-orange" alt="Rust 1.87+">
<img src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue" alt="MIT OR Apache-2.0">
</p>

> **Status: pre-release (`0.1.0-alpha.1`).** The tool and the [graphlava](https://crates.io/crates/graphlava) core it embeds are pre-release. The file format is **not frozen yet**: files written by this version may not open in the next one. Keep your data in RDF too (`graphlava export`) and do not keep anything you cannot regenerate.

## Install

This is a pre-release, and `cargo install` skips pre-releases unless you name the version:

```sh
cargo install graphlava-cli --version 0.1.0-alpha.1
```

That installs one binary, `graphlava`. Rust 1.87 or newer.

```console
$ graphlava --version
graphlava 0.1.0-alpha.1 (graphlava core 0.1.0-alpha.1, file format 1)
```

## Commands

| Command | What it does |
| --- | --- |
| `graphlava query FILE [QUERY \| -f file.rq \| -]` | SPARQL 1.2 query; `--datalog` runs a Datalog program instead |
| `graphlava datalog FILE prog.dl` | Datalog program; `--update` writes `@assert` heads |
| `graphlava update FILE [UPDATE \| -f file.ru \| -]` | SPARQL 1.2 update as one transaction |
| `graphlava import FILE data.{nt,nq,ttl,trig}` | Load RDF 1.2: named graphs, reifiers and annotations included |
| `graphlava export FILE [-o out.{nt,nq,ttl,trig}]` | Write the database, or one graph, as RDF 1.2 |
| `graphlava shell FILE` | Interactive shell: SPARQL, Datalog and update modes |
| `graphlava info FILE` | File size, format version, feature flags, orderings |
| `graphlava stats FILE` | Statements per predicate, graphs and nesting, hyperedge types |
| `graphlava check FILE` | Cross-table invariant check; exit code 1 on a violation |
| `graphlava sweep FILE` | Mark and sweep unreferenced dictionary terms |
| `graphlava build-ordering FILE pso\|sop\|ops` | Build an on-demand ordering |
| `graphlava build-index FILE <predicate>` | Build a value index for range filters and `ORDER BY` |

`graphlava <command> --help` shows every option with examples. `--quiet` (`-q`) drops the progress and summary lines, which go to stderr, so stdout carries only data.

A database is one file and one owner process holds it; a second `graphlava` on a file that another process has open fails with exit code 3. `query`, `export` and the inspect commands refuse a missing file; `update`, `import`, `datalog --update` and `shell` create it.

## A tour

Every output below comes from running the commands on these two files.

`people.ttl`: a statement with an RDF 1.2 annotation (a statement about a statement):

```turtle
@prefix : <http://example.org/> .

:alice :name "Alice" ;
       :age 42 ;
       :knows :bob ;
       :worksAt :acme {| :confidence 0.9 ; :source :hr-export |} .
:bob   :name "Bob"@en ;
       :knows :carol .
:carol :name "Carol" .
```

`episode.trig`: a named graph nested inside another graph:

```trig
@prefix : <http://example.org/> .
@prefix meta: <https://graphlava.org/ns#> .

:episode1 {
  :alice :said "hello" .
  :bob :heard "hello" .
}
:episode1 meta:in :memory .
```

### Import

```console
$ graphlava import memory.glv people.ttl
imported 10 quads: 9 statements, 1 reifiers, 0 nestings, 0 graphs (7.84 ms)
$ graphlava import memory.glv episode.trig
imported 3 quads: 2 statements, 0 reifiers, 1 nestings, 1 graphs (6.96 ms)
```

Formats come from the extension (`.nt`, `.nq`, `.ttl`, `.trig`) or `--format`; `-` reads stdin. How RDF maps onto graphlava:

- Named graphs in N-Quads and TriG become graphs; `--graph IRI` puts the default-graph triples into a named graph.
- `<child> meta:in <parent>` between two graph names is a nesting edge.
- An RDF 1.2 reifier (`r rdf:reifies <<( s p o )>>`, which is what `{| … |}` and `~ r` produce) stands for the statement of `(s p o)`, so `r :confidence 0.9` annotates that statement. A blank-node reifier is the statement ID; an IRI reifier becomes an alias of it. A triple term in object position is the statement too, created unasserted when the triple is not asserted.
- Quads go in through the core write-request API in relaxed batches (`--batch`, default 10 000); one durable commit ends the import. The file is parsed into memory first, so blank nodes keep their identity across batches.

### Query

SELECT and ASK print an aligned table by default. A reifier prints as `stmt:<id>`, short for `<https://graphlava.org/ns/stmt/<id>>`, which you can paste into later queries:

```console
$ graphlava query memory.glv 'PREFIX : <http://example.org/>
SELECT ?who ?org ?confidence ?r WHERE {
  << ?who :worksAt ?org ~ ?r >> . ?r :confidence ?confidence }'
?who    ?org   ?confidence  ?r
------  -----  -----------  ------------------------
:alice  :acme  0.9          stmt:4611686018428436481
1 row (0.71 ms)
```

Prefixes declared in the query, plus `rdf:`, `rdfs:`, `xsd:`, `owl:`, `meta:` and `stmt:`, shorten IRIs in the table. Numbers and booleans print bare; other literals keep their quotes, language tags and datatypes; triple terms print as `<<( s p o )>>`.

`--format json|csv|tsv` writes SPARQL 1.1 Query Results JSON, CSV or TSV (with RDF 1.2 triple terms in JSON as `"type": "triple"`):

```console
$ graphlava query memory.glv 'PREFIX : <http://example.org/> SELECT ?who ?name WHERE { ?who :name ?name }' --format csv
who,name
http://example.org/alice,Alice
http://example.org/bob,Bob
http://example.org/carol,Carol
3 rows (0.52 ms)
```

```console
$ printf 'PREFIX : <http://example.org/>\nSELECT ?who ?name WHERE { ?who :name ?name } ORDER BY ?name\n' | graphlava query memory.glv - --format tsv
?who	?name
<http://example.org/alice>	"Alice"
<http://example.org/carol>	"Carol"
<http://example.org/bob>	"Bob"@en
3 rows (0.95 ms)
```

`GRAPH :g` sees `g` and every graph nested under it (`GRAPH! :g` sees only `g`); `--standard` switches to SPARQL 1.1 dataset semantics:

```console
$ graphlava query memory.glv 'PREFIX : <http://example.org/> ASK { GRAPH :memory { :alice :said "hello" } }' --format json
{"head":{},"boolean":true}
true (0.53 ms)
```

CONSTRUCT and DESCRIBE print N-Triples:

```console
$ graphlava query memory.glv 'PREFIX : <http://example.org/> CONSTRUCT { ?b :heardFrom ?a } WHERE { ?a :said ?w . ?b :heard ?w }'
<http://example.org/bob> <http://example.org/heardFrom> <http://example.org/alice> .
1 triple (0.62 ms)
```

The query text can also come from `-f file.rq` or stdin (`-`, or no argument with piped input). `--timeout 5s` and `--max-rows N` bound a query.

### Update

```console
$ graphlava update memory.glv 'PREFIX : <http://example.org/>
INSERT DATA { :carol :knows :dan {| :since 2021 |} }'
inserted 2, deleted 0, updated in place 0 (6.75 ms)
```

`--durability sync|ordered|relaxed` (default `ordered`) sets when the update counts as committed, and `--standard` applies SPARQL 1.1 semantics to `DELETE DATA` without `GRAPH`.

### Datalog

`reach.dl`:

```
@prefix : <http://example.org/> .
reach(?x, ?y) :- :knows(?x, ?y) .
reach(?x, ?z) :- reach(?x, ?y), :knows(?y, ?z) .
?- reach(:alice, ?who) .
```

```console
$ graphlava datalog memory.glv reach.dl
?who
------
:bob
:carol
:dan
3 rows (0.93 ms)
```

`graphlava query FILE --datalog -f reach.dl` does the same. Programs whose heads are marked `@assert` write statements and need `graphlava datalog FILE prog.dl --update`.

### Export

```console
$ graphlava export memory.glv -o dump.trig
exported 16 quads (1.39 ms)
$ cat dump.trig
@prefix rdf: <http://www.w3.org/1999/02/22-rdf-syntax-ns#> .
@prefix xsd: <http://www.w3.org/2001/XMLSchema#> .
@prefix meta: <https://graphlava.org/ns#> .
<http://example.org/alice> <http://example.org/knows> <http://example.org/bob> ;
	<http://example.org/worksAt> <http://example.org/acme> .
<http://example.org/bob> <http://example.org/knows> <http://example.org/carol> .
<http://example.org/carol> <http://example.org/knows> <http://example.org/dan> .
_:s4000000000100001 rdf:reifies <<( <http://example.org/alice> <http://example.org/worksAt> <http://example.org/acme> )>> ;
	<http://example.org/source> <http://example.org/hr-export> .
<http://example.org/alice> <http://example.org/name> "Alice" ;
	<http://example.org/age> 42 .
<http://example.org/bob> <http://example.org/name> "Bob"@en .
<http://example.org/carol> <http://example.org/name> "Carol" .
_:s4000000000100001 <http://example.org/confidence> 0.9 .
_:s4000000000100004 rdf:reifies <<( <http://example.org/carol> <http://example.org/knows> <http://example.org/dan> )>> ;
	<http://example.org/since> 2021 .
<http://example.org/episode1> meta:in <http://example.org/memory> .
<http://example.org/episode1> {
	<http://example.org/alice> <http://example.org/said> "hello" .
	<http://example.org/bob> <http://example.org/heard> "hello" .
}
```

`--graph IRI` exports one graph and the graphs nested under it (`--direct` for the graph alone), with their nesting edges:

```console
$ graphlava export memory.glv --graph http://example.org/memory
<http://example.org/alice> <http://example.org/said> "hello" <http://example.org/episode1> .
<http://example.org/bob> <http://example.org/heard> "hello" <http://example.org/episode1> .
<http://example.org/episode1> <https://graphlava.org/ns#in> <http://example.org/memory> .
exported 3 quads (2.69 ms)
```

The format comes from `--format` or the `-o` extension, N-Quads by default. N-Triples and Turtle merge all graphs into one. Annotated statements get a reifier (`_:s<id>`, or their alias IRI), statements in object position become triple terms, and importing the export gives the same dataset back: the test suite round-trips N-Triples, N-Quads, Turtle and TriG with RDF 1.2 annotations. Hyperedges export as their `rdf:type` and role triples, so they come back as plain statements.

### Shell

```console
$ graphlava shell memory.glv
graphlava 0.1.0-alpha.1 (graphlava core 0.1.0-alpha.1, file format 1) shell on memory.glv. Statements end with ';' or an empty line; .help for commands.
sparql> .prefix : <http://example.org/>
sparql> SELECT ?who ?since WHERE {
......>   << ?who :knows ?x ~ ?r >> . ?r :since ?since
......> }
......>
?who    ?since
------  ------
:carol  2021
(1 row, 3.19 ms)
sparql> .mode datalog
datalog> ?- :knows(?x, :carol) .
.......>
?x
----
:bob
(1 row, 1.24 ms)
datalog> .mode update
update> DELETE DATA { :alice :age 42 } ;
inserted 0, deleted 1, updated in place 0 (11 ms)
update> .quit
```

A statement ends with `;` at the end of a line (outside braces) or with an empty line. Errors print and the shell carries on. `.prefix` declarations apply to every later statement in every mode. History lives in `~/.graphlava_history` (or `--history PATH`, or `$GRAPHLAVA_HISTORY`).

| Command | |
| --- | --- |
| `.mode sparql\|datalog\|update` | switch the language |
| `.format table\|json\|csv\|tsv` | switch the output format |
| `.prefix NAME: <IRI>` / `.prefixes` | declare or list prefixes |
| `.timing on\|off` | row counts and timings |
| `.durability sync\|ordered\|relaxed` | durability of updates |
| `.help`, `.quit` | |

With stdin not a terminal (`graphlava shell db.glv < script.txt`) the shell runs the script without prompts and exits with code 1 if any statement failed.

### Inspect and maintain

```console
$ graphlava info memory.glv
file                     memory.glv
size                     3.0 MiB (3162112 bytes)
format version           1 (this build reads 1)
features                 nesting (0x40)
commits                  6
orderings                spo pos osp (base), pso absent, sop absent, ops absent
graph depth limit        64
owned literal threshold  256 bytes
namespace cap            65536
core version             0.1.0-alpha.1 (cli 0.1.0-alpha.1)
```

```console
$ graphlava stats memory.glv
statements  12

predicate                        statements
<http://example.org/knows>                3
<http://example.org/name>                 3
<http://example.org/confidence>           1
<http://example.org/heard>                1
<http://example.org/said>                 1
<http://example.org/since>                1
<http://example.org/source>               1
<http://example.org/worksAt>              1

graph                              direct    in scope  nested in
(default graph)                        10          10  -
<http://example.org/episode1>           2           2  <http://example.org/memory>
<http://example.org/memory>             0           2  -

hyperedge types  none
```

With hyperedges, the last section lists each type with its hyperedge count and roles.

```console
$ graphlava check memory.glv
ok: no invariant violations
checked in 4.72 ms
$ graphlava sweep memory.glv
marked 18, examined 19, deleted 1, kept (young) 0, kept (re-referenced) 0
swept in 15 ms
$ graphlava build-ordering memory.glv pso
ordering pso ready
built in 28 ms
$ graphlava build-index memory.glv http://example.org/age
value index on <http://example.org/age> ready
built in 20 ms
```

`sweep --window 10m` keeps terms interned in the last ten minutes. `build-index` takes `<iri>`, a bare IRI or a prefixed name with a built-in prefix (`rdfs:label`).

## Exit codes

| Code | Meaning |
| --- | --- |
| 0 | success |
| 1 | the query, update, import or check failed (syntax error, invariant violation, …) |
| 2 | usage error: bad arguments, missing query text, unknown file format |
| 3 | the database could not be opened or written: missing file, locked by another process, I/O error, incompatible format |

```console
$ graphlava query missing.glv 'ASK {}'
error: cannot open missing.glv: no such database file
$ echo $?
3
```

Errors are red when both stdout and stderr are terminals and `NO_COLOR` is unset.

## Learn more

- [graphlava.org](https://graphlava.org): overview and status
- [Mini book: Hypergraphs and Metagraphs in Statement Graphs](https://graphlava.org/book/): 11 short chapters, from binary edges to recursive metagraphs
- [Articles](https://graphlava.org/articles/), including [Statement graphs: the one idea under graphlava](https://graphlava.org/articles/statement-graphs.html)
- [The graphlava crate](https://github.com/Volland/graphlava/tree/main/crates/graphlava): embed the database in a Rust program
- [Design specifications](https://github.com/Volland/graphlava/tree/main/tg.md) and [decision records](https://github.com/Volland/graphlava/tree/main/docs/adr)

## License

Licensed under either of [Apache License 2.0](https://github.com/Volland/graphlava/blob/main/LICENSE-APACHE) or [MIT](https://github.com/Volland/graphlava/blob/main/LICENSE-MIT) at your option.

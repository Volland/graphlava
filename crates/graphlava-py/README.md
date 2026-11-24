<p align="center"><img src="https://graphlava.org/assets/logo.svg" width="120" alt="graphlava logo"></p>

<h1 align="center">graphlava for Python</h1>

<p align="center"><em>Layered like baklava. Linked like a graph.</em></p>

<p align="center"><strong>An embedded metagraph database for agent memory, in one file, from Python.</strong><br>
Statements about statements, n-ary hyperedges and nested graphs, queried with SPARQL&nbsp;1.2 and Datalog. No server.</p>

<p align="center">
<a href="https://graphlava.org">Website</a> ·
<a href="https://graphlava.org/book/">Mini book</a> ·
<a href="https://graphlava.org/articles/">Articles</a> ·
<a href="https://github.com/Volland/graphlava/tree/main/crates/graphlava">Rust core</a> ·
<a href="https://github.com/Volland/graphlava">Repository</a>
</p>

> **Status: pre-release.** The API is complete and tested, but nothing is on PyPI yet, and the file format is not frozen: files written now may not open in the next version.

## Why graphlava

Agent frameworks keep memory as text chunks or flat triples. An agent needs more: *how sure* it is and *where it read* a fact, a meeting of three people stored as one thing, and episodes kept as graphs inside its memory. graphlava stores those three [metagraph](https://graphlava.org/book/03-metagraphs.html) powers natively: every statement has a stable ID you can annotate (the RDF 1.2 reifier), hyperedges are one record with typed roles, and graphs nest as a DAG. Read [Statement graphs: the one idea under graphlava](https://graphlava.org/articles/statement-graphs.html) for the model in ten minutes.

## Install

```sh
pip install graphlava            # once published; until then: pip install maturin && maturin develop -r
pip install "graphlava[arrow]"   # with pyarrow, for Rows.to_arrow()
```

One abi3 wheel per platform covers CPython 3.10 and newer; a separate wheel supports free-threaded CPython 3.14t. Wheels are built for Linux (manylinux and musllinux, x86-64 and arm64), macOS and Windows. Types ship with the package (`py.typed`).

## Quick start

```python
import graphlava as gl
from graphlava import Durability, Namespace

EX = Namespace("http://example.org/")

with gl.Database.open("memory.glv") as db:
    # A statement, then a statement about it, in one-shot requests.
    works = db.submit([gl.Insert(EX.alice, EX.worksAt, EX.acme)],
                      durability=Durability.ORDERED).results[0]     # a statement ID
    db.submit([gl.Annotate(works, EX.confidence, 0.9)])
    db.submit([gl.Update(works, EX.globex)])                        # same ID, annotation kept

    # The reifier ?r is the statement's ID.
    for org, c in db.sparql("""
            PREFIX : <http://example.org/>
            SELECT ?org ?c WHERE { << :alice :worksAt ?org ~ ?r >> . ?r :confidence ?c }"""):
        print(org, c)                                               # http://example.org/globex 0.9
```

Python values map to RDF terms: `str`, `int` (any size), `float`, `bool`, `Decimal`, timezone-aware `datetime` and `date` become typed literals; `IRI`, `BNode`, `Literal` and `Triple` are explicit terms; `Namespace` builds IRIs. Statement and node IDs are `gl.Id`, an `int` subclass holding the full 64 bits.

## SPARQL and Datalog

```python
db.sparql_update("PREFIX : <http://example.org/> INSERT DATA { :a :knows :b . :b :knows :c }")

reach = db.datalog("""
    @prefix : <http://example.org/> .
    reach(?x, ?y) :- :knows(?x, ?y) .
    reach(?x, ?z) :- reach(?x, ?y), :knows(?y, ?z) .
    ?- reach(:a, ?who) .""")
for (who,) in reach:
    print(who)
```

- **SPARQL 1.2** query and update: reifiers (`<< s p o ~ ?r >>`), annotation syntax (`{| … |}`), transitive `GRAPH` over nested graphs and direct `GRAPH!`, aggregates, property paths, and `SERVICE <meta:datalog>`.
- **Datalog** in Nemo-compatible syntax with stratified negation, aggregates and `@assert` heads (`db.datalog_update`).

## Large results

`Rows` is lazy and decodes terms only when you read them:

```python
for batch in db.sparql(query).iter_batches(size=1024):  # lists of row tuples
    ...
ids = db.sparql(query).ids()                              # IDs only, nothing decoded
table = db.sparql(query).to_arrow()                       # a pyarrow.Table (graphlava[arrow])
```

## Transactions and durability

```python
with db.write(Durability.SYNC) as w:     # commits on exit, aborts on an exception
    w.insert(EX.bob, EX.knows, EX.alice)
with db.read() as r:                     # one consistent snapshot
    bob = r.lookup(EX.bob)
    print(r.out(bob))
```

| Durability | `submit` / commit returns when |
| --- | --- |
| `Durability.SYNC` | the write is on disk |
| `Durability.ORDERED` (default) | its group commit is on disk; concurrent writers share one fsync |
| `Durability.RELAXED` | it is applied; a later durable commit makes it durable |

Prefer `db.submit([...])` for writes: an interactive `with db.write(...)` block holds the single writer for at most 100 ms. Every engine call releases the GIL, so other threads keep running during a `sync` commit, and the package works with free-threaded Python.

## Processes and fork

One process owns a database file; a second open of the same path, in any process, raises `LockedError`. A handle inherited across `fork()` raises `ForkedHandleError` in the child instead of corrupting the file: open the database again in the child, or use the `spawn` start method.

## Errors

Every engine error has its own exception class carrying its fields, all under `gl.Error`: `ConflictError`, `NotFoundError`, `LockedError`, `GraphCycleError`, `FillsRoleError` (`.item`, `.hedge`), `WriteTimeoutError` and more.

```python
try:
    db.submit([gl.Nest(EX.g1, EX.g2), gl.Nest(EX.g2, EX.g1)])
except gl.GraphCycleError as e:
    print(e)
```

## Learn more

- [graphlava.org](https://graphlava.org): what graphlava is and where it is going
- [Mini book: Hypergraphs and Metagraphs in Statement Graphs](https://graphlava.org/book/): 11 short chapters, from binary edges to recursive metagraphs
- [Articles](https://graphlava.org/articles/), including [graphlava and tiramemsu](https://graphlava.org/articles/graphlava-and-tiramemsu.html)
- [The Python binding design](https://github.com/Volland/graphlava/blob/main/tg.md/16-api.md) and [ADR 0042: binding strategy](https://github.com/Volland/graphlava/blob/main/docs/adr/0042-binding-strategy.md)
- Other languages: [Rust](https://github.com/Volland/graphlava/tree/main/crates/graphlava), [Node.js](https://github.com/Volland/graphlava/tree/main/bindings/node), [C](https://github.com/Volland/graphlava/tree/main/crates/graphlava-ffi), [Swift](https://github.com/Volland/graphlava/tree/main/bindings/swift), [Kotlin](https://github.com/Volland/graphlava/tree/main/bindings/kotlin)

## License

MIT OR Apache-2.0.

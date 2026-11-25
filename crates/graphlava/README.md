<p align="center"><img src="https://graphlava.org/assets/logo.svg" width="140" alt="graphlava logo: a diamond of baklava whose layers are statements"></p>

<h1 align="center">graphlava</h1>

<p align="center"><em>Layered like baklava. Linked like a graph.</em></p>

<p align="center"><strong>An embedded metagraph database for agent memory, in one file.</strong><br>
Statements carry statements, hyperedges are stored once, graphs nest inside graphs, and you query it all with SPARQL&nbsp;1.2 and Datalog.</p>

<p align="center">
<a href="https://graphlava.org">Website</a> ·
<a href="https://graphlava.org/book/">Mini book</a> ·
<a href="https://graphlava.org/articles/">Articles</a> ·
<a href="https://github.com/Volland/graphlava/tree/main/tg.md">Design specs</a> ·
<a href="https://github.com/Volland/graphlava">Repository</a>
</p>

<p align="center">
<a href="https://github.com/Volland/graphlava/actions/workflows/ci.yml"><img src="https://github.com/Volland/graphlava/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
<img src="https://img.shields.io/badge/rust-1.87%2B-orange" alt="Rust 1.87+">
<img src="https://img.shields.io/badge/license-MIT%20OR%20Apache--2.0-blue" alt="MIT OR Apache-2.0">
</p>

> **Status: pre-release `0.1.0-alpha.1`.** The store, named and nested graphs, hyperedges, SPARQL 1.2 and Datalog are implemented and tested. The file format is **not frozen yet**: files written by this version may not open in the next one. Do not keep data you cannot regenerate.

## What it is

graphlava is the storage engine you embed in an agent (or any app that needs a knowledge graph) when plain triples are not enough. An agent needs to say *how sure am I* and *where did I read it* about a fact, record a meeting between three people as one thing, and keep an episode as a graph inside its memory. Those are the three powers of a [metagraph](https://graphlava.org/book/03-metagraphs.html), and graphlava stores each natively:

| You want | You write | graphlava stores |
| --- | --- | --- |
| Facts about facts | `:alice :worksAt :acme {\| :confidence 0.9 \|}` | Every statement has a stable 64-bit ID. The ID *is* the RDF 1.2 reifier and survives value updates, so annotations stay attached. |
| n-ary relations | a meeting with an agent, a target and a time | One record with typed roles plus an incidence index; role triples stay virtual until you annotate one. |
| Context and provenance | an episode inside a memory inside an agent | Graphs are nodes that nest as a DAG. `GRAPH :g` sees `g` and everything nested under it; `GRAPH! :g` sees only `g`. |
| Fast overwrites | `:alice :age 43` replacing `42` | Literal values live inline in the node's property record: one B-tree write, no dictionary entry for small values. |

It is tuned for agent-memory workloads: many small write transactions, frequent value overwrites, point lookups, 2–3 hop traversals and star patterns. Read [Statement graphs: the one idea under graphlava](https://graphlava.org/articles/statement-graphs.html) for the model in ten minutes.

## Install

```sh
cargo add graphlava@0.1.0-alpha.1
```

This is a pre-release, so name the version: `cargo add graphlava` alone does not pick it. Rust 1.87 or newer; Linux, macOS, Windows, iOS and Android. For a command line, `cargo install graphlava-cli --version 0.1.0-alpha.1` gives you the `graphlava` binary.

## Quick start

```rust
use graphlava::{Database, Durability, OpenOptions, Reader, Term};

fn ex(local: &str) -> Term {
    Term::iri(format!("http://example.org/{local}"))
}

let db = Database::open("memory.glv", OpenOptions::default())?;

// Write: one transaction, durable when commit() returns.
let mut w = db.write(Durability::Ordered)?;
let works = w.insert(ex("alice"), ex("worksAt"), ex("acme"))?.unwrap();
w.annotate(works, ex("confidence"), Term::double(0.9))?;   // a statement about a statement
w.insert(ex("alice"), ex("age"), Term::integer(42))?;      // an inline property
w.commit()?;

// Update in place: the statement keeps its ID and its annotations.
let mut w = db.write(Durability::Ordered)?;
w.update(works, ex("globex"), None)?;
w.commit()?;

// Read from a snapshot.
let r = db.read()?;
let t = r.stmt(works)?.unwrap();
assert_eq!(r.decode(t.o)?, ex("globex"));
assert_eq!(r.props(works)?.len(), 1);                       // the annotation followed
# Ok::<(), graphlava::Error>(())
```

## SPARQL 1.2

Queries and updates follow the pinned SPARQL 1.2 draft; the W3C SPARQL 1.1 query and update suites pass in standard mode (`QueryOptions::standard()`), with a short list of documented skips.

```rust
db.sparql_update(r#"
    PREFIX : <http://example.org/>
    INSERT DATA { :bob :worksAt :acme {| :source :hr-export |} }"#,
    Durability::Ordered)?;

let rows = db.sparql(r#"
    PREFIX : <http://example.org/>
    SELECT ?who ?c WHERE { << ?who :worksAt ?org ~ ?r >> . ?r :confidence ?c }"#)?;
for row in rows.rows() { println!("{row:?}"); }
```

- A reifier (`~ ?r`) is the statement's ID: binding it costs no join, and it comes back as `https://graphlava.org/ns/stmt/{id}`, which you can use in later queries.
- A delete plus an insert through the same reifier updates the statement in place.
- `GRAPH :g { … }` is transitive over nested graphs, `GRAPH! :g { … }` is direct.
- `SERVICE <meta:datalog> { … }` runs Datalog rules inside a SPARQL query.

## Datalog

A standalone surface with Nemo-compatible syntax, stratified negation and aggregation, evaluated on demand with magic sets and semi-naive recursion.

```rust
let reach = db.datalog(r#"
    @prefix : <http://example.org/> .
    reach(?x, ?y) :- :knows(?x, ?y) .
    reach(?x, ?z) :- reach(?x, ?y), :knows(?y, ?z) .
    ?- reach(:a, ?who) ."#)?;
```

Rules see the same relations as SPARQL: `triple(s, p, o[, id])`, `prop`, `role`, `in` and `in_star`. Heads marked `@assert` write base statements (use `Database::datalog_update`).

## Nested graphs and hyperedges

```rust
use graphlava::{HedgeTypeDecl, RoleSpec};

let mut w = db.write(Durability::Ordered)?;
w.nest(ex("episode1"), ex("agent"))?;                                     // a DAG; cycles are rejected
w.insert_in(ex("episode1"), ex("alice"), ex("said"), Term::string("hello"))?;

w.declare_hedge_type(ex("Meeting"), HedgeTypeDecl {
    roles: vec![RoleSpec::one(ex("agent")), RoleSpec::one(ex("target"))],
    attrs: vec![ex("when").into()],
})?;
let meeting = w.create_hedge(ex("Meeting"), None,
    vec![(ex("agent").into(), ex("alice").into()), (ex("target").into(), ex("bob").into())],
    vec![])?;
w.commit()?;
```

A statement can belong to several graphs; deleting it inside one graph removes only that membership. Deleting a node that fills a hyperedge role fails unless you ask to detach.

## Durability and transactions

| Level | `commit()` returns when | Use for |
| --- | --- | --- |
| `Durability::Sync` | the transaction is on disk | facts you cannot lose |
| `Durability::Ordered` (default) | its group commit is on disk (a few ms window) | most writes |
| `Durability::Relaxed` | it is applied in memory; a later durable commit makes it durable | caches, bulk load |

Every transaction is atomic and crash-consistent at every level, and a crash keeps a prefix of whole transactions (checked by a crash harness that kills the process at every I/O call, and by a Lean 4 model). One owner process holds the file; other processes send one-shot `WriteRequest`s. Interactive write transactions are limited to 100 ms so a slow caller cannot stall the writer.

## How it is verified

- A **Lean 4 model** of the store, graphs, hyperedges and Datalog stratification with machine-checked invariants ([formal/](https://github.com/Volland/graphlava/tree/main/formal)).
- A **refinement replay**: traces from the Lean model are replayed through this crate, operation by operation.
- **Crash tests** at every I/O call, a 30-minute nightly soak, the **W3C SPARQL suites**, and a cross-OS golden-file test.

## Learn more

- [graphlava.org](https://graphlava.org): overview and status
- [Mini book: Hypergraphs and Metagraphs in Statement Graphs](https://graphlava.org/book/): 11 short chapters, from binary edges to recursive metagraphs
- [Articles](https://graphlava.org/articles/), including [graphlava and tiramemsu](https://graphlava.org/articles/graphlava-and-tiramemsu.html) and [graphlava and Neptune OneGraph](https://graphlava.org/articles/graphlava-and-neptune.html)
- [Design specifications](https://github.com/Volland/graphlava/tree/main/tg.md) and [decision records](https://github.com/Volland/graphlava/tree/main/docs/adr)
- Other languages: [C FFI](https://github.com/Volland/graphlava/tree/main/crates/graphlava-ffi), [Python](https://github.com/Volland/graphlava/tree/main/crates/graphlava-py), [Node.js](https://github.com/Volland/graphlava/tree/main/bindings/node), [Swift and Kotlin](https://github.com/Volland/graphlava/tree/main/bindings)

## License

Licensed under either of [Apache License 2.0](https://github.com/Volland/graphlava/blob/main/LICENSE-APACHE) or [MIT](https://github.com/Volland/graphlava/blob/main/LICENSE-MIT) at your option.

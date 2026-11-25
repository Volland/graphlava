<p align="center"><img src="https://graphlava.org/assets/logo.svg" width="120" alt="graphlava logo"></p>

<h1 align="center">@graphlava/node</h1>

<p align="center"><em>Layered like baklava. Linked like a graph.</em></p>

<p align="center"><strong>graphlava for Node.js: an embedded metagraph database for agent memory, in one file.</strong><br>
Statements about statements, n-ary hyperedges and nested graphs, queried with SPARQL&nbsp;1.2 and Datalog, from JavaScript and TypeScript.</p>

<p align="center">
<a href="https://graphlava.org">Website</a> ·
<a href="https://graphlava.org/book/">Mini book</a> ·
<a href="https://graphlava.org/articles/">Articles</a> ·
<a href="https://github.com/Volland/graphlava/tree/main/crates/graphlava">Rust core</a> ·
<a href="https://github.com/Volland/graphlava">Repository</a>
</p>

> **Status: pre-release.** The API is complete and tested, but nothing is published to npm yet, and the file format is not frozen: files written now may not open in the next version.

## Why graphlava

An agent's memory needs more than triples. It has to record *how sure* it is and *where it read* a fact, store a meeting of three people as one thing, and keep an episode as a graph inside its memory. graphlava stores those three [metagraph](https://graphlava.org/book/03-metagraphs.html) powers natively: every statement has a stable ID you can annotate (the RDF 1.2 reifier), hyperedges are one record with typed roles, and graphs nest as a DAG. It runs in your process, with no server, on a single file.

## Install

```sh
npm install @graphlava/node
```

Node.js 20 or newer. Prebuilt binaries for Linux (x64 and arm64, glibc; x64 musl), macOS (arm64 and x64) and Windows (x64) install as optional dependencies (`@graphlava/node-<platform>-<arch>`); there is nothing to compile.

## Quick start

```js
import { Database, iri } from '@graphlava/node';

const db = await Database.open('memory.glv', { prefixes: { '': 'http://example.org/' } });

// Write: a statement, then a statement about it.
const t = await db.submit([{ op: 'insert', s: ':alice', p: ':worksAt', o: iri(':acme') }], 'ordered');
const works = t.results[0];                                        // a bigint statement ID
await db.submit([{ op: 'annotate', id: works, p: ':confidence', o: 0.9 }]);

// Update in place: same ID, the annotation stays attached.
await db.submit([{ op: 'update', id: works, o: iri(':globex') }]);

// Query: the reifier ?r is the statement's ID.
const rows = await db.sparql(`
  SELECT ?org ?c WHERE { << :alice :worksAt ?org ~ ?r >> . ?r :confidence ?c }`);
for await (const row of rows) console.log(row.org, row.c);        // http://example.org/globex 0.9

await db.close();
```

Strings in subject, predicate and graph positions are IRIs (prefixed names use the `prefixes` you open with); in value positions a string is a string literal, so write IRIs there with `iri(':bob')`. Numbers, booleans, `Date`s and `bigint`s map to typed literals.

## SPARQL and Datalog

```js
await db.sparqlUpdate('INSERT DATA { :a :knows :b . :b :knows :c }');

const reach = await db.datalog(`
  reach(?x, ?y) :- :knows(?x, ?y) .
  reach(?x, ?z) :- reach(?x, ?y), :knows(?y, ?z) .
  ?- reach(:a, ?who) .`);
for await (const { who } of reach) console.log(who);
```

- **SPARQL 1.2** query and update, with reifiers (`<< s p o ~ ?r >>`), annotation syntax (`{| … |}`), transitive `GRAPH` over nested graphs and direct `GRAPH!`, and `SERVICE <meta:datalog>`.
- **Datalog** in Nemo-compatible syntax, with stratified negation, aggregates and `@assert` heads (`db.datalogUpdate`).
- The `prefixes` given at open apply to every query and program.

## Rows and batches

`Rows` is an async iterable of plain objects. For large results read batches of up to 1024 rows of IDs and decode only what you need:

```js
for await (const batch of (await db.sparql('SELECT ?s WHERE { ?s :knows ?o }')).batches()) {
  const ids = batch.ids('s');            // BigUint64Array
  const values = await batch.values('s'); // decoded on demand
}
```

## Transactions and durability

```js
{
  await using w = await db.write('sync');  // interactive, limited to 100 ms, rolled back if not committed
  await w.insert(':bob', ':knows', iri(':alice'));
  await w.commit();
}
```

| Durability | The promise resolves when |
| --- | --- |
| `'sync'` | the write is on disk |
| `'ordered'` (default) | its group commit is on disk; concurrent writers share one fsync |
| `'relaxed'` | it is applied; a later durable commit makes it durable |

Every call runs off the event loop (writes on a commit pool, reads on a read pool), so a `sync` commit never blocks your timers. One process owns a file: a second `Database.open` on the same path, in any worker or process, fails with `LOCKED`.

## Errors

Every failure is a `GraphlavaError` with a stable `code` (`CONFLICT`, `NOT_FOUND`, `LOCKED`, `GRAPH_CYCLE`, `REFERENCED_BY_STATEMENT`, `WRITE_TIMEOUT`, …) and typed fields:

```js
import { GraphlavaError } from '@graphlava/node';
try {
  await db.submit([{ op: 'nest', child: ':g1', parent: ':g2' }, { op: 'nest', child: ':g2', parent: ':g1' }]);
} catch (e) {
  if (e instanceof GraphlavaError && e.code === 'GRAPH_CYCLE') console.log(e.child, e.parent);
}
```

## Scripts and CLIs

`@graphlava/node/sync` offers the same API with blocking calls (`Database.openSync`, `submitSync`, `sparqlSync`, …) for scripts where an event loop is in the way.

## TypeScript

Types ship with the package (`index.d.ts`, `sync.d.ts`); IDs are `bigint`, terms are tagged unions, and every core operation is a typed `{ op: … }` object.

## Learn more

- [graphlava.org](https://graphlava.org): what graphlava is and where it is going
- [Mini book: Hypergraphs and Metagraphs in Statement Graphs](https://graphlava.org/book/)
- [Statement graphs: the one idea under graphlava](https://graphlava.org/articles/statement-graphs.html)
- [The Node.js binding design](https://github.com/Volland/graphlava/blob/main/tg.md/16-api.md) and [the Rust core](https://github.com/Volland/graphlava/tree/main/crates/graphlava)

## License

MIT OR Apache-2.0.

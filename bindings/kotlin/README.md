<p align="center"><img src="https://graphlava.org/assets/logo.svg" width="120" alt="graphlava logo"></p>

<h1 align="center">GraphLava for Kotlin and Android</h1>

<p align="center"><strong><a href="https://graphlava.org">graphlava</a> on Android and the JVM: an embedded metagraph database for agent memory, in one file.</strong><br>
Statements about statements, n-ary hyperedges and nested graphs, queried with SPARQL&nbsp;1.2 and Datalog, with coroutines and Flow.</p>

<p align="center">
<a href="https://graphlava.org">Website</a> ·
<a href="https://graphlava.org/book/">Mini book</a> ·
<a href="https://graphlava.org/articles/">Articles</a> ·
<a href="https://github.com/Volland/graphlava/tree/main/crates/graphlava-ffi">C FFI</a> ·
<a href="https://github.com/Volland/graphlava">Repository</a>
</p>

> **Status: pre-release.** Tested on the JVM host; instrumented tests run on Android emulators (including a 16 KB page-size image) in CI. Not yet published to Maven Central. The file format is not frozen yet.

## Why graphlava on Android

An on-device assistant needs a memory that keeps *how sure* it is and *where it learned* each fact, stores a meeting as one thing, and groups facts into episodes. graphlava stores those [metagraph](https://graphlava.org/book/03-metagraphs.html) structures natively in one local file, with no server, and flushes safely when Android stops or kills your process.

## Modules

| Module | What it is |
| --- | --- |
| `org.graphlava:graphlava` | The Kotlin API (JVM): coroutines, `Flow<RowBatch>`, `ULong` IDs, sealed exceptions |
| `org.graphlava:graphlava-android` | The AAR (minSdk 24): native libraries for `arm64-v8a` and `x86_64`, linked for 16 KB pages, plus `GraphLavaLifecycle` |

```kotlin
dependencies {
    implementation("org.graphlava:graphlava-android:<version>")
}
```

## Quick start

```kotlin
import org.graphlava.*

fun iri(local: String) = Term.iri("http://example.org/$local")

val db = Database.open(context.filesDir.resolve("memory.glv").path)

// Write a statement and a statement about it.
val res = db.submit(listOf(Op.insert(iri("alice"), iri("worksAt"), iri("acme"))), Durability.ORDERED)
val works = (res[0] as OpResult.Id).id!!
db.submit(listOf(Op.annotate(works, iri("confidence").v, Term.decimal("0.9").v)))
db.submit(listOf(Op.update(works, iri("globex").v)))   // same ID, annotation kept

// Query: rows arrive as a Flow of batches.
db.query("""
    PREFIX : <http://example.org/>
    SELECT ?org ?c WHERE { << :alice :worksAt ?org ~ ?r >> . ?r :confidence ?c }
""").collect { batch ->
    for (i in 0 until batch.count) println(batch.term(i, "org"))
}
db.close()
```

## Datalog

```kotlin
db.query("""
    @prefix : <http://example.org/> .
    reach(?x, ?y) :- :knows(?x, ?y) .
    reach(?x, ?z) :- reach(?x, ?y), :knows(?y, ?z) .
    ?- reach(:a, ?who) .
""", datalog = true).collect { batch -> /* … */ }
```

## Transactions, threads and durability

```kotlin
db.write(Durability.SYNC) { txn -> txn.insert(iri("bob"), iri("knows"), iri("alice")) }
val n = db.read { snap -> snap.lookup(iri("bob"))?.let { snap.find(s = it).size } ?: 0 }
```

- Every call is a `suspend` function: writes run on one writer thread, reads on a read pool, never on the main thread.
- Blocking variants (`Database.openBlocking`, `flushBlocking`) exist for workers and refuse to run on the main thread.
- `Durability.SYNC`, `ORDERED` (default; concurrent writers share one fsync) or `RELAXED`.
- Failures are a sealed `GraphLavaException`, one subclass per status code; `Database` is `AutoCloseable`.

## Android lifecycle

Install the observer once after opening: `GraphLavaLifecycle.install(context, db)`. It registers with the `ProcessLifecycleOwner` and, on `ON_STOP`, flushes the group commit, so nothing acknowledged is lost when Android later kills the process. Opening uses quick repair, so the first open after a kill takes milliseconds.

## Learn more

- [graphlava.org](https://graphlava.org) · [Mini book](https://graphlava.org/book/) · [Articles](https://graphlava.org/articles/)
- [Mobile bindings design](https://github.com/Volland/graphlava/blob/main/tg.md/16-api.md)
- The [C FFI](https://github.com/Volland/graphlava/tree/main/crates/graphlava-ffi) this binding wraps, and the [Swift package](https://github.com/Volland/graphlava/tree/main/bindings/swift) for iOS

## License

MIT OR Apache-2.0.

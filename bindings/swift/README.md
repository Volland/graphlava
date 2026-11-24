<p align="center"><img src="https://graphlava.org/assets/logo.svg" width="120" alt="graphlava logo"></p>

<h1 align="center">GraphLava for Swift</h1>

<p align="center"><strong><a href="https://graphlava.org">graphlava</a> on iOS and macOS: an embedded metagraph database for agent memory, in one file.</strong><br>
Statements about statements, n-ary hyperedges and nested graphs, queried with SPARQL&nbsp;1.2 and Datalog, behind a Swift actor.</p>

<p align="center">
<a href="https://graphlava.org">Website</a> ·
<a href="https://graphlava.org/book/">Mini book</a> ·
<a href="https://graphlava.org/articles/">Articles</a> ·
<a href="https://github.com/Volland/graphlava/tree/main/crates/graphlava-ffi">C FFI</a> ·
<a href="https://github.com/Volland/graphlava">Repository</a>
</p>

> **Status: pre-release.** Tested on the macOS host and built for the iOS simulator and devices; device and simulator test runs happen in CI. The file format is not frozen yet.

## Why graphlava on a phone

An on-device assistant needs a memory that keeps *how sure* it is and *where it learned* each fact, stores a meeting as one thing, and groups facts into episodes. graphlava stores those [metagraph](https://graphlava.org/book/03-metagraphs.html) structures natively in one local file, with no server, and survives the app being suspended or killed by iOS.

## Install

Until the package is published, build the XCFramework and add the package from a checkout:

```sh
git clone https://github.com/Volland/graphlava && cd graphlava/bindings/swift
./build-xcframework.sh          # GraphLavaFFI.xcframework: iOS device, iOS simulator (arm64, x86_64), macOS
```

Then in Xcode choose *File ▸ Add Package Dependencies ▸ Add Local…* and pick `bindings/swift`, or in a `Package.swift`:

```swift
.package(path: "../graphlava/bindings/swift")
// target dependency: .product(name: "GraphLava", package: "swift")
```

The XCFramework carries a module map and a privacy manifest. iOS 15 and macOS 12 or newer.

## Quick start

```swift
import GraphLava

let ex = { (local: String) in Term.iri("http://example.org/\(local)") }
let db = try await Database.open(url)

// Write a statement and a statement about it.
let results = try await db.submit([.insert(ex("alice"), ex("worksAt"), ex("acme"))], durability: .ordered)
guard case let .id(works?) = results[0] else { return }
try await db.submit([.annotate(id: works, ex("confidence").v, Term.decimal("0.9").v)])
try await db.submit([.update(id: works, to: ex("globex").v)])   // same ID, annotation kept

// Query: rows arrive as an AsyncSequence of batches.
for try await batch in try await db.query("""
    PREFIX : <http://example.org/>
    SELECT ?org ?c WHERE { << :alice :worksAt ?org ~ ?r >> . ?r :confidence ?c }
    """) {
    for i in 0..<batch.count { print(try batch.term(row: i, column: "org")) }
}
try await db.close()
```

## Datalog

```swift
for try await batch in try await db.query("""
    @prefix : <http://example.org/> .
    reach(?x, ?y) :- :knows(?x, ?y) .
    reach(?x, ?z) :- reach(?x, ?y), :knows(?y, ?z) .
    ?- reach(:a, ?who) .
    """, datalog: true) { /* … */ }
```

## Transactions and reads

```swift
try await db.write(.sync) { txn in                  // committed when the closure returns, aborted if it throws
    _ = try txn.insert(ex("bob"), ex("knows"), ex("alice"))
}
let friends = try await db.read { snap in           // one consistent snapshot
    try snap.lookup(ex("bob")).map { try snap.out($0).count } ?? 0
}
```

`Durability` is `.sync`, `.ordered` (default; concurrent writers share one fsync) or `.relaxed`. IDs are `UInt64`, terms are the `Term` enum (`.iri`, `.blank`, `.literal`, `.triple`), and failures are `GraphLavaError`, one case per status code.

## The app lifecycle

`Database` installs a `LifecycleObserver` by default. When the app moves to the background it suspends the store inside a `beginBackgroundTask`: open snapshots end, an interactive write is aborted, the group commit is flushed, and for a file in an app-group container the file lock is released, so iOS never kills the app for holding a lock (`0xdead10cc`). On return to the foreground it resumes. Opening uses quick repair, so the first open after the app was killed takes milliseconds.

## App extensions

An app extension must not open the database (it would contend for the lock with the app). Use `ExtensionClient` instead: it sends writes to the running app over XPC, or appends them to a CRC-checked journal in the app-group container that the app applies exactly once on its next open.

```swift
let client = ExtensionClient(appGroupContainer: groupURL, extensionID: "share")
try client.submit(sparqlUpdate: "PREFIX : <http://example.org/> INSERT DATA { :n :saved true }")
```

## Learn more

- [graphlava.org](https://graphlava.org) · [Mini book](https://graphlava.org/book/) · [Articles](https://graphlava.org/articles/)
- [Mobile bindings design](https://github.com/Volland/graphlava/blob/main/tg.md/16-api.md) and [ADR 0043: iOS extension write journal](https://github.com/Volland/graphlava/blob/main/docs/adr/0043-ios-extension-write-journal.md)
- The [C FFI](https://github.com/Volland/graphlava/tree/main/crates/graphlava-ffi) this package wraps, and the [Kotlin binding](https://github.com/Volland/graphlava/tree/main/bindings/kotlin) for Android

## License

MIT OR Apache-2.0.

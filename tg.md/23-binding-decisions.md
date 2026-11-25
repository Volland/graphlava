---
type: Decision
status: accepted
specs: ["adr:0002", "adr:0011", "adr:0026", "adr:0042"]
---
# Binding decisions

Decisions the host-language bindings make within [[adr:0042]]: each section gives the choice, the rejected alternatives and the reasons.

The Python decisions come from the `v1-python-binding` change (design D1–D5); later bindings add their own sections here.

## Python binds the Rust API with PyO3

`crates/graphlava-py` depends on `graphlava` as a Rust crate and builds with maturin as an abi3-py310 extension, plus a version-specific free-threaded 3.14t build.

- motivated_by:: [[adr:0042]] {weight: primary}
- motivated_by:: [[24-binding-constraints#CPython stable ABI]]
- cites:: [[16-api#Rust API]] {intent: usesMethodIn}

<option id="py-pyo3" outcome="chosen">

PyO3 over the Rust API: one marshalling step, panic and ownership handling come with PyO3, and the Rust types stay visible to the binding.

</option>

<option id="py-cffi" outcome="rejected">

cffi over the C FFI: results are marshalled twice, and PyO3's panic and ownership handling would have to be written again by hand.

</option>

<option id="py-uniffi" outcome="rejected">

UniFFI: no Node backend, and a second ABI next to the C ABI.

</option>

## Python releases the GIL around core work

Every call that does I/O, parses, plans, executes or waits on a commit runs inside `Python::detach`, and the module declares `gil_used = false` for free-threaded CPython.

- motivated_by:: [[24-binding-constraints#Free-threaded CPython]] {weight: primary}
- cites:: [[16-api#^api-no-async-required]] {intent: usesConclusionsFrom}

<option id="py-hold-gil" outcome="rejected">

Holding the GIL: one slow `sync` commit would stall every Python thread.

</option>

<option id="py-asyncio" outcome="deferred">

An asyncio API in v1: an asyncio facade can wrap the threaded API later without core changes.

</option>

## Python interactive writes keep the writer time limit

`with db.write(...)` enforces the Rust time limit unchanged (default 100 ms). Documentation and examples lead with `db.submit(...)` and compare-and-set, and `WriteTimeoutError` names `submit` in its message.

- motivated_by:: [[adr:0026]] {weight: primary}
- motivated_by:: [[adr:0042]]
- cites:: [[16-api#^api-write-in-process]] {intent: usesConclusionsFrom}

<option id="py-longer-limit" outcome="rejected">

A longer limit for Python: a slow callback would hold the single writer and stall group commit for every client.

</option>

## Python IDs are int and values round-trip exactly

IDs are unsigned 64-bit `int`s (the `Id` subclass of `int` marks an ID in object position). Integers inside ±2^57 go inline; larger integers become dictionary `xsd:integer` literals and come back as equal `int`s.

- motivated_by:: [[adr:0011]] {weight: primary}
- cites:: [[02-id-encoding#Bit layout]] {intent: usesConclusionsFrom}

<option id="py-truncate-i64" outcome="rejected">

Truncating to i64: it would silently lose `2**70` and turn tagged IDs negative.

</option>

## Python detects fork instead of sharing handles

A handle records the PID that opened it, and any call from another PID raises `ForkedHandleError` before it touches the file. Objects inherited by a forked child are never flushed or closed there.

- motivated_by:: [[adr:0002]] {weight: primary}
- cites:: [[16-api#^api-single-owner]] {intent: usesConclusionsFrom}

<option id="py-reopen-in-child" outcome="rejected">

Reopening the file silently in the child: it breaks the single owner.

</option>

<option id="py-at-fork-close" outcome="rejected">

An `os.register_at_fork` hook that closes the parent's handle: it breaks the parent.

</option>

## Node runs database work on its own threads

The Node decisions come from the `v1-node-binding` change (design D2–D5; D1, napi-rs over the Rust API, is [[adr:0042]]). Every call that can do I/O, run a query or wait on a commit returns a Promise and runs off the event loop.

- motivated_by:: [[24-binding-constraints#Node.js event loop]] {weight: primary}
- motivated_by:: [[adr:0042]]
- cites:: [[16-api#^api-no-async-required]] {intent: usesConclusionsFrom}

Reads, queries and `open` run on a read pool sized from the available parallelism. One-shot requests (`submit`, `sparqlUpdate`, `datalogUpdate`) run on a commit pool, so concurrent `ordered` requests wait for one group commit together while the core's writer token orders them; an interactive `WriteTxn` and `admin()` run in call order on one writer thread per database. Blocking variants exist only under `@graphlava/node/sync`.

<option id="node-sync-default" outcome="rejected">

Synchronous calls by default, as in better-sqlite3: fine for SQLite point reads, but a `sync` commit takes milliseconds and would freeze every timer and socket.

</option>

<option id="node-libuv-pool" outcome="rejected">

napi `AsyncTask` on libuv's default pool of 4 threads: shared with `fs` and `dns`, it starves under load.

</option>

<option id="node-writer-thread-submit" outcome="rejected">

One-shot requests on the single writer thread: each `ordered` request would wait for its own group commit before the next started, so concurrent callers never share a batch (measured: 8 callers, `ordered` p50 56 ms instead of 7 ms on a development machine).

</option>

## Node write limit runs across awaits

The interactive writer time limit (default 100 ms) starts when `write()` resolves and keeps running while JavaScript awaits other work.

At the deadline the writer thread aborts the transaction and serves queued requests. `await using` aborts on scope exit without `commit()`, and `WRITE_TIMEOUT` names `submit`.

- motivated_by:: [[adr:0026]] {weight: primary}
- motivated_by:: [[24-binding-constraints#Explicit resource management]]
- cites:: [[16-api#^api-write-in-process]] {intent: usesConclusionsFrom}

<option id="node-pause-clock" outcome="rejected">

Pausing the clock while JavaScript runs: one request could then hold the single writer for as long as it chose.

</option>

## Node IDs are bigint and integers are numbers only when safe

IDs are always `bigint` (columns are `BigUint64Array`); an `xsd:integer` result is a `number` inside ±(2^53−1) and a `bigint` outside it.

A `number` input is `xsd:integer` when it is a safe integer and `xsd:double` otherwise; an unsafe integer or `NaN` is rejected.

- motivated_by:: [[24-binding-constraints#JavaScript numbers]] {weight: primary}
- cites:: [[02-id-encoding#Bit layout]] {intent: usesConclusionsFrom}

<option id="node-string-ids" outcome="rejected">

String IDs: an allocation on every call, and `===` bugs between `"1"` and `1n`.

</option>

<option id="node-always-bigint" outcome="rejected">

Always returning `bigint` for integers: it breaks arithmetic in typical agent code (`1n + 1` throws).

</option>

## Node keeps one handle per file per process

A process-global registry of canonical paths in the addon makes a second `open` of a file in the same process, from any worker thread, reject with `LOCKED`.

Handles are marked uncloneable, and a collected handle flushes its batch and releases the lock in its finalizer.

- motivated_by:: [[24-binding-constraints#Node worker threads]] {weight: primary}
- motivated_by:: [[adr:0002]]
- cites:: [[16-api#^api-single-owner]] {intent: usesConclusionsFrom}

<option id="node-shared-handle" outcome="deferred">

Handing workers a shared handle: feasible, but it hides which thread owns the file and complicates close order; revisit if users ask.

</option>

## Suspend is a core operation

Suspend and resume live in the core; the mobile bindings only wire them to the app lifecycle (`v1-mobile-bindings`, design D2–D5).

`Database::suspend` makes new calls wait, aborts the open interactive write, flushes the batch, waits for snapshots to end and, for an app-group file, drops the engine handle to release the lock; `resume` reopens it. The bindings only wire it to the app lifecycle: Swift inside `beginBackgroundTask`, Kotlin with a flush on `ON_STOP` that keeps the lock.

- motivated_by:: [[07-platforms#iOS]] {weight: primary}
- motivated_by:: [[07-platforms#Android]]
- cites:: [[16-api#^api-ios-suspend]] {intent: usesConclusionsFrom}

<option id="mobile-suspend-core" outcome="chosen">

Suspend and resume in the core, exposed as `graphlava_suspend` and `graphlava_resume`: the plan cache and statistics survive, and resume costs one lock acquire.

</option>

<option id="mobile-close-reopen" outcome="rejected">

Close and reopen the database: loses the plan cache and the statistics and costs a full open.

</option>

<option id="mobile-commit-open-write" outcome="rejected">

Commit the open interactive write on suspend: its caller has not finished it.

</option>

## Mobile threading is an actor in Swift and dispatchers in Kotlin

Database calls never run on the main thread or the shared cooperative pool.

Swift's `Database` is an `actor` whose executor is one dedicated thread, so blocking FFI calls never occupy the cooperative pool; `suspend` and `resume` are non-isolated so they run while the actor is busy. Kotlin runs writes on a single-thread dispatcher and reads on a pool sized to the core count; blocking variants refuse the main thread in debuggable apps.

- motivated_by:: [[24-binding-constraints#Android main thread]] {weight: primary}
- cites:: [[16-api#^api-kotlin-threads]] {intent: usesConclusionsFrom}
- cites:: [[16-api#^api-no-async-required]] {intent: usesConclusionsFrom}

<option id="mobile-sync-api" outcome="rejected">

A synchronous API: it blocks the main thread and causes ANRs on Android.

</option>

## iOS extensions write through XPC or a journal

`ExtensionClient` uses XPC when it holds the running app's endpoint, else a per-extension journal applied exactly once on the owner's next open ([[adr:0043]]).

- motivated_by:: [[adr:0043]] {weight: primary}
- motivated_by:: [[07-platforms#iOS]]
- motivated_by:: [[24-binding-constraints#App extension limits]]
- cites:: [[16-api#^api-ios-extension-ipc]] {intent: usesConclusionsFrom}

<option id="mobile-journal" outcome="chosen">

XPC, else a per-extension journal with CRC-32C frames, sequence numbers committed with each request, torn-tail drop and truncation.

</option>

<option id="mobile-extension-fail" outcome="rejected">

Fail the write: extensions lose data.

</option>

## Mobile IDs are unsigned and errors map one-to-one

Swift uses `UInt64` IDs and a `GraphLavaError` enum; Kotlin uses `ULong` IDs and a sealed `GraphLavaException` hierarchy. Both map one-to-one from `graphlava_status` codes.

- motivated_by:: [[adr:0042]] {weight: primary}
- cites:: [[02-id-encoding#Bit layout]] {intent: usesConclusionsFrom}

<option id="mobile-signed-ids" outcome="rejected">

Signed `Int64` / `Long`: every inline-literal ID, whose tag sets the top bit, turns negative.

</option>

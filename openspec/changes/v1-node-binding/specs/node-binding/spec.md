# Spec Delta

## Purpose

The npm package `@graphlava/node` (scoped under the `graphlava` npm organization, published with public access): a napi-rs addon over the Rust API that gives Node.js and TypeScript agent code the data model, transactions, durability levels and query surfaces without blocking the event loop. Design: `tg.md/16-api.md`, this change's `design.md`.

## ADDED Requirements

### Requirement: Node API parity
The Node package SHALL expose `Database.open`, `read`, `write`, `submit`, `sparql`, `datalog` and `admin`, the core reads (`props`, `out`, `inc`, `stmt`, `hedge`, `lookup`), and every `CoreOp` as a plain object tagged by `op`. Each call SHALL have the same effect and return the same IDs and rows as the Rust call it wraps.

Formal: none — marshalling layer; the effect is the Rust call's, covered by the store and query requirements

#### Scenario: Agent memory session from Node
- **Level**: e2e
- **WHEN** a Node test opens `memory.glv` in a temp dir and calls `await db.submit([{op: "insert", s: ":alice", p: ":knows", o: ":bob"}], "ordered")`. It then annotates the returned statement with `:confidence 0.9`, closes and reopens the file, and runs the reifier query for `:confidence`
- **THEN** one row with `0.9` is returned, and the statement ID equals the `bigint` in the `submit` result from before the reopen

#### Scenario: Oracle trace replay through Node
- **Level**: refinement
- **WHEN** the oracle traces for seeds 1..16, with 2,000 operations each, are replayed through the Node API
- **THEN** every operation result and the final state match the Rust replay of the same trace

#### Scenario: Malformed core operation
- **Level**: unit
- **WHEN** `submit([{op: "insert", s: ":a", p: ":b"}], "sync")` omits `o`
- **THEN** the Promise rejects with a `GraphlavaError` whose `code` is `"INVALID"` and whose message names the missing field, and nothing is written

### Requirement: Event loop is never blocked
Every operation that can do I/O, wait on a commit or run a query SHALL return a Promise and SHALL run on a worker thread outside the libuv event loop. Synchronous variants SHALL exist only for in-memory metadata (`version`, `isOpen`) and under a separate `@graphlava/node/sync` entry point that is documented as blocking.

Formal: none — host runtime scheduling, outside the model

#### Scenario: Timers keep firing during a sync commit
- **Level**: e2e
- **WHEN** a 1 ms `setInterval` runs while 200 `sync` commits are awaited one after another
- **THEN** the largest gap between interval callbacks stays under 20 ms (p99), and all 200 commits survive a reopen

#### Scenario: Sync entry point is opt-in
- **Level**: unit
- **WHEN** `import { Database } from "@graphlava/node"` is used and the code calls `db.readSync`
- **THEN** the property is `undefined`; it exists only on `@graphlava/node/sync`

### Requirement: Interactive write transactions in Node
`await db.write(durability)` SHALL return a `WriteTxn` whose methods return Promises and run in order on the writer thread. `await using w = await db.write("sync")` SHALL abort the transaction if the scope exits without `commit()`. The writer time limit of the Rust API SHALL apply from the moment `write()` resolves, including time spent in JavaScript between awaits.

Formal: none — scoping sugar over `commit` and `abort`, whose atomicity is `MetaCore.Durability`

#### Scenario: Scope exit without commit aborts
- **Level**: e2e
- **WHEN** an `await using` block inserts 3 statements and then throws
- **THEN** the error propagates, and after reopening none of the 3 statements exist

#### Scenario: Time limit across awaits
- **Level**: e2e
- **WHEN** code holds a `WriteTxn`, awaits a 150 ms `setTimeout` with the default 100 ms limit, and then inserts
- **THEN** the insert rejects with `code === "WRITE_TIMEOUT"`, and a concurrent `submit` commits

#### Scenario: Node process killed after a sync commit
- **Level**: crash
- **WHEN** a Node child process awaits a `sync` commit, writes the returned ID to stdout, and is killed with SIGKILL
- **THEN** after reopening, the statement with that ID exists

### Requirement: Node ID mapping
IDs SHALL be `bigint` holding the full unsigned 64-bit value, in arguments and in results.

Formal: none — host type conversion; the ID layout is proved in `MetaCore.Id`

#### Scenario: Tagged IDs round trip
- **Level**: e2e
- **WHEN** a hyperedge ID with kind tag bits 60–63 set is returned, the file is reopened, and the ID is passed back to `hedge()`
- **THEN** the same hyperedge is returned, and `typeof id === "bigint"` and `id < 2n**64n`

#### Scenario: Number passed as an ID
- **Level**: unit
- **WHEN** `stmt(42)` is called with a `number`
- **THEN** the call rejects with `code === "INVALID"` and asks for a `bigint`

### Requirement: Node literal mapping
The package SHALL map `string`, `boolean`, `bigint`, `number` and `Date` to their XSD datatypes; a `number` is `xsd:integer` when it is a safe integer and `xsd:double` otherwise. Other terms are tagged objects. Integer results SHALL be `number` inside ±(2^53−1) and `bigint` outside it. Unsafe-integer numbers and `NaN` SHALL be rejected.

Formal: none — host type conversion; inline routing itself is proved in `MetaCore.Id`

#### Scenario: Integers across the safe and inline limits
- **Level**: e2e
- **WHEN** `2n**53n - 1n`, `2n**57n - 1n`, `-(2n**57n)` and `2n**57n` are stored as `:n` properties, the file is reopened and they are read back
- **THEN** the first comes back as `number` 9007199254740991 and the other three as equal `bigint` values

#### Scenario: Unsafe number is rejected
- **Level**: unit
- **WHEN** `2 ** 60` is passed as a `number` object
- **THEN** the call rejects with `code === "INVALID"` and suggests passing a `bigint`

### Requirement: Single owner across workers and processes
A `Database` handle SHALL NOT be transferable to a `worker_thread`. `Database.open` of a file already open in the same process, whether from a worker or from the main thread, SHALL reject with `code === "LOCKED"`, and so SHALL `Database.open` from another process. `db.close()` SHALL release the lock. A handle that is garbage-collected without `close()` SHALL release the lock in its finalizer, after flushing a pending group commit.

Formal: none — OS process and lock behaviour, outside the model

#### Scenario: Worker thread is refused
- **Level**: e2e
- **WHEN** the main thread holds `memory.glv` open and a `Worker` calls `Database.open` on it
- **THEN** the worker's Promise rejects with `LOCKED`, and the main thread's next commit succeeds

#### Scenario: Close releases the lock
- **Level**: e2e
- **WHEN** a process opens, commits with `ordered`, awaits `close()`, and a second process then opens the file
- **THEN** the second open succeeds and sees the commit

#### Scenario: Posting a handle to a worker
- **Level**: unit
- **WHEN** `worker.postMessage(db)` is called
- **THEN** it throws `DataCloneError`, and the handle stays usable on the main thread

### Requirement: Node row streaming
Query results SHALL be an async iterable of rows. `rows.batches()` SHALL yield batches of up to 1024 rows, with each ID column as a `BigUint64Array`, and terms SHALL be decoded lazily per column. Breaking out of iteration SHALL release the read snapshot.

Formal: none — result transport; row content is defined by the query requirements

#### Scenario: Typed-array batches
- **Level**: e2e
- **WHEN** a query that returns 3,000 rows is read through `batches()`
- **THEN** three batches arrive with 1024, 1024 and 952 rows, and the ID columns are `BigUint64Array`

#### Scenario: Early break releases the snapshot
- **Level**: e2e
- **WHEN** a `for await` loop breaks after the first row of a 10,000-row result
- **THEN** the debug-build counter `graphlava._debug.openSnapshots()` returns 0 after the loop

### Requirement: Node errors
Errors SHALL be instances of `GraphlavaError` with a stable string `code`. Each Rust `Error` variant SHALL have its own code, for example `LOCKED`, `CONFLICT`, `WRITE_TIMEOUT`, `NOT_FOUND`, `GRAPH_CYCLE`, `FILLS_ROLE`, `CORRUPTED` and `INVALID`, and SHALL carry the variant's fields as `bigint` properties. A Rust panic SHALL reject with `code === "INTERNAL"` and SHALL NOT crash the process.

Formal: none — error translation

#### Scenario: Graph cycle error carries IDs
- **Level**: e2e
- **WHEN** `:g2` is nested under `:g1`, and the code then nests `:g1` under `:g2`
- **THEN** the call rejects with `GRAPH_CYCLE`, and `child` and `parent` hold the two graph IDs, and the nesting is unchanged after reopening

#### Scenario: Nesting depth limit
- **Level**: e2e
- **WHEN** a chain of 64 nested graphs is built and a 65th level is added
- **THEN** the call rejects with `GRAPH_DEPTH_EXCEEDED` and `limit === 64`

### Requirement: Node packaging
The package SHALL ship prebuilt N-API 9 binaries for linux-x64-gnu, linux-arm64-gnu, linux-x64-musl, darwin-arm64, darwin-x64 and win32-x64-msvc. It SHALL support Node.js 20, 22 and 24, ESM and CommonJS, and `.d.ts` declarations that pass `tsc --strict`. On any other platform, loading SHALL fail with an error naming the platform.

Formal: none — packaging

#### Scenario: Install and smoke test per target
- **Level**: e2e
- **WHEN** CI runs `npm install @graphlava/node` from the packed tarball on each target and Node version and runs the agent memory session
- **THEN** it passes on every combination

#### Scenario: Unsupported platform
- **Level**: unit
- **WHEN** the package is installed on `freebsd-x64`
- **THEN** loading it throws an error naming `freebsd-x64` and listing the supported targets

### Requirement: Node call overhead
The overhead the Node binding adds to an awaited `props()` point lookup SHALL be measured as p50 and p99 in microseconds above the same Rust call, together with event-loop delay. The dataset is 1M statements, setup durability is `ordered`, and the hardware is the fixed Linux x86_64 nightly runner. Pass line: to be measured in the first run, then gated at +20% over that baseline. Benchmark: `tg.md/17-benchmarks.md#Binding overhead`.

Formal: none — performance

#### Scenario: Overhead benchmark runs nightly
- **Level**: benchmark
- **WHEN** the nightly runner runs `Binding overhead` for Node
- **THEN** p50 and p99 overhead and `monitorEventLoopDelay` p99 are recorded, and the job fails if p50 exceeds the stored baseline by more than 20% at 95% confidence

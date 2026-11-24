---
type: Requirement
status: draft
priority: must
specs: ["adr:0002", "adr:0012", "adr:0013", "adr:0014", "adr:0018"]
---
# API

The ways applications reach meta-core: the Rust embedding API (v0), a C FFI (v0), Swift and Kotlin bindings (v1), and the `meta-core-server` crate with SPARQL Protocol and a Datalog endpoint (v1).

- follows_from:: [[adr:0002]]
- follows_from:: [[adr:0014]]
- depends_on:: [[06-transactions-durability#Durability levels]] {strength: hard}
- depends_on:: [[11-sparql#Scope]] {strength: hard}
- depends_on:: [[13-datalog#Surface syntax]] {strength: hard}
- introduced_by:: [[18-roadmap#v0]]

## Purpose

One owner process opens the file ([[adr:0002]]); every API is either in-process in that owner or a client talking to it over IPC. The API exposes queries, updates, prepared queries, transactions with a durability level, and administration.

<non-goal>

Opening one file from two processes. A second process that calls `open` on a file held by another owner gets an error, not shared access.

</non-goal>

## Rust API

The native embedding API. Every other binding wraps it.

- depends_on:: [[14-planner-execution#Plan cache]] {strength: soft}
- measured_by:: [[17-benchmarks#Durability commit latency]]

```rust
pub struct Database;            // the owner handle; Send + Sync
pub struct ReadTxn<'db>;        // snapshot; many at once
pub struct WriteTxn<'db>;       // one at a time
pub enum Durability { Sync, Ordered, Relaxed }

impl Database {
    pub fn open(path: &Path, opts: OpenOptions) -> Result<Database>;
    pub fn read(&self) -> Result<ReadTxn<'_>>;
    pub fn write(&self, durability: Durability) -> Result<WriteTxn<'_>>;
    pub fn prepare(&self, lang: Lang, text: &str) -> Result<Prepared>;
    pub fn sparql(&self, text: &str) -> Result<Prepared>;    // = prepare(Lang::Sparql, ..)
    pub fn datalog(&self, program: &str) -> Result<Prepared>; // = prepare(Lang::Datalog, ..)
    pub fn admin(&self) -> Admin<'_>;   // sweep, build/drop orderings, retrain, export
}

impl ReadTxn<'_> {
    pub fn query(&self, q: &Prepared, params: &Params) -> Result<Rows>;
}

impl WriteTxn<'_> {
    pub fn query(&self, q: &Prepared, params: &Params) -> Result<Rows>;   // read-your-writes
    pub fn update(&mut self, q: &Prepared, params: &Params) -> Result<UpdateStats>;
    pub fn commit(self) -> Result<CommitTicket>;   // resolves when the durability level is met
    pub fn abort(self);
}

pub enum Lang { Sparql, Datalog, Cypher /* v1 */ }
```

<rule id="api-durability-per-txn" strength="must">

Every write transaction carries one durability level chosen at `write()`. `commit()` returns when the level's guarantee holds: immediately durable for `Sync`, after its group commit is durable for `Ordered`, after the in-memory commit for `Relaxed` ([[06-transactions-durability#Group commit]]).

</rule>

<rule id="api-datalog-surface" strength="must">

Datalog is a standalone query surface in v0, equal to SPARQL: `Database::datalog(program)` (and `prepare(Lang::Datalog, ..)`) accepts a Datalog program in the surface syntax of [[13-datalog#Surface syntax]] with one or more goals, returns a `Prepared` that runs in a `ReadTxn` or `WriteTxn`, and yields `Rows` like a SPARQL query. Rule declarations in the program are scoped to the query unless installed through `Admin`.

</rule>

<rule id="api-rows-late" strength="must">

`Rows` yields IDs and decodes terms lazily per column on access, so a caller that only needs IDs or counts never pays for dictionary decoding ([[14-planner-execution#Vectorized execution]]).

</rule>

<rule id="api-single-owner" strength="must">

`Database::open` takes an exclusive OS lock on the file for the lifetime of the handle. A second `open` in any process fails with `Error::Locked`.

</rule>

<rule id="api-no-async-required" strength="should">

The core API is synchronous. An `async` facade (for the server crate) wraps it by running transactions on a dedicated thread pool; the core crate does not depend on an async runtime.

</rule>

## C FFI

A stable C ABI over the Rust API, used by Swift, Kotlin and other languages. Handles are opaque pointers; errors are codes plus a message.

- depends_on:: [[16-api#Rust API]] {strength: hard}

<rule id="api-ffi-abi" strength="must">

The C header is generated (cbindgen) and versioned. Every function returns an `mc_status` code; outputs go through out-parameters. No Rust panic crosses the FFI boundary; panics become `MC_ERR_INTERNAL`.

</rule>

<rule id="api-ffi-rows" strength="must">

Results cross the FFI as column batches of `u64` IDs plus an on-demand term decoder, matching the internal batch size of 1024 rows, so bindings can avoid per-row calls.

</rule>

| Function group | Examples |
| --- | --- |
| lifecycle | `mc_open`, `mc_close`, `mc_suspend`, `mc_resume` |
| transactions | `mc_read_begin`, `mc_write_begin(durability)`, `mc_commit`, `mc_abort` |
| queries | `mc_prepare(lang, text)` with `lang` = `MC_LANG_SPARQL` or `MC_LANG_DATALOG`, `mc_query(txn, prepared, params)`, `mc_rows_next_batch`, `mc_term_decode` |
| admin | `mc_sweep`, `mc_export_snapshot` |

## Mobile bindings

Swift and Kotlin wrappers over the C FFI, with the app-lifecycle hooks iOS and Android need. Arrive in v1.

- depends_on:: [[16-api#C FFI]] {strength: hard}
- cites:: [[07-platforms#iOS]] {intent: citesAsAuthority}
- cites:: [[07-platforms#Android]] {intent: citesAsAuthority}
- introduced_by:: [[18-roadmap#v1]]
- measured_by:: [[17-benchmarks#Mobile]]

<rule id="api-ios-suspend" strength="must">

The Swift binding calls `mc_suspend` when the app moves to the background. Suspend finishes or aborts the open write transaction, flushes a pending group commit, ends all read snapshots and releases the file lock, so the app is never killed for holding a lock on a shared container file (`0xdead10cc`). `mc_resume` re-acquires the lock and re-opens snapshots lazily.

</rule>

<rule id="api-ios-extension-ipc" strength="should">

An iOS app extension does not open the database file. It reaches the owner (the containing app) through an XPC service or, when the app is not running, queues writes in a small append-only file in the app group that the owner applies on its next start.

</rule>

<rule id="api-kotlin-threads" strength="must">

The Kotlin binding runs every database call on a dedicated executor, never on the main thread, and exposes results as `Flow` batches.

</rule>

## Server crate

`meta-core-server`: a separate crate that hosts the owner process and exposes SPARQL Protocol, a Datalog endpoint and a local IPC socket. Arrives in v1; HTTP dependencies stay out of the core crate.

- follows_from:: [[adr:0014]]
- depends_on:: [[11-sparql#Datalog service]] {strength: hard}
- depends_on:: [[13-datalog#Surface syntax]] {strength: hard}
- depends_on:: [[11-sparql#Conformance]] {strength: soft}
- introduced_by:: [[18-roadmap#v1]]

| Endpoint | Method | Content |
| --- | --- | --- |
| `/sparql` | GET, POST | SPARQL 1.2 query (SPARQL 1.1 Protocol), results as SPARQL JSON, XML, CSV/TSV or N-Triples/Turtle for `CONSTRUCT` |
| `/update` | POST | SPARQL 1.2 Update, with header `Meta-Durability: sync`, `ordered` or `relaxed` |
| `/datalog` | POST | a Datalog program with goals, in the syntax of [[13-datalog#Surface syntax]]; results as SPARQL JSON; same durability header for programs that write |
| local socket | framed binary | the C FFI operations over IPC, for other processes on the same machine |

<rule id="api-server-protocol" strength="must">

`/sparql` and `/update` conform to the SPARQL 1.1 Protocol and pass the W3C protocol tests, with RDF 1.2 result formats where defined.

</rule>

<rule id="api-server-auth" strength="must">

The server listens on loopback and the local socket by default. Any non-loopback listener requires an authentication method to be configured; the server refuses to start otherwise.

</rule>

<rule id="api-server-remote-service" strength="should">

The server crate provides the HTTP `ServiceResolver` for remote `SERVICE` calls, with per-call timeouts and batched bind-joins ([[11-sparql#Datalog service]]).

</rule>

## Failure behaviour

How API calls behave on errors and crashes.

<rule id="api-commit-ticket" strength="must">

If the owner process dies before a commit's durability level is met, the caller gets an error (in process) or a closed connection (over IPC), never a success. A `Relaxed` commit reported as successful may still be lost by a crash, as its level allows.

</rule>

## Platform notes

The Rust and C APIs build for all five platforms. Swift bindings ship as an XCFramework, Kotlin bindings as an AAR with arm64-v8a and x86_64 libraries built for 16 KB page alignment ([[07-platforms#Android]]).

## Test plan

API tests across bindings.

- Lock test: two `open` calls on one file from two processes; the second gets `Locked`.
- Suspend test (iOS simulator and device): background the app during a write and a group commit; no `0xdead10cc` and no lost acknowledged commit.
- FFI fuzz: random call sequences over the C API, no crash and no leak under sanitizers.
- Server: W3C SPARQL 1.1 Protocol tests.

## Open items

Questions this spec leaves open; each is an `OpenQuestion` in [[open-questions]].

- [[open-questions#Extension write queue]]: whether iOS extensions may queue writes in a side file when the app is not running, or must fail.
- [[open-questions#Server authentication]]: which authentication methods the server crate supports first.

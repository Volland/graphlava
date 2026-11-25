---
type: Requirement
openspec: [python-binding, node-binding, mobile-bindings]
status: draft
priority: must
specs: ["adr:0002", "adr:0012", "adr:0013", "adr:0014", "adr:0018", "adr:0025", "adr:0026", "adr:0042", "adr:0043"]
---
# API

The ways applications reach graphlava: the Rust embedding API (v0), a C FFI (v0), Python and Node.js bindings (v1), Swift and Kotlin bindings (v1), and the `graphlava-server` crate with SPARQL Protocol and a Datalog endpoint (v1).

- follows_from:: [[adr:0002]]
- follows_from:: [[adr:0014]]
- follows_from:: [[adr:0026]]
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
    pub fn write(&self, durability: Durability) -> Result<WriteTxn<'_>>;   // in-process only, time-limited
    pub fn submit(&self, req: WriteRequest, durability: Durability) -> Result<CommitTicket>; // one-shot
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

<rule id="api-write-in-process" strength="must">

An interactive `WriteTxn` exists only inside the owner process and holds the writer for at most a configured time (default 100 ms, measured from `write()`); past it, further calls fail with `WriteTimeout` and the transaction aborts. Other processes never hold the writer ([[adr:0026]]).

</rule>

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

## Core API

Typed operations on the data model that bypass the parser, planner and plan cache. They serve the point lookups and small writes that dominate agent memory at storage speed, and every query surface lowers to the same operations ([[adr:0026]]).

- depends_on:: [[01-data-model#Statements#Operations]] {strength: hard}

```rust
impl ReadTxn<'_> {   // also on WriteTxn
    pub fn props(&self, node: Id) -> Result<Props>;                         // one record read
    pub fn out(&self, s: Id, p: Option<Id>) -> Result<Edges>;               // SPO prefix
    pub fn inc(&self, o: Id, p: Option<Id>) -> Result<Edges>;               // POS or OSP prefix
    pub fn stmt(&self, id: Id) -> Result<Option<Triple>>;                   // one `stmt` read
    pub fn hedge(&self, e: Id) -> Result<Option<Hyperedge>>;
    pub fn lookup(&self, term: &Term) -> Result<Option<Id>>;                // no interning
}

pub enum CoreOp {
    Insert { s: TermRef, p: TermRef, o: TermRef, graph: Option<Id> },
    Set { s: TermRef, p: TermRef, o: TermRef },
    Update { id: Id, o: TermRef, expected: Option<TermRef> },               // compare-and-set
    Annotate { id: Id, p: TermRef, o: TermRef },
    Delete { id: Id, mode: DeleteMode },                                    // cascade, KEEP, DETACH
    CreateHedge { ty: Id, roles: Vec<(Id, TermRef)>, attrs: Vec<(Id, TermRef)> },
    AddMember { item: Id, graph: Id },
    RemoveMember { item: Id, graph: Id },
}
```

<rule id="api-core-equivalence" strength="must">

A core operation has exactly the effect of the IR `Write` operation it names ([[10-ir#Operators#Write operators]]), and a core read returns the same rows as the corresponding SPARQL pattern. The property test of [[01-data-model#Test plan]] runs through this API.

</rule>

## Write requests

How writes reach the single writer from outside the owner process: as one-shot requests that run to completion inside one batch ([[06-transactions-durability#Group commit]]).

- follows_from:: [[adr:0026]]
- follows_from:: [[adr:0025]]

| Request | Content |
| --- | --- |
| `Update` | SPARQL Update text and parameters |
| `Program` | a Datalog program that writes |
| `Ops` | a list of core operations, applied in order, all or nothing |

<rule id="api-one-shot" strength="must">

A write request carries everything it needs; it runs in the writer without waiting for its client, and its result returns with the commit acknowledgment. Read-modify-write across round trips uses `Update { expected }` (compare-and-set) and retries on `Conflict`.

</rule>

## C FFI

A stable C ABI over the Rust API, used by Swift, Kotlin and other languages. Handles are opaque pointers; errors are codes plus a message.

The crate is `graphlava-ffi` (`libgraphlava_ffi`, cdylib and staticlib); every symbol carries the `graphlava_` prefix.

- depends_on:: [[16-api#Rust API]] {strength: hard}

<rule id="api-ffi-abi" strength="must">

The C header is generated (cbindgen) and versioned. Every function returns a `graphlava_status` code; outputs go through out-parameters. No Rust panic crosses the FFI boundary; panics become `GRAPHLAVA_ERR_INTERNAL`.

</rule>

<rule id="api-ffi-rows" strength="must">

Results cross the FFI as column batches of `u64` IDs plus an on-demand term decoder, matching the internal batch size of 1024 rows, so bindings can avoid per-row calls.

</rule>

The header `crates/graphlava-ffi/include/graphlava.h` is generated from `crates/graphlava-ffi/src/lib.rs` with `crates/graphlava-ffi/cbindgen.toml` and checked in. The test `tests/header.rs` regenerates it and fails when the checked-in copy differs; `GRAPHLAVA_BLESS=1` rewrites it. The test skips when the `cbindgen` binary is absent unless `GRAPHLAVA_REQUIRE_CBINDGEN=1` is set.

Conventions:

- `graphlava_version()` returns a static string; every other function returns a `graphlava_status`.
- A failing call stores a message per thread; `graphlava_last_error(char **out)` copies it (NULL when none). Strings the library returns are freed with `graphlava_string_free`.
- NULL pointers, non-UTF-8 strings and out-of-range enum arguments give `GRAPHLAVA_ERR_INVALID_ARGUMENT`. The `*_free` functions accept NULL.
- Enum arguments (`graphlava_lang`, `graphlava_durability`, `graphlava_term_kind`) are `uint32_t` with `#define` constants, so a bad value from C is an error, not undefined behaviour.
- A `graphlava_rows` holds a read snapshot of its database. `graphlava_close` refuses with `GRAPHLAVA_ERR_BUSY` while any result handle is alive.
- `graphlava_rows_next_batch` returns at most 1024 rows in column-major order (`ids[col * nrows + row]`, `0` = unbound) from a buffer owned by the handle and valid until the next call or free; `nrows = 0` ends the result.
- The header defines `GRAPHLAVA_ABI_VERSION` (1). `graphlava_open_ex(abi_version, path, flags, &db)` refuses any other value with `GRAPHLAVA_ERR_ABI` before touching the file system; `graphlava_open(path, &db)` is `graphlava_open_ex(GRAPHLAVA_ABI_VERSION, path, 0, &db)`. Bindings also compare `graphlava_abi_version(&v)` with the header they were built from. The ABI version changes with any signature, layout or constant meaning.
- Handles are opaque tokens from a registry, never dereferenced and never reused. A handle used after it was closed, freed, committed or aborted gives `GRAPHLAVA_ERR_CLOSED`; a pointer that is not a handle, or a handle of another kind, gives `GRAPHLAVA_ERR_INVALID_ARGUMENT`. `graphlava_commit` and `graphlava_abort` always end their handle, so a second commit is `GRAPHLAVA_ERR_CLOSED`.
- Listing calls (`graphlava_find`, `graphlava_scope`, `graphlava_result_deleted`, ...) fill a caller buffer of `cap` entries and store the total in `*out_n`; when `*out_n > cap` the caller calls again with a larger buffer. A read snapshot keeps repeated calls consistent.
- Inputs are `graphlava_arg` values: a term (`GRAPHLAVA_TERM_IRI`, `_BLANK`, `_LITERAL`), `GRAPHLAVA_ARG_ID`, `GRAPHLAVA_ARG_UINT` or `GRAPHLAVA_ARG_NONE`. A write request is built with `graphlava_request_add(req, GRAPHLAVA_OP_*, args, nargs)` (one op code per `CoreOp`, arity checked at once) or from SPARQL Update or Datalog text, and encodes to bytes with `graphlava_request_encode` (the extension journal's payload).

| Function group | Functions |
| --- | --- |
| lifecycle | `graphlava_version`, `graphlava_abi_version(&v)`, `graphlava_open(path, &db)`, `graphlava_open_ex(abi_version, path, flags, &db)` with flag `GRAPHLAVA_OPEN_APP_GROUP`, `graphlava_close(db)`, `graphlava_suspend(db)`, `graphlava_resume(db)`, `graphlava_flush(db)`, `graphlava_is_suspended(db, &s)`, `graphlava_open_snapshots(db, &n)` |
| errors | `graphlava_last_error(&msg)`, `graphlava_string_free(s)` |
| writes | `graphlava_sparql_update(db, text, durability)`, `graphlava_datalog_update(db, text, durability)`, `graphlava_insert(db, s_iri, p_iri, o_kind, o_value, o_datatype, o_lang, durability, &stmt_id)` |
| queries | `graphlava_prepare(db, lang, text, &prepared)` with `lang` = `GRAPHLAVA_LANG_SPARQL` or `GRAPHLAVA_LANG_DATALOG`, `graphlava_query(db, prepared, &rows)`, `graphlava_prepared_free` |
| rows | `graphlava_rows_columns(rows, &n)`, `graphlava_rows_column_name(rows, i, &name)`, `graphlava_rows_next_batch(rows, &ids, &nrows, &ncols)`, `graphlava_term_decode(rows, id, &term)`, `graphlava_term_free(&term)`, `graphlava_rows_free(rows)` |
| requests | `graphlava_request_new(&req)`, `graphlava_request_new_text(lang, text, &req)`, `graphlava_request_add(req, op, args, nargs)`, `graphlava_request_encode(req, buf, cap, &len)`, `graphlava_request_decode(buf, len, &req)`, `graphlava_request_free` |
| transactions | `graphlava_submit(db, req, durability, &result)`, `graphlava_result_len`, `graphlava_result_get(res, i, &kind, &value)`, `graphlava_result_deleted`, `graphlava_result_free`; `graphlava_read_begin(db, &read)`, `graphlava_read_end`; `graphlava_write_begin(db, durability, &write)` (in-process only), `graphlava_write_run(write, req, &result)`, `graphlava_commit`, `graphlava_abort`. Query parameters are not offered yet: the core has none |
| core | on a read snapshot: `graphlava_lookup`, `graphlava_decode`, `graphlava_stmt`, `graphlava_find`, `graphlava_out`, `graphlava_in`, `graphlava_props`, `graphlava_graph`, `graphlava_graphs_of`, `graphlava_scope`, `graphlava_hedge` |
| admin | `graphlava_sweep(db, window_ms, &deleted)`, `graphlava_export_snapshot(db, path)` |
| extension journal | `graphlava_journal_append(path, req, &seq)`, `graphlava_journal_apply(db, path, key, &report)` ([[16-api#Mobile bindings]]) |

| Status | Value | Meaning |
| --- | --- | --- |
| `GRAPHLAVA_OK` | 0 | success |
| `GRAPHLAVA_ERR_INVALID_ARGUMENT` | 1 | NULL pointer, bad UTF-8, unknown enum value, index out of range |
| `GRAPHLAVA_ERR_INTERNAL` | 2 | caught panic, or an engine error this ABI does not know |
| `GRAPHLAVA_ERR_BUSY` | 3 | `graphlava_close` while result, read or write handles are alive; `graphlava_suspend` that cannot release the lock under an open snapshot (`Error::Busy`) |
| `GRAPHLAVA_ERR_ABI` | 4 | `graphlava_open_ex` with another `GRAPHLAVA_ABI_VERSION`; no file was touched |
| `GRAPHLAVA_ERR_CLOSED` | 5 | a closed, freed, committed or aborted handle, or a result or read handle whose snapshot `graphlava_suspend` ended |
| `GRAPHLAVA_ERR_IO` | 10 | `Error::Io` |
| `GRAPHLAVA_ERR_CORRUPTED` | 11 | `Error::Corrupted` |
| `GRAPHLAVA_ERR_FULL` | 12 | `Error::Full` |
| `GRAPHLAVA_ERR_FORMAT_MISMATCH` | 13 | `Error::FormatMismatch` |
| `GRAPHLAVA_ERR_LOCKED` | 14 | `Error::Locked` |
| `GRAPHLAVA_ERR_CONFLICT` | 15 | `Error::Conflict` |
| `GRAPHLAVA_ERR_NOT_FOUND` | 16 | `Error::NotFound` |
| `GRAPHLAVA_ERR_REFERENCED` | 17 | `Error::ReferencedByStatement`, `Error::FillsRole` |
| `GRAPHLAVA_ERR_GRAPH_CONSTRAINT` | 18 | `Error::GraphCycle`, `Error::GraphDepthExceeded` |
| `GRAPHLAVA_ERR_HYPEREDGE_SHAPE` | 19 | `Error::HyperedgeShapeMismatch` |
| `GRAPHLAVA_ERR_CASCADE_LIMIT` | 20 | `Error::CascadeLimitExceeded` |
| `GRAPHLAVA_ERR_WRITE_TIMEOUT` | 21 | `Error::WriteTimeout` |
| `GRAPHLAVA_ERR_READ_ONLY` | 22 | `Error::ReadOnly` |
| `GRAPHLAVA_ERR_INVALID_REQUEST` | 23 | `Error::Invalid`: a query, update or program that does not parse, a bad term |
| `GRAPHLAVA_ERR_ABORTED` | 24 | `Error::Aborted`: the interactive write transaction was aborted by `graphlava_suspend` |

## Mobile bindings

Swift and Kotlin wrappers over the C FFI, with the app-lifecycle hooks iOS and Android need. Arrive in v1.

- depends_on:: [[16-api#C FFI]] {strength: hard}
- follows_from:: [[adr:0042]]
- follows_from:: [[adr:0043]]
- cites:: [[07-platforms#iOS]] {intent: citesAsAuthority}
- cites:: [[07-platforms#Android]] {intent: citesAsAuthority}
- cites:: [[23-binding-decisions#Suspend is a core operation]] {intent: usesConclusionsFrom}
- cites:: [[23-binding-decisions#iOS extensions write through XPC or a journal]] {intent: usesConclusionsFrom}
- introduced_by:: [[18-roadmap#v1]]
- measured_by:: [[17-benchmarks#Mobile]]

Requirements: `openspec:mobile-bindings` (Complete C FFI, FFI misuse safety, Swift API, Kotlin API, iOS suspend and resume, Android background and kill safety, Mobile native packaging, iOS extension writes, Mobile latency).

Swift, the package `GraphLava` in `bindings/swift/` over `GraphLavaFFI.xcframework` (iOS arm64, iOS simulator arm64 and x86_64, macOS arm64 and x86_64; module map and privacy manifest):

```swift
let db = try await Database.open(url)                    // an actor on one dedicated thread
let r = try await db.submit([.insert(alice, knows, bob)], durability: .ordered)
try await db.write(.sync) { txn in try txn.insert(a, p, o) }   // commits on return, aborts on throw
for try await batch in try await db.query("SELECT ...") { batch.term(row: 0, column: "c") }  // ≤ 1024 rows
```

IDs are `UInt64`, terms a `Term` enum, errors `GraphLavaError` with one case per status plus `extensionProcess`. A `LifecycleObserver` suspends inside `beginBackgroundTask` on `willResignActive`/`didEnterBackground` and resumes on `willEnterForeground`/`didBecomeActive`. A file under `.../Shared/AppGroup/...` is opened with `GRAPHLAVA_OPEN_APP_GROUP`, so suspend releases its lock; a private-container file keeps it (`ios-private-container`).

Kotlin, `org.graphlava:graphlava-android` in `bindings/kotlin/`: the Kotlin API (module `graphlava`, JNI shim `graphlava_jni.c` compiled into `libgraphlava.so` with the C FFI) and the AAR with `arm64-v8a` and `x86_64` libraries linked with `-z max-page-size=16384`. `Database` has `suspend` functions on a single-thread writer dispatcher and a read pool sized to the core count, `Flow<RowBatch>` results, `ULong` IDs, a sealed `GraphLavaException` hierarchy and `AutoCloseable`; blocking variants throw `IllegalStateException` on the main thread in debuggable apps. `GraphLavaLifecycle.install` flushes on `ProcessLifecycleOwner` `ON_STOP` and keeps the lock.

Extension journal ([[adr:0043]]): an extension never opens the file. `ExtensionClient` sends a request to the running app over XPC when it holds the app's endpoint, else `graphlava_journal_append` adds it to `<appgroup>/graphlava/<extension-id>.journal`: a 16-byte header (`GLVJ`, version 1, base sequence number) and frames `len:u32le | seq:u64le | crc32c:u32le | request bytes`, the CRC-32C covering `seq` and the request. On open, `graphlava_journal_apply` runs each complete frame as one write request and stores `journal_seq:<extension-id> = seq` under a metadata key in the same transaction (`Database::submit_once`), so each frame applies exactly once; the first incomplete or corrupt frame and everything after it is a torn tail, dropped with one warning; after a durable flush the journal is truncated to its header with the base raised to the last applied number.

<rule id="api-ios-suspend" strength="must">

The Swift binding calls `graphlava_suspend` when the app moves to the background. Suspend, in this order, makes new calls wait for resume, aborts the open interactive write transaction (one-shot requests already in the batch commit), flushes the batch, ends all read snapshots and, for a file in an app-group container, releases the file lock, so the app is never killed for holding a lock on a shared container file (`0xdead10cc`). `graphlava_resume` re-acquires the lock; snapshots are re-opened lazily by the next calls.

</rule>

<rule id="api-ios-extension-ipc" strength="should">

An iOS app extension does not open the database file. It reaches the owner (the containing app) through an XPC service or, when the app is not running, queues writes in a small append-only file in the app group that the owner applies on its next start.

</rule>

<rule id="api-kotlin-threads" strength="must">

The Kotlin binding runs every database call on a dedicated executor, never on the main thread, and exposes results as `Flow` batches.

</rule>

## Python binding

The PyPI package `graphlava`: a PyO3 extension (`crates/graphlava-py`, built with maturin) that links the Rust API directly, without the C FFI ([[adr:0042]]). Arrives in v1.

- follows_from:: [[adr:0042]]
- depends_on:: [[16-api#Rust API]] {strength: hard}
- depends_on:: [[16-api#Core API]] {strength: hard}
- introduced_by:: [[18-roadmap#v1]]
- measured_by:: [[17-benchmarks#Binding overhead]]

Requirements: `openspec:python-binding` ([[23-binding-decisions#Python binds the Rust API with PyO3]] and the decisions after it).

```python
import graphlava as gl
from graphlava import IRI, Durability, Insert, Update

db = gl.Database.open("memory.glv")             # LockedError if another owner holds it
sid = db.submit([Insert(IRI(":alice"), IRI(":knows"), IRI(":bob"))],
                durability=Durability.ORDERED).results[0]
with db.write(Durability.SYNC) as w:             # commits on normal exit, aborts on exception
    w.annotate(sid, IRI(":confidence"), 0.9)
with db.read() as r:                             # snapshot; core reads return Id ints
    r.props(sid); r.out(r.lookup(IRI(":alice")))
for row in db.sparql("SELECT ?c WHERE { << :alice :knows :bob >> :confidence ?c }"):
    ...                                          # tuples of Python values, decoded lazily
db.close()
```

| Python | Rust |
| --- | --- |
| `Database.open(path, *, write_timeout_ms, group_window_ms, cache_bytes)`, `close()`, `with Database.open(...)` | `Database::open`, `close` |
| `read()`, `write(durability)` as context managers | `read`, `write`, `commit` on normal exit, `abort` on exception |
| `submit(ops, durability)` → `CommitResult(results, batch)` | `submit(WriteRequest, Durability)` → `CommitTicket` |
| op classes `Insert`, `Set`, `Update`, `Annotate`, `Delete`, `Remove`, `StatementId`, `InsertUnasserted`, `DeclarePredicate`, `CreateGraph`, `AddMember`, `RemoveMember`, `MoveMember`, `MembershipId`, `Nest`, `Unnest`, `DropGraph`, `DeclareHedgeType`, `CreateHedge`, `UpdateRole`, `RoleId` | every `CoreOp` |
| `props`, `out`, `inc`, `find`, `stmt`, `hedge`, `lookup`, `decode`, `graph`, `home_graph`, `graphs_of` on `ReadTxn` and `WriteTxn` | the `Reader` methods |
| `sparql(text)`, `datalog(program)` → `Rows` (or `bool` for `ASK`); `sparql_update`, `datalog_update` | `query_ids` with lazy decoding, `sparql_update`, `datalog_update` |
| `Rows`: iteration, `iter_batches()` (≤ 1024 rows), `ids()`, `to_arrow()`, `vars` | `IdRows` plus its snapshot (rule `api-rows-late`) |
| `admin()`: `sweep`, `build_ordering`, `build_value_index`; `file_size()` | `Admin`, `file_size` |

Type mapping:

| Python value | Term |
| --- | --- |
| `IRI(str)`, `BNode(label)`, `Literal(lexical, datatype, lang)`, `Triple(s, p, o)` | IRI, blank node, any literal, triple term |
| `str`, `bool`, `int` (any size), `float` (not NaN), `decimal.Decimal`, aware `datetime.datetime`, `datetime.date` | `xsd:string`, `xsd:boolean`, `xsd:integer`, `xsd:double`, `xsd:decimal`, `xsd:dateTime`, `xsd:date` |
| `Id` (an `int` subclass, full unsigned 64 bits) | an existing ID in any position; a plain `int` is an ID in subject and predicate position and an `xsd:integer` in object position |

Integers inside ±2^57 are inline; larger ones go to the dictionary and come back as equal `int`s. A naive `datetime` and NaN raise `InvalidError`. Decoding returns the same Python types; a dateTime without a timezone and literals of other datatypes come back as `Literal`.

Runtime rules:

- Every call that does I/O, parses, plans, executes or waits on a commit releases the GIL; the module declares `gil_used = false` for free-threaded 3.14t.
- The interactive writer keeps the Rust time limit (rule `api-write-in-process`); `WriteTimeoutError` names `submit` as the alternative.
- A handle records the PID that opened it. A call from another PID (after `fork`) raises `ForkedHandleError` before touching the file, and the child never flushes or closes the parent's handle.
- Every `Error` variant maps to a subclass of `graphlava.Error` named `<Variant>Error`, with the variant's fields as attributes. A Rust panic becomes `InternalError`.
- Wheels: abi3 for CPython ≥ 3.10 and free-threaded 3.14t, for Linux x86_64 and aarch64 (manylinux_2_28, musllinux_1_2), macOS arm64 and x86_64, Windows x86_64, each with `py.typed` and `.pyi` stubs. `graphlava.__version__ == graphlava.core_version()`. The crate's MSRV is the workspace MSRV, 1.87 (PyO3 0.30 needs 1.85).

The shared conformance corpus `bindings/conformance/` (JSON lines, schema in `schema.json`) is run first by the Rust runner `crates/graphlava/tests/conformance.rs` and then by each binding ([[adr:0042]]).

## Node.js binding

The npm package `@graphlava/node`: a napi-rs extension (`crates/graphlava-node`, N-API 9) over the Rust API ([[adr:0042]]), with the JavaScript layer and types in `bindings/node/`. Arrives in v1.

- follows_from:: [[adr:0042]]
- depends_on:: [[16-api#Rust API]] {strength: hard}
- introduced_by:: [[18-roadmap#v1]]
- measured_by:: [[17-benchmarks#Binding overhead]]
- measured_by:: [[17-benchmarks#Durability commit latency]]
- cites:: [[23-binding-decisions#Node runs database work on its own threads]] {intent: usesConclusionsFrom}
- cites:: [[23-binding-decisions#Node write limit runs across awaits]] {intent: usesConclusionsFrom}
- cites:: [[23-binding-decisions#Node IDs are bigint and integers are numbers only when safe]] {intent: usesConclusionsFrom}
- cites:: [[23-binding-decisions#Node keeps one handle per file per process]] {intent: usesConclusionsFrom}

```ts
import { Database, iri, GraphlavaError } from '@graphlava/node';

const db = await Database.open('memory.glv', { prefixes: { '': 'http://ex.org/' } });
const { results: [stmt] } = await db.submit([{ op: 'insert', s: ':alice', p: ':knows', o: iri(':bob') }], 'ordered');
await db.submit([{ op: 'annotate', id: stmt as bigint, p: ':confidence', o: 0.9 }]);
for await (const row of await db.sparql('SELECT ?c WHERE { << :alice :knows :bob >> :confidence ?c }')) console.log(row.c);
await db.close();
```

| JS API | Rust call | Runs on |
| --- | --- | --- |
| `Database.open(path, {prefixes, writeTimeoutMs, ...})` | `Database::open` | read pool |
| `db.submit(ops, durability)` → `{results, batch}` | `Database::submit` | commit pool |
| `db.sparql(text)`, `db.datalog(program)` → `Rows` | `ReadTxn::query_ids` | read pool |
| `db.sparqlUpdate`, `db.datalogUpdate` | `Database::sparql_update`, `datalog_update` | commit pool |
| `db.read()` → `ReadTxn`; core reads `props`, `out`, `inc`, `find`, `stmt`, `hedge`, `lookup`, `graph`, `decode`, ... on it or on `db` | `Reader` | read pool |
| `db.write(durability)` → `WriteTxn` (`run`, `insert`, ..., reads, `sparql`, `commit`, `abort`) | `Database::write`, `WriteTxn` | the database's writer thread |
| `db.admin()` (`sweep`, `buildOrdering`, ...) | `Admin` | writer thread |
| `db.close()` | `Database::close` | writer thread, after queued writes |

Rules of the binding:

- Every call that can do I/O, run a query or wait on a commit returns a Promise and runs off the event loop. Only in-memory metadata is synchronous (`version()`, `db.isOpen`, `w.active`). Blocking variants (`Database.openSync`, `db.submitSync`, `db.readSync()`, `db.sparqlSync`, `*Sync` core reads) exist only in `@graphlava/node/sync`, documented as blocking.
- A core operation is a plain object tagged by `op`: `insert`, `set`, `update`, `annotate`, `delete`, `remove`, `statementId`, `insertUnasserted`, `declarePredicate`, `createGraph`, `addMember`, `removeMember`, `moveMember`, `membershipId`, `nest`, `unnest`, `dropGraph`, `declareHedgeType`, `createHedge`, `updateRole`, `roleId`. A missing field rejects with `INVALID` naming it, before anything is written.
- IDs are `bigint` in arguments and results; a `number` where an ID is expected rejects with `INVALID`. Row ID columns are `BigUint64Array`s.
- Terms: in node positions (`s`, `p`, `graph`, `type`, `role`, ...) a string is an IRI (`<iri>`, a declared prefix, or an absolute IRI) and a `bigint` is an ID. In value positions `string` → `xsd:string`, `boolean` → `xsd:boolean`, `bigint` → `xsd:integer`, `number` → `xsd:integer` when it is a safe integer and `xsd:double` otherwise (`NaN` and unsafe integers reject with `INVALID`; `double(x)` forces a double), `Date` → `xsd:dateTime` in milliseconds; other terms are tagged objects `{iri}`, `{bnode}`, `{id}`, `{value, datatype}`, `{value, lang}`. Integer results are `number` inside ±(2^53−1) and `bigint` outside it. `decodeTerm(id)` and `Batch.terms(v)` return the lossless form of the conformance corpus.
- `WriteTxn` keeps the Rust time limit, measured from the moment `write()` resolves and running across awaits; at the deadline the writer thread aborts the transaction and serves queued requests. `await using w = await db.write(...)` aborts on scope exit without `commit()`. `WRITE_TIMEOUT` names `submit` as the alternative.
- A process-global registry of canonical paths refuses a second `open` of a file in the same process, also from a `worker_thread`, with `LOCKED`; another process gets `LOCKED` from the OS lock. Handles are uncloneable (`postMessage(db)` throws `DataCloneError`). `close()` releases the lock and rejects with `BUSY` while row sets, snapshots or transactions are open; a handle collected without `close()` flushes its pending group commit and releases the lock in its finalizer.
- Rows are an async iterable of row objects; `rows.batches()` yields batches of up to 1024 rows whose ID columns are `BigUint64Array`s and whose terms are decoded per column on request (`batch.values(v)`). Ending or leaving either loop releases the read snapshot at once; `_debug.openSnapshots()` counts live snapshots.
- Errors are `GraphlavaError` with a stable `code`, one per Rust `Error` variant in SCREAMING_SNAKE_CASE (`LOCKED`, `CONFLICT`, `WRITE_TIMEOUT`, `NOT_FOUND`, `GRAPH_CYCLE`, `GRAPH_DEPTH_EXCEEDED`, `FILLS_ROLE`, `REFERENCED_BY_STATEMENT`, `CORRUPTED`, `INVALID`, ...), plus `BUSY`, `CLOSED` and `INTERNAL`. ID fields (`id`, `item`, `by`, `hedge`, `child`, `parent`) are `bigint`; counts (`limit`) are `number`. A Rust panic rejects with `INTERNAL` and the process survives.
- Packaging: `@graphlava/node` (ESM, CommonJS, `.d.ts` checked with `tsc --strict`) with optional per-target packages `@graphlava/node-<platform>-<arch>[-<abi>]` for linux-x64-gnu, linux-arm64-gnu, linux-x64-musl, darwin-arm64, darwin-x64 and win32-x64-msvc, on Node 20, 22 and 24; every package.json sets `"publishConfig": { "access": "public" }`. Any other platform fails to load with an error naming it and listing the supported targets. The crate's MSRV is Rust 1.88 (napi 3), above the workspace's 1.87, so the workspace MSRV job excludes it and `.github/workflows/node.yml` checks it.

The Node tests (`bindings/node/test/*.test.mjs`, `npm test`) cover every scenario of `node-binding`, replay the oracle traces (seeds 1..16 × 2,000) and run the conformance corpus, including the Node cases in `bindings/conformance/node.jsonl`.

## Server crate

`graphlava-server`: a separate crate that hosts the owner process and exposes SPARQL Protocol, a Datalog endpoint and a local IPC socket. Arrives in v1; HTTP dependencies stay out of the core crate.

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
| local socket | framed binary | the C FFI read and core operations and `graphlava_submit` over IPC, for other processes on the same machine; no interactive write transaction |

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
- Writer hold: an in-process `WriteTxn` that exceeds its time limit fails with `WriteTimeout`, and other writers proceed.
- Core API: random core operations and the equivalent SPARQL updates produce identical stores; compare-and-set retries converge under 16 concurrent clients.
- Suspend test (iOS simulator and device): background the app during a write and a group commit; no `0xdead10cc` and no lost acknowledged commit.
- FFI fuzz: random call sequences over the C API, no crash and no leak under sanitizers.
- Server: W3C SPARQL 1.1 Protocol tests.

## Open items

Questions this spec leaves open; each is an `OpenQuestion` in [[open-questions]].

- [[open-questions#Server authentication]]: which authentication methods the server crate supports first.

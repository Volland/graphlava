# Spec Delta

## Purpose

The Swift package `GraphLava` and the Kotlin library `org.graphlava:graphlava-android`. Both wrap the C FFI and fit on-device agents into iOS and Android app lifecycles, including suspension, background kills and app extensions. Design: `tg.md/16-api.md#Mobile bindings`, `tg.md/07-platforms.md`, this change's `design.md`.

## ADDED Requirements

### Requirement: Complete C FFI
The C FFI SHALL provide every function group in `tg.md/16-api.md#C FFI` behind a generated, versioned `graphlava.h`. Every function except `graphlava_version` SHALL return a `graphlava_status` and write its outputs through out-parameters. Results SHALL cross as 1024-row batches of `u64` ID columns, with `graphlava_term_decode` for terms.

Formal: none — ABI surface, outside the model

#### Scenario: C lifecycle under sanitizers
- **Level**: e2e
- **WHEN** a C program built with AddressSanitizer and LeakSanitizer opens a file, runs the agent memory session through `graphlava_submit`, `graphlava_prepare`, `graphlava_query` and `graphlava_rows_next_batch`, and then closes it
- **THEN** the program exits 0 with no sanitizer report, and the reopened file holds the session's statements

#### Scenario: Header version mismatch
- **Level**: unit
- **WHEN** a caller passes an ABI version other than the library's `GRAPHLAVA_ABI_VERSION` (for example 2 to a version 1 library) to `graphlava_open_ex`
- **THEN** `graphlava_open_ex` returns `GRAPHLAVA_ERR_ABI` and creates no file

### Requirement: FFI misuse safety
No Rust panic and no caller misuse SHALL cause undefined behaviour across the FFI. A panic SHALL become `GRAPHLAVA_ERR_INTERNAL`, and misuse SHALL return an error status.

Formal: none — memory safety at the ABI, covered by sanitizers and fuzzing

#### Scenario: Random call sequences
- **Level**: property
- **WHEN** `cargo fuzz` drives random sequences of FFI calls, including use after `graphlava_close`, a double `graphlava_commit` and null out-parameters, for 1 hour in CI nightly
- **THEN** no crash, leak or undefined-behaviour report occurs, and every misuse returns an error status

#### Scenario: Use after close
- **Level**: e2e
- **WHEN** a C program calls `graphlava_read_begin` on a handle after `graphlava_close`
- **THEN** it returns `GRAPHLAVA_ERR_CLOSED`, and a fresh `graphlava_open` of the file succeeds

### Requirement: Swift API
The Swift package SHALL expose `GraphLava.Database` as an `actor` with `async throws` methods mirroring the Rust API. Transactions SHALL use closure scope: `try await db.write(.sync) { txn in ... }` commits when the closure returns and aborts when it throws. Terms SHALL be a `Term` enum, IDs SHALL be `UInt64`, errors SHALL be a `GraphLavaError` enum with associated values, and rows SHALL be an `AsyncSequence` of 1024-row batches.

Formal: none — marshalling layer; the effect is the Rust call's

#### Scenario: Agent memory session from Swift
- **Level**: e2e
- **WHEN** an XCTest on the iOS simulator opens `memory.glv` in the app's private container, runs the agent memory session with `.ordered` commits, closes the database and opens it again
- **THEN** the reifier query returns the stored `:confidence` and the IDs equal the ones returned before the reopen

#### Scenario: Throwing closure aborts
- **Level**: e2e
- **WHEN** a `write(.sync)` closure inserts 3 statements and then throws
- **THEN** the error reaches the caller, and none of the 3 statements exist after reopening

#### Scenario: Oracle trace replay through Swift
- **Level**: refinement
- **WHEN** the oracle traces for seeds 1..8, with 2,000 operations each, are replayed on the macOS host through the Swift package
- **THEN** every result and the final state match the Rust replay

### Requirement: Kotlin API
The Kotlin library SHALL expose `org.graphlava.Database` with `suspend` functions mirroring the Rust API. Every database call SHALL run on one dedicated single-thread dispatcher for writes and a bounded pool for reads, never on the main thread. Results SHALL be a `Flow<RowBatch>` of 1024-row batches. IDs SHALL be `ULong`, and errors SHALL be a sealed `GraphLavaException` hierarchy. `Database` SHALL implement `AutoCloseable`.

Formal: none — marshalling layer and host threading, outside the model

#### Scenario: Agent memory session from Kotlin
- **Level**: e2e
- **WHEN** an instrumented test on an arm64-v8a emulator opens `memory.glv` in `filesDir`, runs the agent memory session with `ORDERED` commits, closes the database and opens it again
- **THEN** the reifier query returns the stored `:confidence` and the IDs equal the ones returned before the reopen

#### Scenario: Main-thread call is refused in debug builds
- **Level**: unit
- **WHEN** a debug build calls the blocking `Database.openBlocking(...)` from `Looper.getMainLooper()`
- **THEN** it throws `IllegalStateException` naming the main thread, and StrictMode reports no disk read

#### Scenario: Flow cancellation releases the snapshot
- **Level**: e2e
- **WHEN** a collector of a 10,000-row query `Flow` is cancelled after the first batch
- **THEN** the read snapshot is released, and `db.debugOpenSnapshots()` returns 0

### Requirement: iOS suspend and resume
On `willResignActive` or `didEnterBackground`, the Swift package SHALL call `graphlava_suspend` inside a `beginBackgroundTask` window, and it SHALL call `graphlava_resume` on return to the foreground. Suspend SHALL end the open write and read transactions and flush the batch. It SHALL release the lock for an app-group file. Commits acknowledged before suspend SHALL survive a kill.

Formal: none — OS lifecycle behaviour; the durable-prefix property it relies on is `MetaCore.Durability`

#### Scenario: Background during a group commit
- **Level**: crash
- **WHEN** an app with its database in an app-group container issues 50 `.ordered` commits, is sent to the background while the batch is open, and is then terminated by the system
- **THEN** no `0xdead10cc` crash report is produced, and after relaunch every commit acknowledged before backgrounding is present

#### Scenario: Calls while suspended wait for resume
- **Level**: e2e
- **WHEN** a background task calls `db.read` after `graphlava_suspend` and the app returns to the foreground 2 s later
- **THEN** the call completes after `graphlava_resume` with correct rows, and no snapshot from before the suspend is reused

#### Scenario: Private container keeps the lock
- **Level**: e2e
- **WHEN** the database is in the app's private container and the app is backgrounded
- **THEN** the pending batch is flushed, the lock is kept (the `ios-private-container` exception), and resume does not reopen the file

#### Scenario: Suspend with nothing open
- **Level**: unit
- **WHEN** `graphlava_suspend` is called on a database with no transaction and an empty batch
- **THEN** it returns `GRAPHLAVA_OK` within 5 ms for a private-container file (an app-group file adds the engine close, one flush, bounded by the `Mobile` benchmark), and a second `graphlava_suspend` returns `GRAPHLAVA_OK` without effect

### Requirement: Android background and kill safety
The Kotlin library SHALL observe `ProcessLifecycleOwner`. On `ON_STOP` it SHALL flush the pending group commit and finish or abort the open write transaction. Every commit acknowledged with `SYNC` or `ORDERED` SHALL survive a process kill without warning.

Formal: none — OS lifecycle behaviour; the durable-prefix property it relies on is `MetaCore.Durability`

#### Scenario: Kill after background
- **Level**: crash
- **WHEN** a test app makes 100 `ORDERED` commits, goes to the background, and is killed with `am kill` and then `kill -9`
- **THEN** after relaunch all 100 acknowledged commits are present and the file opens without a recovery error

#### Scenario: Relaxed commit may be lost
- **Level**: crash
- **WHEN** a `RELAXED` commit is acknowledged and the process is killed before `ON_STOP`
- **THEN** after relaunch the transaction is either fully present or fully absent

### Requirement: Mobile native packaging
The Swift package SHALL ship an XCFramework for iOS arm64, iOS simulator arm64 and x86_64, and macOS arm64 and x86_64, with a privacy manifest that declares file-timestamp API use. The Kotlin library SHALL ship an AAR with `arm64-v8a` and `x86_64` libraries. These SHALL be linked with `-z max-page-size=16384`, and SHALL load on a 16 KB-page Android 15 emulator. Both packages SHALL report the same core version as the C header.

Formal: none — packaging

#### Scenario: 16 KB page alignment
- **Level**: e2e
- **WHEN** CI checks the AAR's libraries with `llvm-readelf -l` and runs the Kotlin agent memory session on a 16 KB-page Android 15 emulator
- **THEN** every `LOAD` segment is aligned to 0x4000 and the session passes

#### Scenario: 4 KB-aligned library is rejected by CI
- **Level**: unit
- **WHEN** a library linked without the 16 KB flag is placed in the AAR
- **THEN** the alignment check job fails and names the library

### Requirement: iOS extension writes
An iOS app extension SHALL NOT open the database file. `GraphLava.ExtensionClient` SHALL send write requests to the running app over XPC, or append them to a per-extension journal in the app group. The owner SHALL apply journal entries on its next open in append order, each exactly once as one write request.

Formal: none — inter-process protocol; per-request atomicity is the transactions requirement

#### Scenario: Share extension write while app is closed
- **Level**: e2e
- **WHEN** a share extension submits 3 write requests while the app is not running, and the app is launched afterwards
- **THEN** on open the app applies the 3 requests in order, the journal is empty, and relaunching does not apply them again

#### Scenario: Crash while applying the journal
- **Level**: crash
- **WHEN** the owner is killed after applying the second of 3 journal entries
- **THEN** on the next open exactly the third entry is applied, and the first two are not applied again

#### Scenario: Extension tries to open the file
- **Level**: unit
- **WHEN** code running in an extension bundle calls `Database.open`
- **THEN** it throws `GraphLavaError.extensionProcess` and the file is not locked

#### Scenario: Corrupt journal entry
- **Level**: e2e
- **WHEN** the last journal entry has a bad checksum because the extension was killed mid-append
- **THEN** the owner applies the earlier entries, drops the torn tail, and logs one warning

### Requirement: Mobile latency
On one reference iPhone and one reference Android arm64 device, the agent-memory mix subset SHALL be measured through each binding at 1M and 10M statements with `ordered` durability. The metrics are p50 and p99 latency in microseconds per operation, time to suspend and time to resume in milliseconds, and file size. Pass line: suspend p99 under 500 ms at 10M statements; every other metric is to be measured. Benchmark: `tg.md/17-benchmarks.md#Mobile`.

Formal: none — performance

#### Scenario: Suspend fits the background window
- **Level**: benchmark
- **WHEN** the `Mobile` benchmark backgrounds the app with a full open batch at 10M statements, 100 times
- **THEN** suspend p99 is recorded, and the run fails if it is 500 ms or more

#### Scenario: Mobile baseline recorded
- **Level**: benchmark
- **WHEN** the `Mobile` benchmark runs on each reference device
- **THEN** p50 and p99 latency, file size, and energy per 10^4 operations where the OS exposes it are stored as the baseline JSON with the device model and OS version

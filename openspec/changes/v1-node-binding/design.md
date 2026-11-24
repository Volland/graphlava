# Design

## Context

The core API is synchronous ([[16-api#Rust API]], `api-no-async-required`). Node runs all JavaScript on one thread, so a synchronous `fsync` on that thread freezes every timer, socket and request in the process. JS `number` is a double, but graphlava IDs use all 64 bits ([[02-id-encoding#Bit layout]]). Node also runs `worker_thread`s inside the same process, so the single-owner lock must hold within one process, not only between processes.

## Decisions

### D1. Bind the Rust API directly with napi-rs ([[adr:0042]])

- **Choice:** `crates/graphlava-node` depends on `graphlava` and targets N-API 9.
- **Rejected:**
  - `ffi-napi` or `koffi` over the C FFI: slower calls, manual memory ownership, and no type safety.
  - A WebAssembly build: no file locks and no `fsync` semantics, so the durability levels could not be honoured.
- **Reasons:** `motivated_by` [[16-api#Rust API]].

### D2. Promises on a dedicated pool, never the event loop

- **Choice:**
  - Every I/O, query or commit call returns a Promise backed by a napi deferred that a binding-owned thread settles (not a libuv `AsyncTask`).
  - An interactive `WriteTxn` (and `admin()`, `close()`) runs on one writer thread per database, which keeps each transaction's calls in order.
  - One-shot requests (`submit`, `sparqlUpdate`, `datalogUpdate`) run on a commit pool, so concurrent `ordered` requests wait for one group commit together; the core's writer token orders them.
  - Reads run on a pool sized from the available parallelism (the value `os.availableParallelism()` reports).
  - Blocking variants exist only under `@graphlava/node/sync`, for scripts and CLIs.
- **Rejected:**
  - Synchronous calls by default, as in better-sqlite3. That is acceptable for SQLite point reads, but a `sync` commit can take milliseconds.
  - libuv's default pool of 4 threads, which is shared with `fs` and `dns` and starves under load.
  - One-shot requests on the writer thread: each `ordered` request would wait for its own group commit before the next started, so concurrent callers would never share a batch (measured during implementation: 8 callers, `ordered` p50 56 ms instead of 7 ms).
- **Reasons:** `motivated_by` `api-no-async-required`.

### D3. The writer limit runs across awaits

- **Choice:** the 100 ms limit starts when `write()` resolves and includes time the caller spends awaiting other work. `await using` aborts on scope exit without `commit()`.
- **Rejected:** pausing the clock while JS runs. Pausing would let one request hold the single writer for as long as it chose.
- **Reasons:** `motivated_by` `api-write-in-process` and [[adr:0026]]. The documentation leads with `submit`.

### D4. `bigint` IDs, `number` values only when safe

- **Choice:**
  - IDs are always `bigint`.
  - An `xsd:integer` result is returned as `number` inside ±(2^53−1) and as `bigint` outside it.
  - Inputs that are unsafe-integer `number`s are rejected.
- **Rejected:**
  - String IDs, which allocate on every call and invite `===` bugs between `"1"` and `1n`.
  - Always returning `bigint` for integers, which breaks arithmetic in typical agent code.
- **Reasons:** `motivated_by` [[02-id-encoding#Bit layout]].

### D5. One handle per process; workers are refused

- **Choice:**
  - A process-global registry of open canonical paths makes a second `open` in the same process reject with `LOCKED`.
  - Handles are not structured-cloneable.
  - A finalizer flushes the batch and releases the lock when a handle is collected without `close()`.
- **Rejected:** handing workers a shared handle. It is feasible, but it hides which thread owns the file and complicates close order. This can be revisited if users ask for it.
- **Reasons:** `motivated_by` `api-single-owner`. redb's OS lock does not catch a second open within the same process on every platform, so the registry is required.

## Constraints

| Constraint | Binds |
| --- | --- |
| N-API 9 ABI stability | one binary per target across Node 20–24 |
| Node.js release schedule (20, 22, 24 supported) | the CI matrix |
| npm `optionalDependencies` + `os`/`cpu`/`libc` fields | per-target packages `@graphlava/node-<platform>-<arch>[-<abi>]` |
| npm scoped packages are private by default | `"publishConfig": { "access": "public" }` in every package.json |
| TC39 explicit resource management (`Symbol.asyncDispose`) | `await using` on `WriteTxn` and `Database` |

## Formal model

None. The binding adds no state and no invariant, so no Lean module changes. New `<assumption>` for `tg.md/20-proofs.md`: no two handles to one file exist in one process (D5).

## Refinement

Identity abstraction on public API state. Oracle traces (seeds 1..16 × 2,000) are replayed through Node, and every result and the final dump must equal the Rust replay. The shared corpus `bindings/conformance/` adds Node-specific cases:

- The 2^53 boundary.
- `-0`, `NaN` and `Infinity`.
- `Date` with milliseconds.
- `bigint` above 2^63.

## Test strategy

| Requirement | L1 | L2 | L3 | L4 | L6 |
| --- | --- | --- | --- | --- | --- |
| API parity | oracle replay | corpus | agent memory session | — | — |
| Event loop never blocked | — | — | timer-gap test | — | event-loop delay |
| Interactive writes | — | — | scope abort, time limit across awaits | SIGKILL after a `sync` commit | — |
| ID and literal mapping | — | corpus, fast-check | boundary round trips | — | — |
| Single owner | — | — | worker refused, close releases | — | — |
| Row streaming | — | corpus | batches, early break | — | — |
| Errors | — | — | cycle and depth-64 errors | — | — |
| Packaging | — | — | install smoke test (6 targets × 3 Node) | — | — |
| Overhead | — | — | — | — | Binding overhead |

## Cost model

No added B-tree operations. Each call adds one fixed overhead (a Promise, a thread hop and argument conversion), and each decoded term adds a per-term overhead. Batches cost one hop per 1024 rows. ID columns cross as `BigUint64Array` backed by an `ArrayBuffer`, copied once.

## Benchmarks

Reuses `Binding overhead` (workloads, dataset, seed and pass line as in `v1-python-binding`), with one extra metric: `perf_hooks.monitorEventLoopDelay` p99 during the commit workload. Pass line for event-loop delay: under 20 ms p99 while 200 `sync` commits run back to back.

## Failure model

- A thrown error inside `await using` aborts the transaction.
- A Rust panic rejects with `INTERNAL`, and the process survives.
- A process kill keeps the durable prefix of the durability level.
- The finalizer flush runs only on GC, so `close()` is documented as required for `ordered` batches to be flushed promptly.

## Risks / Trade-offs

- [The time limit across awaits will surprise users] → the `WRITE_TIMEOUT` message names `submit`.
- [The finalizer timing is unpredictable] → `close()` and `await using` are the documented path, and the finalizer is a safety net.
- [Six native targets to build] → napi-rs's CI templates, gated on the conformance corpus.

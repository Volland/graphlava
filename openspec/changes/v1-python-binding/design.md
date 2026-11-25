# Design

## Context

The core API is synchronous. One owner process holds each file ([[adr:0002]]). An interactive writer is held for at most 100 ms ([[16-api#Rust API]]), and rows come back as lazily decoded 1024-row batches. Python adds three complications: a GIL that serialises threads unless it is released, `os.fork` copying open handles into children, and arbitrary-precision `int`.

## Decisions

### D1. Bind the Rust API directly with PyO3 (ADR 0042)

- **Choice:** `crates/graphlava-py` depends on `graphlava` as a Rust crate and builds with maturin as an abi3-py310 extension.
- **Rejected:**
  - cffi over the C FFI. Results would be marshalled twice, and PyO3's panic and ownership handling would have to be written again by hand.
  - UniFFI. It has no Node backend, and it would add a second ABI next to the C ABI.
- **Reasons:** `motivated_by` [[16-api#Rust API]] ("every other binding wraps it"). ADR 0042 also covers the Node and mobile choices, so the later slices cite it.

### D2. Release the GIL around all core work

- **Choice:** every call that does I/O, parses, plans, executes or waits on a commit runs inside `Python::detach` (named `allow_threads` before PyO3 0.26), and the module declares `gil_used = false` for 3.14t.
- **Rejected:** holding the GIL (one slow `sync` commit would stall every Python thread), and an asyncio API in v1. An asyncio facade can wrap the threaded API later without core changes.
- **Reasons:** `motivated_by` `api-no-async-required`.

### D3. Interactive writes keep the 100 ms limit; `submit` is the main path

- **Choice:** `with db.write(...)` enforces the Rust time limit unchanged. The documentation and examples lead with `db.submit(...)` and compare-and-set.
- **Rejected:** a longer limit for Python. A slow callback would hold the single writer and stall group commit for every client.
- **Reasons:** `motivated_by` `api-write-in-process`, `api-one-shot` and [[adr:0026]].

### D4. IDs are Python `int`; values round-trip exactly

- **Choice:** IDs are unsigned 64-bit `int`. Integers inside ±2^57 go inline. Larger integers become dictionary `xsd:integer` literals and come back as equal `int` values.
- **Rejected:** truncating to i64, which would silently lose `2**70`.
- **Reasons:** `motivated_by` [[02-id-encoding#Bit layout]].

### D5. Detect `fork` instead of sharing handles

- **Choice:** a handle records the PID that opened it, and any call from another PID raises `ForkedHandleError` before it touches the file.
- **Rejected:**
  - Reopening the file silently in the child, which breaks the single owner.
  - An `os.register_at_fork` hook that closes the parent's handle, which breaks the parent.
- **Reasons:** `motivated_by` `api-single-owner` and [[adr:0002]].

## Constraints

| Constraint | Binds |
| --- | --- |
| CPython abi3 stable ABI (3.10+) | packaging, the PyO3 feature set |
| PEP 703 free-threading (3.14t) | GIL release, `Send + Sync` handle |
| manylinux_2_28 / musllinux_1_2 wheel policy | packaging |
| PEP 561 typed packages | `.pyi` stubs and `py.typed` |

## Formal model

None. The binding adds no state and no invariant. "Same effect as the Rust call" is checked by refinement, not proved, so no Lean module changes and no PO is opened.

New `<assumption>` entry for `tg.md/20-proofs.md`: a binding never calls the core with a handle opened by another process (D5).

## Refinement

The abstraction function is the identity on public API state. A trace from `lake exe oracle <seed> 2000` is replayed through Python. Every result, mapped back with `toRust`, must equal the Rust replay result, and so must the final dump.

The conformance corpus `bindings/conformance/*.jsonl` adds mapping cases that the oracle does not generate:

- ±2^57 and ±2^63 boundaries, and `2**70`.
- NaN, naive and aware datetimes, and `Decimal` scale.
- Empty results and 64-deep nesting.

The Rust runner validates the corpus first, so a corpus bug is not reported as a binding bug.

## Test strategy

| Requirement | L1 | L2 | L3 | L4 | L6 |
| --- | --- | --- | --- | --- | --- |
| API parity | oracle replay | corpus | agent memory session | — | — |
| Term mapping | — | corpus, Hypothesis | boundary round trips | — | — |
| Context managers | — | — | exception abort, time limit | SIGKILL after a `sync` commit | — |
| GIL release | — | — | parallel readers, 3.14t | — | — |
| Fork | — | — | fork and spawn tests | — | — |
| Row batches | — | corpus | Arrow, ids-only | — | — |
| Errors | — | — | guard and CAS errors | — | — |
| Packaging | — | — | wheel smoke test per target | — | — |
| Overhead | — | — | — | — | Binding overhead |

Crash point: after the Python process has printed an acknowledged `sync` commit ID.

## Cost model

The binding adds no B-tree operations, so each write costs what the Rust call costs. The overheads are:

- One fixed overhead per call: acquiring and releasing the GIL, plus argument conversion.
- One overhead per decoded term.
- In batch mode, one call per 1024 rows.

`to_arrow()` copies ID columns once and decodes terms column by column.

## Benchmarks

New section `Binding overhead` in `tg.md/17-benchmarks.md`. The Node and mobile slices reuse it.

- **Workloads:**
  - A `props()` point lookup.
  - An `out()` 2-hop traversal.
  - A 1-statement `ordered` submit.
  - A 10,000-row SELECT, read row by row and in batches.
- **Dataset:** the agent-memory generator, seed 42, 1M statements.
- **Metrics:** p50, p99 and p99.9 µs per call above the Rust baseline, recorded as HDR histograms.
- **Runner:** the fixed Linux x86_64 nightly runner.
- **Baseline:** the first nightly run. **Pass line:** p50 within +20% of the baseline at 95% confidence.

## Failure model

- An exception in a `with` block aborts the transaction, and nothing is written.
- A Rust panic becomes `InternalError`. The handle stays usable, or reports closed if the panic happened while it held the writer.
- A process kill keeps the durable prefix that the durability level promises.
- A handle that is garbage-collected without `close()` flushes the batch and releases the lock.

## Risks / Trade-offs

- [The 100 ms limit will surprise Python users] → `WriteTimeoutError` names `submit` in its message.
- [Arrow pulls in a large dependency] → `to_arrow()` is behind an optional feature and the `graphlava[arrow]` extra.
- [Free-threaded wheels are young] → 3.14t is tested in CI (PEP 779 made free-threading supported in CPython 3.14).
- [PyO3 0.30 builds free-threaded extensions only for CPython ≥ 3.14] → the free-threaded target is 3.14t, not 3.13t as first planned; abi3 wheels still cover 3.10 and later, including the GIL build of 3.13.

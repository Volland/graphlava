# Spec Delta

## Purpose

The Python package `graphlava`: a PyO3 extension over the Rust API that gives Python agent code the same data model, transactions, durability levels and query surfaces as Rust. Design: `tg.md/16-api.md`, this change's `design.md`.

## ADDED Requirements

### Requirement: Python API parity
The Python package SHALL expose `Database.open`, `read`, `write`, `submit`, `sparql`, `datalog` and `admin`, the core reads (`props`, `out`, `inc`, `stmt`, `hedge`, `lookup`), and every `CoreOp`. Each call SHALL have the same effect and return the same IDs and rows as the Rust call it wraps.

Formal: none — marshalling layer; the effect is the Rust call's, covered by the store and query requirements

#### Scenario: Agent memory session from Python
- **Level**: e2e
- **WHEN** a Python test opens `memory.glv` in a temp dir, inserts `(:alice, :knows, :bob)` with `Durability.ORDERED`, annotates the statement with `:confidence 0.9`, commits, closes and reopens the file, and runs `SELECT ?c WHERE { << :alice :knows :bob >> :confidence ?c }`
- **THEN** one row with `0.9` is returned, and the statement ID equals the one `insert` returned before the reopen

#### Scenario: Oracle trace replay through Python
- **Level**: refinement
- **WHEN** the oracle traces for seeds 1..16, with 2,000 operations each, are replayed through the Python API
- **THEN** every operation result and the final state match the Rust replay of the same trace

#### Scenario: Unknown keyword argument
- **Level**: unit
- **WHEN** `db.write(durability="fast")` is called
- **THEN** it raises `ValueError` naming the accepted values `sync`, `ordered` and `relaxed`, and no write transaction is opened

### Requirement: Python term mapping
The package SHALL map `str`, `bool`, `int`, `float`, `decimal.Decimal` and timezone-aware `datetime` to their XSD datatypes, and other terms to `IRI`, `BNode` and `Literal`. IDs SHALL be `int` holding the full unsigned 64-bit value. Every value SHALL round-trip exactly, stored inline or in the dictionary. A naive `datetime` and NaN SHALL be rejected.

Formal: none — host type conversion; inline routing itself is proved in `MetaCore.Id`

#### Scenario: Integers on both sides of the inline limit
- **Level**: e2e
- **WHEN** the values `2**57 - 1`, `-(2**57)`, `2**57` and `2**70` are stored as `:n` properties, the file is reopened and they are read back
- **THEN** each value comes back as an equal Python `int`, and a `FILTER(?n > 2**57 - 1)` query returns exactly the last two

#### Scenario: Naive datetime is rejected
- **Level**: unit
- **WHEN** `datetime(2026, 11, 24, 9, 0)` with no `tzinfo` is passed as an object
- **THEN** `graphlava.InvalidError` is raised and the transaction is unchanged

#### Scenario: Statement ID with the top tag bits set
- **Level**: unit
- **WHEN** a statement ID whose kind tag occupies bits 60–63 is returned to Python and passed back to `stmt()`
- **THEN** the same triple is returned, and the ID is a non-negative `int` below `2**64`

### Requirement: Python transactions as context managers
`with db.read() as r:` and `with db.write(Durability.SYNC) as w:` SHALL open a transaction. A `with` block that exits normally SHALL commit a write transaction, and an exception SHALL abort it and propagate. A transaction object used after its block ends SHALL raise `graphlava.TransactionClosedError`. An interactive write SHALL keep the writer time limit of the Rust API.

Formal: none — scoping sugar over `commit` and `abort`, whose atomicity is `MetaCore.Durability`

#### Scenario: Exception aborts the write
- **Level**: e2e
- **WHEN** a `with db.write(...)` block inserts 3 statements and then raises `RuntimeError`
- **THEN** `RuntimeError` reaches the caller, and after reopening the file none of the 3 statements exist

#### Scenario: Writer time limit applies to slow Python code
- **Level**: e2e
- **WHEN** a `with db.write(...)` block sleeps for 150 ms with the default 100 ms limit and then inserts
- **THEN** the insert raises `graphlava.WriteTimeoutError`, the transaction is aborted, and a later `db.submit(...)` from another thread commits

#### Scenario: Python process killed after a sync commit
- **Level**: crash
- **WHEN** a Python child process commits a `with db.write(Durability.SYNC)` block, prints the returned statement ID, and is killed with SIGKILL before closing
- **THEN** after reopening, the statement with that ID exists and the file opens without a recovery error

#### Scenario: Empty write block
- **Level**: unit
- **WHEN** a `with db.write(Durability.SYNC)` block does nothing
- **THEN** it commits without error and `file_size()` is unchanged

### Requirement: GIL released during core work
Every call that reads or writes the store, parses or plans a query, or waits on a commit SHALL release the GIL for its duration. The package SHALL be importable and correct on free-threaded CPython 3.14t without re-enabling the GIL.

Formal: none — host runtime concurrency, outside the model

#### Scenario: Readers run in parallel threads
- **Level**: e2e
- **WHEN** 8 Python threads each run 1,000 `out()` lookups on a 1M-statement file while a ninth thread waits on a `SYNC` commit
- **THEN** all threads finish with correct rows, and the wall time is below 0.5× the time of running the same lookups serially (on a machine with 8 or more cores)

#### Scenario: Free-threaded interpreter
- **Level**: e2e
- **WHEN** the package is imported on CPython 3.14t with `PYTHON_GIL=0`
- **THEN** `sys._is_gil_enabled()` stays `False` and the agent memory session scenario passes

### Requirement: Single owner across fork
The package SHALL keep the single-owner rule under `os.fork` and `multiprocessing`. A `Database` handle used in a process other than the one that opened it SHALL raise `graphlava.ForkedHandleError` and SHALL NOT touch the file. A second `Database.open` of the same file, whether from a child process or another interpreter, SHALL raise `graphlava.LockedError`.

Formal: none — OS process and lock behaviour, outside the model

#### Scenario: Child process uses inherited handle
- **Level**: e2e
- **WHEN** a process opens a database, forks, and the child calls `db.read()`
- **THEN** the child gets `ForkedHandleError`, and the parent's next `SYNC` commit succeeds and survives a reopen

#### Scenario: Second interpreter is refused
- **Level**: e2e
- **WHEN** a `multiprocessing` worker with the `spawn` start method opens a file the parent holds
- **THEN** the worker gets `LockedError` and the parent is unaffected

### Requirement: Python row batches
Query results SHALL be iterable row by row, and also SHALL be iterable in batches of up to 1024 rows. Terms SHALL be decoded lazily per column. `rows.ids()` SHALL return IDs without dictionary decoding. `rows.to_arrow()` SHALL return a `pyarrow.Table` built from the column batches. Iterating an exhausted or closed result SHALL stop cleanly.

Formal: none — result transport; row content is defined by the query requirements

#### Scenario: IDs without decoding
- **Level**: e2e
- **WHEN** a query returning 10,000 rows is consumed with `rows.ids()`
- **THEN** the debug-build decode counter `graphlava._debug.decode_count()` is unchanged

#### Scenario: Arrow export
- **Level**: e2e
- **WHEN** `SELECT ?s ?age WHERE { ?s :age ?age }` over 2,500 nodes is exported with `to_arrow()`
- **THEN** the table has 2,500 rows, the `age` column is `int64`, and the values equal the row-iterator values

#### Scenario: Empty result
- **Level**: unit
- **WHEN** a query matches nothing
- **THEN** iteration yields no rows and `to_arrow()` returns a table with the right column names and 0 rows

### Requirement: Python errors
Every Rust `Error` variant SHALL map to a subclass of `graphlava.Error`, with the variant's fields as attributes. Examples are `LockedError`, `ConflictError`, `WriteTimeoutError`, `NotFoundError(id)`, `GraphCycleError(child, parent)`, `FillsRoleError(item, hedge)` and `CorruptedError`. A Rust panic SHALL surface as `graphlava.InternalError` and SHALL NOT abort the interpreter.

Formal: none — error translation

#### Scenario: Hyperedge guard error carries IDs
- **Level**: e2e
- **WHEN** a node that fills a role of hyperedge `e` is deleted without detach
- **THEN** `FillsRoleError` is raised with `item` and `hedge` equal to the node ID and `e`, and the node still exists after reopening

#### Scenario: Compare-and-set conflict
- **Level**: e2e
- **WHEN** `submit` with `Update(id, o=2, expected=1)` runs after another client has set the value to 3
- **THEN** `ConflictError` is raised and the value is still 3

### Requirement: Python packaging
The package SHALL ship abi3 wheels for CPython 3.10 and later, and free-threaded 3.14t wheels. Wheels SHALL cover Linux x86_64 and aarch64 (manylinux_2_28 and musllinux_1_2), macOS arm64 and x86_64, and Windows x86_64. Each wheel SHALL include `py.typed` and `.pyi` stubs that pass `mypy --strict` on the examples. `graphlava.__version__` SHALL equal the core crate version.

Formal: none — packaging

#### Scenario: Wheel install and smoke test per target
- **Level**: e2e
- **WHEN** CI installs each built wheel into a clean virtualenv on its target and runs the agent memory session scenario
- **THEN** it passes on every target and `graphlava.__version__ == graphlava.core_version()`

#### Scenario: Unsupported interpreter
- **Level**: unit
- **WHEN** `pip install graphlava` runs on CPython 3.9
- **THEN** pip finds no matching distribution, because the sdist metadata declares `requires-python >= 3.10`

### Requirement: Python call overhead
The overhead the Python binding adds to a `props()` point lookup SHALL be measured as p50 and p99 in microseconds above the same Rust call. The dataset is 1M statements, the durability level is `ordered` for setup, and the hardware is the fixed Linux x86_64 nightly runner. Pass line: to be measured in the first run, then gated at +20% over that baseline. Benchmark: `tg.md/17-benchmarks.md#Binding overhead`.

Formal: none — performance

#### Scenario: Overhead benchmark runs nightly
- **Level**: benchmark
- **WHEN** the nightly runner runs `Binding overhead` for Python
- **THEN** p50 and p99 overhead are recorded with their HDR histograms, and the job fails if p50 exceeds the stored baseline by more than 20% at 95% confidence

#### Scenario: Batch path amortises overhead
- **Level**: benchmark
- **WHEN** 1,024 rows are read through `iter_batches()` and through row iteration
- **THEN** both counts are recorded, and the batch path's per-row overhead is reported next to the row path's

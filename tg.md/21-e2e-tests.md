---
type: Scenario
status: draft
specs: ["adr:0019"]
---
# End-to-end tests

The catalogue of end-to-end scenarios: each runs through public entry points on a real database file and checks what a user of meta-core would observe, including across process restarts and crashes.

- cites:: [[19-verification#Test pyramid]] {intent: usesMethodIn}

Rules for every scenario here: the test creates its file in a fresh temporary directory, uses only `Database`, `sparql()`, `datalog()` or the C FFI, never internal tables, and closes and reopens the file before its final assertion. Tests live in `crates/meta-core/tests/e2e/` and carry `@tg: verifies:: [[21-e2e-tests#<Scenario>]] {level: e2e}`. Once code exists, set `lat: require-code-mention: true` in this file so `tg check` reports any scenario without a test.

## Agent memory session

A complete agent session: facts, annotations, overwrites, nested episode graphs and queries, surviving a restart. This is the smoke test of the product promise.

- refines:: [[01-data-model#^dm-update-in-place]]
- refines:: [[09-graphs#^gr-scope-transitive]]
- refines:: [[01-data-model#^dm-promote-on-annotate]]

<given>

An empty file; graphs `:agent` and `:episode1` with `:episode1 :in :agent`.

</given>

<when>

In `:episode1`, insert `:alice :worksAt :acme {| :confidence 0.9 |}` and `:alice :age 42`; annotate `:age` with `:source :doc5`; update the employer to `:globex` through the same reifier; close and reopen; query `GRAPH :agent { << :alice :worksAt ?o ~ ?r >> . ?r :confidence ?c }` and the `:source` of `:age`.

</when>

<then>

One row `(:globex, 0.9)` with the reifier ID unchanged from before the update; the `:age` statement ID is stable and still carries `:source :doc5`; `GRAPH! :agent` returns no rows.

</then>

## Same answers on every surface

The same question asked through SPARQL, Datalog and `SERVICE <meta:datalog>` returns the same set, because all surfaces lower to one IR.

- refines:: [[01-data-model#^dm-props-virtual]]

<given>

A random graph of 10^4 `:knows` statements from a fixed seed, with literal properties and annotations.

</given>

<when>

Reachability from 20 random start nodes is asked as a SPARQL property path, as a standalone Datalog program, and through `SERVICE <meta:datalog>`.

</when>

<then>

The three result sets are equal for every start node.

</then>

## Model trace replay

The Lean oracle's traces, replayed through the public API on a real file with a close and reopen in the middle, give the model's results and final state.

- refines:: [[01-data-model#^dm-unique-spo]]
- refines:: [[02-id-encoding#^id-never-reused]]

<given>

The output of `lake exe oracle <seed> 2000` for seeds 1..64.

</given>

<when>

Each operation is applied in its own transaction; after operation 1,000 the database is closed and reopened.

</when>

<then>

Every operation's result and the final state through the abstraction function equal the trace.

</then>

## Crash at every commit point

Killing the writer at each step of a commit leaves a durable prefix of whole transactions, as the Lean durability model allows.

- refines:: [[06-transactions-durability#^tx-durability-prefix]]
- refines:: [[06-transactions-durability#^tx-atomic]]

<given>

A child process runs a scripted mix of `sync`, `ordered` and `relaxed` transactions of 1–1,000 statements and reports each acknowledgment over a pipe.

</given>

<when>

The parent kills the child with SIGKILL (TerminateProcess on Windows) at the N-th I/O step, for every N in the script, and reopens the file.

</when>

<then>

The state equals the first `k` transactions for some `k` at least the index after the last acknowledged `sync` or `ordered` commit; no ID of a lost transaction appears anywhere; the allocator counter is not below any ID present.

</then>

## Second process is refused

The owner-process lock holds across real processes on every OS.

- refines:: [[06-transactions-durability#^tx-single-writer]]

<given>

Process A has the file open with an active write transaction.

</given>

<when>

Process B opens the same path.

</when>

<then>

B gets the locked error within 100 ms; A commits normally and its data is intact.

</then>

## Portable file round trip

A file written on one OS is read with identical results on every other, which is the portable-format promise.

- refines:: [[06-transactions-durability#^tx-recovery-format]]

<given>

A golden file produced by the `e2e-golden` CI job on Linux from a fixed seed, plus its expected query results.

</given>

<when>

The macOS, Windows, iOS-simulator and Android-emulator jobs open it read-only and run the query set.

</when>

<then>

Every result equals the expected one, byte for byte after serialisation; opening a copy with an unknown feature flag fails and leaves the file unchanged.

</then>

## Graph scope and nesting

Transitive scope, shared children, cycle rejection and drop behave as specified through SPARQL.

- refines:: [[09-graphs#^gr-scope-transitive]]
- refines:: [[09-graphs#^gr-dag-cycle]]
- refines:: [[09-graphs#^gr-drop-recursive]]

<given>

`:episode1 :in :agent`, `:episode1 :in :project`, `:project :in :agent`, with statements in each graph.

</given>

<when>

Query `GRAPH :agent`; then add `:agent :in :episode1`; then drop `:project` recursively.

</when>

<then>

Each statement of `:episode1` appears once under `:agent`; the cycle-closing insert fails and nesting is unchanged; after the drop, `:episode1` survives because `:agent` is still its parent.

</then>

## Cascade and hyperedge guard

Deleting a statement removes its annotation chain but is blocked while a hyperedge role references it.

- refines:: [[06-transactions-durability#^tx-cascade-annotations]]
- refines:: [[06-transactions-durability#^tx-cascade-hyperedge-block]]

<given>

`#42` annotated by `#77`, `#77` by `#90`, and a `:Meeting` hyperedge with `#42` in its `:target` role.

</given>

<when>

`DELETE DATA` on `#42`; then the same delete with detach.

</when>

<then>

The first request fails and changes nothing; the second removes `#42`, `#77`, `#90` and the hyperedge in one transaction.

</then>

## C FFI lifecycle

A C program linked against the FFI library opens, writes, queries and closes, with no leak, so mobile bindings rest on a tested base.

- refines:: [[06-transactions-durability#^tx-single-writer]]

<given>

A C test program built in CI against `libmeta_core_ffi`.

</given>

<when>

It runs the agent-memory session scenario through the FFI under AddressSanitizer and LeakSanitizer.

</when>

<then>

Results equal the Rust scenario; the sanitizers report nothing; every error code is the documented one.

</then>

## Soak against the model

A long random agent-memory mix runs against the model, checking invariants all the way, to catch faults that only appear at scale.

- refines:: [[01-data-model#^dm-unique-spo]]
- refines:: [[01-data-model#^dm-delete-cascade]]

<given>

The agent-memory mix of [[17-benchmarks]] at 10^6 statements with readers on 4 threads.

</given>

<when>

It runs for 30 minutes nightly, with a sweep and a reopen every 5 minutes.

</when>

<then>

After every transaction the sampled invariants hold (uniqueness, no dangling annotation, allocator above every ID); readers never see a partial transaction.

</then>

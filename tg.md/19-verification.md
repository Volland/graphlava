---
type: Concept
status: draft
specs: ["adr:0019", "adr:0020"]
---
# Verification strategy

How graphlava argues that it is correct and fast: a Lean 4 model with machine-checked theorems, a refinement bridge from that model to the Rust code, and tests, crash injection, conformance suites and benchmarks around it.

- follows_from:: [[adr:0019]]
- follows_from:: [[adr:0020]]
- defines:: [[20-proofs]]
- defines:: [[21-e2e-tests]]

## Purpose

Tests sample behaviour; proofs cover every input of a model. graphlava needs both, because its core promises are universal: IDs are never reused, statements stay unique, nesting stays acyclic, acknowledged commits survive.

The goal is one argument per requirement, at the strongest level that is affordable. The proof ledger is [[20-proofs]]; the E2E catalogue is [[21-e2e-tests]]; the measurement plan is [[17-benchmarks]].

## Assurance levels

Seven levels, strongest first. Each requirement states its levels in its spec (the `Formal:` and `Level:` lines required by `openspec/config.yaml`).

| Level | Name | What it shows | Where |
| --- | --- | --- | --- |
| L0 | Lean proof | an invariant holds for **every** state and operation of the model | `formal/MetaCore/*.lean` |
| L1 | Refinement | the Rust store behaves like the model on seeded operation traces | `formal/Main.lean` → `crates/graphlava/tests/refinement.rs` |
| L2 | Property test | layout-level facts the model abstracts (orderings agree, inline routing, codecs) | `proptest` in each crate |
| L3 | End-to-end | a user-visible scenario works through the public API on a real file | [[21-e2e-tests]] |
| L4 | Crash / fault | recovery after `kill -9` at every commit point is a durable prefix | crash harness, child processes |
| L5 | Conformance | standard semantics (W3C SPARQL 1.1/1.2, Datalog corpus) | conformance runner |
| L6 | Benchmark | performance claims hold and do not regress | [[17-benchmarks]] |

<rule id="vf-invariant-needs-proof" strength="must">

A requirement that states an invariant (uniqueness, identity under update, order preservation, acyclicity, atomicity, durability, never-reuse) has an L0 theorem or an open proof obligation in [[20-proofs]]. Tests alone do not close it.

</rule>

<rule id="vf-e2e-per-requirement" strength="must">

Every requirement has at least one L3 or L4 scenario. A requirement proved in Lean still gets an end-to-end test, because the proof covers the model, not the build, the file or the FFI.

</rule>

## Formal model

The Lean model is the executable specification of the observable state. It uses core Lean 4 only (no Mathlib), so it builds in seconds and pins one toolchain in `formal/lean-toolchain`.

| Module | Models | Spec |
| --- | --- | --- |
| `MetaCore.Id` | kind tag, chunk split, inline integer encoding | [[02-id-encoding]] |
| `MetaCore.Store` | statements, uniqueness, update in place, cascade delete, allocator | [[01-data-model]] |
| `MetaCore.Nesting` | graph nesting DAG, transitive scope | [[09-graphs]] |
| `MetaCore.Durability` | commit log, crash outcomes, durable prefix | [[06-transactions-durability]] |

Bit fields are modelled arithmetically (`kind * 2^60 + payload`), which equals the bitwise layout for in-range fields and lets `omega` decide the obligations. Storage layout (orderings, property records) is deliberately absent: every layout must refine the same abstract state.

<rule id="vf-no-sorry" strength="must">

`formal/` builds with zero `sorry`, `admit` or user `axiom`. `formal/scripts/lean-audit.sh` prints the axioms of every headline theorem and fails on anything beyond `propext`, `Classical.choice` and `Quot.sound`.

</rule>

<rule id="vf-model-is-spec" strength="must">

When the Rust code and the model disagree, the model wins until a reviewed change updates it. A model change updates the theorem, the oracle and [[20-proofs]] together.

</rule>

## Refinement bridge

The bridge ties the proofs to the Rust code. `lake exe oracle <seed> <count>` runs the model on a seeded random operation sequence and prints one JSON object per line: a schema header, each operation with its expected result, and the final state.

The Rust replay test reads the same lines, applies each operation to a fresh database through the public API, and compares:

- the result of every operation (allocated ID, `ok` or the error kind, the IDs a cascade removed);
- the final state through the abstraction function `α : redb tables → Store`, which reads `stmt` and folds inline properties back into statements.

The simulation obligation per operation is: `α (rust_op s) = model_op (α s)`. A mismatch is a bug in the Rust code or in the model, and is treated as a failing test either way.

<rule id="vf-replay-volume" strength="should">

CI replays seeds 1..64 with 2,000 operations each on every pull request, and seeds 1..4,096 nightly. A failing seed is added to a fixed regression list.

</rule>

## Test pyramid

Each level catches different faults, so the counts differ: many fast property tests, a moderate number of E2E scenarios, few but exhaustive crash sweeps.

- **Unit and property (L2)**: codecs round-trip and preserve order on 10^7 random values per subtype; all orderings return the same set for every pattern; inline and promoted properties are indistinguishable to queries.
- **Refinement (L1)**: model traces as above.
- **End-to-end (L3)**: [[21-e2e-tests]]; each test opens a real file in a temporary directory, uses only public entry points (`Database`, `sparql()`, `datalog()`, C FFI), and closes and reopens the file before its final assertion.
- **Crash (L4)**: a parent process runs a child writer, kills it at the N-th commit step (lock, write, fsync, header flip) for every N, reopens and checks that the state equals the model's `recover` for some `k` with `durableBound ≤ k`.
- **Conformance (L5)**: W3C suites in direct `GRAPH` mode with a reviewed skip list ([[11-sparql#Conformance]]).

## CI gates

The checks every pull request must pass, and the slower ones that run nightly on a fixed machine.

| Gate | Runs | Fails when |
| --- | --- | --- |
| `cargo fmt`, `clippy -D warnings`, `cargo test` | every PR | any failure |
| `lake build` + `lean-audit.sh` | every PR | an error, `sorry` or an unexpected axiom |
| refinement replay (64 seeds) | every PR | any mismatch |
| E2E scenarios | every PR, 3 OSes | any failure |
| crash sweep | nightly | a recovered state outside the model's crash outcomes |
| instruction-count micro benches | every PR | > 3% more instructions than `main` ([[adr:0020]]) |
| macro benches | nightly, fixed runner | a regression beyond its threshold at 95% confidence |
| `tg check`, `tg ontology check`, `tg trace --gaps` | every PR | a new error or gap |

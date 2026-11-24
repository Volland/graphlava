# Proposal

## Why

Agent frameworks and MCP servers written in TypeScript need embedded agent memory, but graphlava is reachable only from Rust. Node.js runs JavaScript on one event loop, and its `number` loses precision above 2^53, while most graphlava IDs exceed 2^53 because the kind tag sits in bits 60–63. A naive binding would stall the event loop on every `sync` commit and corrupt IDs. This change adds the npm package `@graphlava/node` (scoped under the `graphlava` npm organization, published with public access), which keeps the data model, transactions, durability levels and single-owner rule ([[adr:0002]]) without either problem.

## What Changes

- `crates/graphlava-node`: a napi-rs addon that links the Rust API directly ([[adr:0042]], from `v1-python-binding`).
- Every call that can block returns a Promise and runs on a dedicated pool, off the event loop. Blocking variants exist only under `@graphlava/node/sync`.
- IDs are `bigint`. An integer literal is returned as `number` only when it is in the safe range.
- `WriteTxn` supports `await using`, and the 100 ms writer limit keeps running across awaits.
- Opening a file that is already open in the same process, including from a worker thread, rejects with `LOCKED`. Handles cannot be cloned to workers.
- Rows come as an async iterator and as `BigUint64Array` batches. Errors are `GraphlavaError` with a stable `code`.
- Prebuilt binaries for 6 targets on Node 20, 22 and 24, with ESM, CommonJS and `.d.ts`, as optional per-target packages `@graphlava/node-<platform>-<arch>[-<abi>]` (for example `@graphlava/node-darwin-arm64`, `@graphlava/node-linux-x64-gnu`), all with `publishConfig.access: public`.

Out of scope: browsers and WebAssembly, Deno and Bun-specific builds, a remote client mode, and the server crate.

## Capabilities

### New Capabilities

- `node-binding`: the npm package, its async model, ID and literal mapping, worker-thread rules, errors, packaging and call overhead.

### Modified Capabilities

None.

## Impact

- **Code:** `crates/graphlava-node`, `bindings/node/` (the JS entry points and types), and a CI prebuild job.
- **Dependencies:** `napi` and `napi-derive`.
- **Depends on:** `v1-python-binding` group 1 (ADR 0042, the conformance corpus, the `Binding overhead` section).
- **lat.md sections:** [[16-api]]`#Node.js binding`, [[17-benchmarks]]`#Binding overhead`, [[21-e2e-tests]], [[18-roadmap#v1]].
- **Binding constraints:** the [[16-api]] rules `api-single-owner`, `api-write-in-process`, `api-durability-per-txn`, `api-rows-late` and `api-no-async-required`.
- **Requirements added:** all are `[[openspec:node-binding#…]]`:
  - Node API parity
  - Event loop is never blocked
  - Interactive write transactions in Node
  - Node ID mapping
  - Node literal mapping
  - Single owner across workers and processes
  - Node row streaming
  - Node errors
  - Node packaging
  - Node call overhead
- **File format:** unchanged.
- **Performance:** checked by `Binding overhead`, which for Node also measures event-loop delay. [[17-benchmarks#Durability commit latency]] also runs through Node.
- **Verification:** `Formal: none` for every requirement, because the binding adds no invariant. Coverage by level:
  - L1: oracle replay for seeds 1..16.
  - L2: the conformance corpus and fast-check.
  - L3: e2e tests for every requirement.
  - L4: a SIGKILL after a `sync` commit.
  - L6: Binding overhead.

# Proposal

## Why

Most agent frameworks are written in Python, but graphlava is reachable only from Rust. [[16-api]] plans Swift and Kotlin bindings for v1 ([[adr:0013]]) but has no Python one. This change adds the Python package `graphlava`. It keeps the data model, transactions, durability levels and single-owner rule ([[adr:0002]]) and fits Python's GIL, `fork` and typing. It is the first of three binding slices, so it also lays the groundwork that `v1-node-binding` and `v1-mobile-bindings` reuse.

## What Changes

- `crates/graphlava-py`: a PyO3 extension built with maturin that links the Rust API directly, without the C FFI.
- Transactions are context managers, and every core call releases the GIL. The package supports free-threaded CPython 3.14t.
- `ForkedHandleError` keeps the single-owner rule across `fork`.
- Terms and IDs map exactly to Python types. Results come as 1024-row batches with Arrow export. Each `Error` variant has its own exception class.
- abi3 and 3.14t wheels for 5 targets, with `.pyi` stubs.
- **Shared groundwork:**
  - ADR 0042 "binding strategy": PyO3 and napi-rs over the Rust API, and the C FFI for mobile.
  - The language-neutral conformance corpus `bindings/conformance/`.
  - The `Binding overhead` benchmark section.
  - New `Python binding` and `Node.js binding` sections in [[16-api]].

Out of scope: a remote client mode, sharing one handle across processes ([[00-overview#Non-goals]]), and the server crate.

## Capabilities

### New Capabilities

- `python-binding`: the Python package, its type mapping, GIL and fork behaviour, errors, packaging and call overhead.

### Modified Capabilities

None.

## Impact

- **Code:** `crates/graphlava-py`, `bindings/conformance/`, and a CI wheel job.
- **Dependencies:** `pyo3`, `maturin`, and `arrow` (optional feature).
- **lat.md sections:** [[16-api]] (new `Python binding` section), [[17-benchmarks]] (new `Binding overhead` section), [[21-e2e-tests]], [[18-roadmap#v1]].
- **Binding constraints:** the [[16-api]] rules `api-single-owner`, `api-write-in-process`, `api-durability-per-txn`, `api-rows-late` and `api-no-async-required`.
- **Requirements added:** the following, all prefixed `openspec:python-binding#`:
  - `[[…#Python API parity]]`
  - `[[…#Python term mapping]]`
  - `[[…#Python transactions as context managers]]`
  - `[[…#GIL released during core work]]`
  - `[[…#Single owner across fork]]`
  - `[[…#Python row batches]]`
  - `[[…#Python errors]]`
  - `[[…#Python packaging]]`
  - `[[…#Python call overhead]]`
- **File format:** unchanged.
- **Performance:** checked by [[17-benchmarks]]`#Binding overhead` (new). The change could regress [[17-benchmarks#Durability commit latency]], so that benchmark is also run through Python.
- **Verification:** the binding adds no invariant, so its requirements have `Formal: none`. Coverage by level:
  - L1: oracle replay for seeds 1..16.
  - L2: the conformance corpus.
  - L3: e2e tests for every requirement.
  - L4: a SIGKILL after a `sync` commit.
  - L6: Binding overhead.

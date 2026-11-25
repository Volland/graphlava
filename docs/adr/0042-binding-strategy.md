---
status: accepted
date: 2026-11-24
decision-makers: [Volodia]
---
# Bindings: PyO3 and napi-rs over the Rust API, the C FFI for mobile

## Context and Problem Statement

v1 ships Python, Node.js, Swift and Kotlin bindings ([ADR-0013](0013-build-order-sparql-datalog-first.md)). Each host has its own threading model (Python's GIL and `fork`, Node's single event-loop thread and `worker_thread`s, the mobile lifecycle) and its own number types, so each binding could drift from the Rust semantics. We need one rule for how each binding reaches the core and which core rules every binding keeps unchanged.

## Considered Options

<option id="native-binders-plus-c-ffi" outcome="chosen">

Python binds the Rust API directly with PyO3 (built with maturin, abi3-py310 plus free-threaded 3.14t). Node binds it directly with napi-rs (N-API 9). Swift and Kotlin go through the one stable C ABI in `graphlava-ffi` (cbindgen header, hand-written Swift over a module map, Kotlin over a thin JNI shim). Every binding keeps these rules:

- The interactive writer time limit (default 100 ms) is the Rust one, unchanged. No binding lengthens it; each one documents and leads with one-shot `submit`.
- IDs are the full unsigned 64-bit value in every host: Python `int`, JS `bigint`, Swift `UInt64`, Kotlin `ULong`. No binding truncates or signs an ID.
- A binding detects use of a handle outside its owner: Python records the opening PID and raises `ForkedHandleError` after `fork`; Node keeps a process-global registry of open paths, so a second open from a worker is refused with `LOCKED`; mobile handles are not shared across processes and extensions never open the file.
- The language-neutral corpus `bindings/conformance/` is the shared contract. A Rust runner validates it against `graphlava` first, then each binding replays it.

</option>

<option id="c-ffi-everywhere" outcome="rejected">

Every binding over the C FFI (cffi for Python, `ffi-napi` or `koffi` for Node): results are marshalled twice (Rust to C, C to host), panic and ownership handling is rewritten by hand per host, and calls are slower.

</option>

<option id="uniffi" outcome="rejected">

UniFFI for all hosts: it has no Node backend, it adds a second ABI next to the C ABI, and its generated lifecycle hooks cannot place `mc_suspend` inside iOS `beginBackgroundTask`.

</option>

<option id="wasm-for-node" outcome="rejected">

A WebAssembly build for Node: no OS file lock and no `fsync` semantics, so neither the single-owner rule ([ADR-0002](0002-redb-single-owner-process.md)) nor the durability levels ([ADR-0012](0012-durability-levels-group-commit.md)) could be honoured.

</option>

## Decision Outcome

Chosen option: "native-binders-plus-c-ffi", because PyO3 and napi-rs link the Rust API with no second marshalling layer and handle panics and ownership already, while mobile platforms, which have no Rust-native binder, share one frozen C ABI that also serves C, Go and .NET callers. The shared rules keep the four bindings semantically identical to Rust.

### Consequences

<consequence kind="good">

One behaviour everywhere: the same IDs, the same writer limit, the same single-owner rule, checked by one shared conformance corpus and by oracle replay in each binding.

</consequence>

<consequence kind="good">

Python and Node calls pay one conversion per argument and per decoded term, measured by [[17-benchmarks#Binding overhead]].

</consequence>

<consequence kind="bad">

Three binding toolchains to maintain (maturin, napi-rs, cbindgen plus Swift and Gradle) and a wide wheel and npm target matrix.

</consequence>

<consequence kind="bad">

Hosts with a short-lived thread model (Python with slow callbacks, Node across awaits) hit `WriteTimeoutError` more often than Rust code; the bindings answer this with documentation, not a longer limit.

</consequence>

## More Information

Realised by `crates/graphlava-py`, `crates/graphlava-node` and `crates/graphlava-ffi`, and by the binding sections of [[16-api]]. Link code with `// @tg: implements:: [[adr:0042]]`.

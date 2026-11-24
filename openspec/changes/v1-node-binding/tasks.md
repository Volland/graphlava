# Tasks

Builds on the archived `2026-11-24-v0-query` change (task 4.1) and depends on `v1-python-binding` group 1 (ADR 0042, the conformance corpus, the `Binding overhead` section).

## 1. Specs and decisions

- [x] 1.1 Record D2–D5 as `type: Decision` sections (D1 is [[adr:0042]]) and the packaging constraints as `type: Constraint` sections, with `motivated_by::` and `constrains:: [[openspec:node-binding#<Requirement>]] {strength: must}` edges.
- [x] 1.2 Fill in the `Node.js binding` section of [[16-api]], and add the in-process single-owner `<assumption>` to [[20-proofs]].
- [x] 1.3 Add the Node cases (2^53, `-0`, `NaN`, `Infinity`, `Date` milliseconds) to `bindings/conformance/`, and the `Node agent memory session` scenario to [[21-e2e-tests]].

## 2. Failing tests (red)

- [x] 2.1 Scaffold `crates/graphlava-node` (napi-rs, N-API 9) and the npm package with per-target optional dependencies, ESM and CJS entry points, and `@graphlava/node/sync`.
- [x] 2.2 Write one `node:test` per scenario in `specs/node-binding/spec.md`.
- [x] 2.3 Write the oracle replay for seeds 1..16 × 2,000 operations, and the conformance corpus runner.
- [x] 2.4 Write the SIGKILL crash test and the 1 ms `setInterval` timer-gap test.

## 3. Implementation

- [x] 3.1 Implement `Database`, `submit`, the core reads and `CoreOp` objects as `AsyncTask`s on the writer thread and the read pool. Annotate `[[openspec:node-binding#Node API parity]]` and `[[openspec:node-binding#Event loop is never blocked]]`.
- [x] 3.2 Implement `WriteTxn` with `Symbol.asyncDispose` and the time limit across awaits. Annotate `[[openspec:node-binding#Interactive write transactions in Node]]`.
- [x] 3.3 Implement the `bigint` ID mapping. Annotate `[[openspec:node-binding#Node ID mapping]]`.
- [x] 3.4 Implement the literal mapping with the safe-integer rule. Annotate `[[openspec:node-binding#Node literal mapping]]`.
- [x] 3.5 Implement the process-global path registry, the non-cloneable handle and the finalizer flush. Annotate `[[openspec:node-binding#Single owner across workers and processes]]`.
- [x] 3.6 Implement the async row iterator, `BigUint64Array` batches, snapshot release on `return()`, and the debug snapshot counter. Annotate `[[openspec:node-binding#Node row streaming]]`.
- [x] 3.7 Implement `GraphlavaError` codes with `bigint` fields and the panic mapping. Annotate `[[openspec:node-binding#Node errors]]`.
- [x] 3.8 Write the `.d.ts` files and pass `tsc --strict` on the examples.
- [ ] 3.9 Set up the prebuild matrix (6 targets × Node 20/22/24) with the install smoke test and the unsupported-platform error. Annotate `[[openspec:node-binding#Node packaging]]`.
  - Configured, not verified here: `.github/workflows/node.yml` builds the six targets, runs `npm test` on Node 20/22/24 for each, and installs the packed tarballs (`@graphlava/node` + `@graphlava/node-<target>`) for the smoke test `bindings/node/test-support/smoke.mjs`. The unsupported-platform error is tested locally (`test/packaging.test.mjs`). Only darwin-arm64 on Node 26 was run locally; the matrix runs in CI only. Publishing to npm is out of scope.

## 4. Benchmarks

- [ ] 4.1 Add the Node `Binding overhead` bench with event-loop delay, record its baseline, and wire the nightly gate. Annotate `[[openspec:node-binding#Node call overhead]]`.
  - Bench `bindings/node/bench/overhead.mjs` (props() overhead vs the in-process Rust call, out() 2-hop, ordered submit, 10k-row SELECT rows vs batches, `monitorEventLoopDelay` p99 over 200 `sync` commits) and gate `bench/gate.mjs` are wired into the `bench` job of `.github/workflows/node.yml`. Open: the baseline `bench/baselines/binding-overhead-node.json` must come from the first run on the fixed Linux x86_64 nightly runner; local runs are not baselines.
- [ ] 4.2 Run [[17-benchmarks#Durability commit latency]] through Node and record the delta against Rust.
  - `bindings/node/bench/commit-latency.mjs` runs sync/ordered/relaxed through Node (1, 8, 64 callers) and through the blocking `submitSync` path. Open: record the delta on the fixed nightly hardware; only a short local development run exists.

## 5. Annotations and checks

- [x] 5.1 Annotate every Node test with `@tg: verifies:: [[openspec:node-binding#<Requirement>#<Scenario>]] {level: …}`.
- [x] 5.2 Add `node-binding` to the `openspec:` frontmatter of [[16-api]].
- [ ] 5.3 Notify the `site/` session about the npm package.
  - Left for the parent session.
- [ ] 5.4 Run `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`, `npm test`, `lake build`, `formal/scripts/lean-audit.sh`, `tg check`, `tg ontology check` and `tg trace node-binding --gaps`, and close every gap they report.

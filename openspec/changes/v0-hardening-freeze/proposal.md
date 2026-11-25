# Proposal

## Why

v0 ends with a frozen file format ([[adr:0018]]), and the freeze is only safe once every end-to-end scenario passes, crash and soak tests run, and the v0 benchmarks have stable nightly baselines ([[adr:0020]]). This last slice of the former `v0-core` change does that work. It changes no requirement, so it carries no spec deltas.

## What Changes

- The full [[21-e2e-tests]] catalogue with `require-code-mention`, the cross-OS golden-file job, the C FFI lifecycle test under ASan and LSan, and the nightly 30-minute soak.
- The `bench/` harness, criterion and `iai-callgrind` micro benches with the PR gate, SQLite / Oxigraph / tiramemsu adapters, LUBM / WatDiv / BSBM and SNB-style workloads, the fixed nightly runner and baselines.
- The format freeze ([[18-roadmap#Format freeze]]) and final traceability (`tg trace --gaps`).

Out of scope: vector maintenance and the scoped RaBitQ index (former task 15.11) are v1 ([[18-roadmap#v1]]).

## Capabilities

### New Capabilities

None.

### Modified Capabilities

None.

## Impact

- **Code:** `crates/graphlava/tests/e2e/`, `bench/`, CI workflows.
- **lat.md sections:** [[17-benchmarks]], [[18-roadmap]], [[21-e2e-tests]].
- **File format:** frozen at the end of this slice.
- **Verification:** L3, L4, L6 across all capabilities.

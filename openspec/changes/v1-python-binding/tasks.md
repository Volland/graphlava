# Tasks

Builds on the Rust API with `sparql()` and `datalog()` from the archived `2026-11-24-v0-query` change (task 4.1). This slice owns the shared binding groundwork in group 1, which `v1-node-binding` and `v1-mobile-bindings` reuse.

## 1. Shared groundwork

- [x] 1.1 Write ADR 0042 "binding strategy". It covers PyO3 and napi-rs over the Rust API, the C FFI for mobile, the 100 ms limit kept in every binding, 64-bit IDs in every host, and fork and worker detection.
- [x] 1.2 Add the `Python binding` and `Node.js binding` sections to [[16-api]], and the binding rows to [[18-roadmap#v1]].
- [x] 1.3 Record D1–D5 as `type: Decision` sections and the packaging constraints as `type: Constraint` sections. Use `motivated_by::` and `constrains:: [[openspec:python-binding#<Requirement>]] {strength: must}` edges, with only the edge names `tg ontology` lists.
- [x] 1.4 Add the fork `<assumption>` to [[20-proofs]].
- [x] 1.5 Create `bindings/conformance/` with a JSON-lines schema and at least 200 cases covering the edge values in design.md#Refinement. Add a Rust runner that validates the corpus against `graphlava`.
- [x] 1.6 Add the `Binding overhead` section to [[17-benchmarks]] and the `Python agent memory session` scenario to [[21-e2e-tests]].

## 2. Failing tests (red)

- [x] 2.1 Scaffold `crates/graphlava-py` (PyO3, maturin, abi3-py310) and `pyproject.toml`, with an empty module that imports.
- [x] 2.2 Write one pytest per scenario in `specs/python-binding/spec.md`.
- [x] 2.3 Write the oracle replay for seeds 1..16 × 2,000 operations, and the conformance corpus runner.
- [x] 2.4 Write the SIGKILL crash test: a child process commits with `sync`, prints the ID, and is killed; the parent reopens the file and checks for the ID.

## 3. Implementation

- [x] 3.1 Implement `Database`, the transactions as context managers, the core reads and `CoreOp`. Annotate `[[openspec:python-binding#Python API parity]]` and `[[openspec:python-binding#Python transactions as context managers]]`.
- [x] 3.2 Implement the term and ID mapping. Annotate `[[openspec:python-binding#Python term mapping]]`.
- [x] 3.3 Release the GIL in every core call and set `gil_used = false`. Annotate `[[openspec:python-binding#GIL released during core work]]`.
  - Note: the GIL is released in every core call and verified (writer-wait test, free-threaded 3.14t run). The scenario "Readers run in parallel threads" is `xfail`: the Rust core's read path does not scale yet (8 Rust threads take 0.77–0.98× the serial time on the same file), so no binding can reach 0.5×. PyO3 0.30 supports free-threading only on CPython ≥ 3.14, so the free-threaded target is 3.14t instead of 3.13t (spec, design and docs updated).
- [x] 3.4 Implement PID tracking and `ForkedHandleError`. Annotate `[[openspec:python-binding#Single owner across fork]]`.
- [x] 3.5 Implement rows, `iter_batches()`, `ids()` and `to_arrow()` (the `arrow` feature), and the debug decode counter. Annotate `[[openspec:python-binding#Python row batches]]`.
- [x] 3.6 Implement the exception hierarchy and the panic mapping. Annotate `[[openspec:python-binding#Python errors]]`.
- [x] 3.7 Write the `.pyi` stubs and `py.typed`, and pass `mypy --strict` on the examples.
- [ ] 3.8 Set up the cibuildwheel matrix (abi3 + 3.14t, 5 targets) with the install smoke test. Annotate `[[openspec:python-binding#Python packaging]]`.
  - Not done here: the matrix is configured (`pyproject.toml` `[tool.cibuildwheel]`, `.github/workflows/python-wheels.yml`: abi3 + 3.14t for Linux x86_64/aarch64 manylinux_2_28 and musllinux_1_2, macOS arm64/x86_64, Windows x86_64, smoke test in a clean venv) but only CI can run it. Verified locally: the abi3 wheel and the cp314t wheel on macOS arm64.

## 4. Benchmarks

- [ ] 4.1 Add the Python `Binding overhead` bench, record its baseline JSON, and wire the nightly gate. Annotate `[[openspec:python-binding#Python call overhead]]`.
  - Not done here: the bench (`crates/graphlava-py/bench/overhead.py` + `--example overhead_baseline`) and the nightly gate (`bench/scripts/nightly.sh`) are in place, and a short local run works. The baseline must come from the first run on the fixed Linux x86_64 runner; `bench/results/binding-overhead-python.baseline.json` is a placeholder until then.
- [ ] 4.2 Run [[17-benchmarks#Durability commit latency]] through Python and record the delta against Rust.
  - Not done here: the 1-statement `sync`/`ordered`/`relaxed` commits run through Python and Rust in the same bench; the delta is recorded only from the fixed runner, not from a local run.

## 5. Annotations and checks

- [x] 5.1 Annotate every Python test with `@tg: verifies:: [[openspec:python-binding#<Requirement>#<Scenario>]] {level: …}`.
- [x] 5.2 Add `python-binding` to the `openspec:` frontmatter of [[16-api]].
- [ ] 5.3 Notify the `site/` session about the PyPI package.
  - Left for the parent session.
- [x] 5.4 Run `cargo fmt --check`, `cargo clippy -- -D warnings`, `cargo test`, `pytest`, `lake build`, `formal/scripts/lean-audit.sh`, `tg check`, `tg ontology check` and `tg trace python-binding --gaps`, and close every gap they report.
  - `lake build` and `lean-audit.sh` not needed (no change under `formal/`). Remaining `tg check` findings are the `[[openspec:python-binding#…]]` edges in `tg.md/24-binding-constraints.md`, which resolve when the change is archived, plus the known `[[adr:…]]` and `pre-design.md` findings.

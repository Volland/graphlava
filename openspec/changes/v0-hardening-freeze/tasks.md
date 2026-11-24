# Tasks

Numbers in parentheses are the task ids of the former `v0-core` change.

## 1. End-to-end and crash

- [x] 1.1 Implement every scenario of [[21-e2e-tests]], each annotated `@tg: verifies:: [[21-e2e-tests#<Scenario>]] {level: e2e}`, then set `lat: require-code-mention: true` in that file (12.2)
- [ ] 1.2 Run the crash harness on Linux, macOS and Windows in CI (12.3)
- [x] 1.3 Add the cross-OS golden-file job and the C FFI lifecycle test under ASan and LSan (12.4)
- [x] 1.4 Add the nightly 30-minute soak against the model (12.5)

## 2. Benchmarks

- [x] 2.1 Create `bench/` (`meta-bench` binary): dataset generator, agent-memory mix, HDR histograms, open-loop driver, JSON results (13.1)
- [x] 2.2 Add criterion and `iai-callgrind` micro benches for [[17-benchmarks#Micro benchmarks]]; gate PRs on +3% instructions (13.2)
- [x] 2.3 Add adapters for SQLite (WAL), Oxigraph and tiramemsu behind the harness trait (13.3)
- [ ] 2.4 Wire LUBM, WatDiv and BSBM and the SNB-style traversal workload (13.4)
- [ ] 2.5 Run [[17-benchmarks#Inline literal range scan]], [[17-benchmarks#Durability commit latency]] and [[17-benchmarks#Dictionary sweep cost]] (10.2)
- [ ] 2.6 Set up the fixed nightly runner, run the scaling sweep, reader scaling, open and recovery time, and space under churn, and store baselines in `bench/results/` (13.5)
- [ ] 2.7 Turn each "to be measured" target into a number in a decision record once two nightly runs agree within their confidence intervals (13.6)

## 3. Freeze

- [ ] 3.1 Freeze the file format ([[18-roadmap#Format freeze]]) once the freeze criteria in design.md hold (10.3)
- [ ] 3.2 Run `tg trace` over all seven capabilities with `--gaps` and close every gap (14.3)

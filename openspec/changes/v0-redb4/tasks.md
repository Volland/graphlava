# Tasks

## 1. Toolchain

- [ ] 1.1 Raise the MSRV to 1.90 in `Cargo.toml`, the CI MSRV job, `openspec/config.yaml`, the README and [[22-implementation-decisions]]
- [ ] 1.2 Check that every workspace crate and the binding crates build on 1.90

## 2. Engine

- [ ] 2.1 Move the storage adapter and the fault-injecting backend to redb 4.6.x
- [ ] 2.2 Map the durability levels without `Durability::Eventual` and keep quick repair (rule `tx-quick-repair`)
- [ ] 2.3 Re-verify rule `tx-durability-verify` on 4.x with the bake-off harness: durable commit after non-durable ones, savepoint after write, crash repair at 10M and 100M

## 3. Checks

- [ ] 3.1 Run the crash harness (both modes and quick repair), the refinement replay, the e2e suite, the W3C suites and the golden-file round trip
- [ ] 3.2 Rerun bake-off runs A and C on 4.x and record the results in [[22-implementation-decisions]]
- [ ] 3.3 Run `tg check`, `tg ontology check` and `tg trace` over all capabilities with `--gaps`

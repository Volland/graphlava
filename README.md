# meta-core

[![CI](https://github.com/Volland/graphlava/actions/workflows/ci.yml/badge.svg)](https://github.com/Volland/graphlava/actions/workflows/ci.yml)

A mutable, single-file metagraph database in Rust, built for agent memory: many small transactions, frequent value overwrites, point lookups, short traversals and star patterns. It stores statements about statements, n-ary hyperedges and nested named graphs, and queries them with SPARQL 1.2 and Datalog. Cypher and a vector index follow in v1. `meta-core` is a working name.

Status: design. The Rust workspace and CI are set up; no engine implementation exists yet beyond the planned engine bake-off spike.

## Where things are

| Path | Content |
| --- | --- |
| [tg.md/00-overview.md](tg.md/00-overview.md) | start here: goals, architecture, spec map |
| [tg.md/](tg.md/) | the specifications `00`–`18` as a typed graph (Requirement, Constraint, Benchmark, Change sections) |
| [tg.md/open-questions.md](tg.md/open-questions.md) | design questions still open, with options and recommendations |
| [docs/adr/](docs/adr/) | architecture decision records 0002–0018 |
| [openspec/changes/v0-core/](openspec/changes/v0-core/) | the v0 change: proposal, capability specs with scenarios, design, tasks |
| [crates/meta-core/](crates/meta-core/) | the embeddable core crate (placeholder) |
| [crates/meta-core-ffi/](crates/meta-core-ffi/) | C FFI, built as `libmeta_core_ffi` for desktop, iOS and Android |
| [.github/workflows/](.github/workflows/) | CI (Linux, macOS, Windows, iOS, Android, MSRV, cargo-deny, `tg` checks) and tag-driven releases |
| [CONTEXT.md](CONTEXT.md) | glossary of domain terms |
| [tg.md/pre-design.md](tg.md/pre-design.md) | the original design handover |

## Working with the docs

The specs are checked with the `tg` CLI: `tg check`, `tg ontology check`, `tg trace`. The project's own ontology types (`OpenQuestion`, `Benchmark`) and edges (`follows_from`, `resolved_by`, `blocks`, `measured_by`) are declared in [Types/meta-core-types.md](Types/meta-core-types.md).

## Building

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

Rust 1.85+ (edition 2024). Pushing a `v*` tag builds `libmeta_core_ffi` for every desktop and iOS target and attaches the archives to a GitHub release.

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT) at your option.

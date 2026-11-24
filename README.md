<p align="center"><img src="site/assets/logo.svg" width="180" alt="graphlava: a diamond of baklava whose pastry layers are statements, with four nodes joined to a pistachio hyperedge on top"></p>

<h1 align="center">graphlava</h1>

<p align="center"><em>Layered like baklava. Linked like a graph.</em></p>

<p align="center"><strong>An embedded metagraph database in Rust, built for agent memory.</strong><br>
Statements carry statements, hyperedges are stored once, and graphs nest inside graphs, all in one file, queried with SPARQL&nbsp;1.2 and Datalog.</p>

<p align="center">
<a href="https://graphlava.org">Website</a> ·
<a href="https://graphlava.org/book/">Mini book: hypergraphs and metagraphs in statement graphs</a> ·
<a href="https://graphlava.org/articles/">Articles</a> ·
<a href="tg.md/00-overview.md">Design</a>
</p>

<p align="center"><a href="https://github.com/Volland/graphlava/actions/workflows/ci.yml"><img src="https://github.com/Volland/graphlava/actions/workflows/ci.yml/badge.svg" alt="CI"></a></p>

> **Status: v0 in progress, pre-release.** The store core, named and nested graphs, hyperedges, SPARQL 1.2 and Datalog are implemented and tested against a Lean 4 model, the W3C SPARQL suites and a crash harness; a C FFI exists, and Python, Node.js, Swift and Kotlin bindings are being built. The file format is not frozen yet and nothing is published to install yet. Start with the [core crate's README](crates/graphlava/README.md).

## Why

An agent's memory is not a pile of triples. It needs to say *how sure am I* and *where did I read it* about a fact, record a meeting between three people as one thing, and keep an episode as a graph inside a bigger graph. Those are the three powers of a metagraph, and graphlava is designed to store each of them natively instead of through glue code:

| Power | What you write | How graphlava is designed to store it |
| --- | --- | --- |
| Statements about statements | `:alice :worksAt :acme {\| :confidence 0.9 \|}` | Every statement has a stable 64-bit ID. The ID is the RDF 1.2 reifier, and it survives value updates, so annotations stay attached. |
| n-ary hyperedges | a meeting with an agent, a target and a topic | One record with typed roles and an incidence index per role. Role triples stay virtual until you annotate one. |
| Nested graphs | an episode inside a memory inside an agent | Graphs are nodes and nest as a DAG. `GRAPH :g` sees g and everything under it; home-graph membership costs zero bytes per statement. |
| Meta-model as data | predicates, hyperedge types, shapes, rules | Statements in a system graph, read by the planner and the write path. |

The workload it is tuned for: many small write transactions, frequent value overwrites, point lookups, 2–3 hop traversals and star patterns, with targets of 10M to 1B statements per file on desktop and 1–5 GB files on phones (to be measured).

## Features (designed)

- **One file, one owner process** on redb, with `sync`, `ordered` or `relaxed` commits per transaction.
- **SPARQL 1.2 and Datalog in v0**, a Cypher/GQL subset in v1, all compiled to one extended-Datalog IR.
- **Rules inside the engine**: on-demand Datalog with magic sets in v0, materialized rules maintained incrementally in v1.
- **Mutable current state**: update a fact in place and keep its ID; delete cascades through its annotations.
- **Scope-aware vector search** beside the graph (v1).
- **Five platforms**: Linux, macOS, Windows, iOS and Android, with a C FFI and Swift and Kotlin bindings (v1).

## Read

- **[Mini book: Hypergraphs and Metagraphs in Statement Graphs](https://graphlava.org/book/)**: 11 short chapters from binary edges to recursive metagraphs, and how to model each part with statements and in graphlava.
- **[Statement graphs: the one idea under graphlava](https://graphlava.org/articles/statement-graphs.html)**
- **[graphlava and tiramemsu](https://graphlava.org/articles/graphlava-and-tiramemsu.html)**: the sibling project. [tiramemsu](https://www.tiramemsu.com) keeps every change with two clocks; graphlava keeps only the current state.
- **[graphlava and Neptune OneGraph](https://graphlava.org/articles/graphlava-and-neptune.html)**: what an embedded store takes from Amazon's 1G model.
- **[Metagraph for AI Agents](https://leanpub.com/metagraphforaiagents)** by Volodymyr Pavlyshyn: the book behind the modeling ideas.

The site's source is in [site/](site/).

## Where things are

| Path | Content |
| --- | --- |
| [tg.md/00-overview.md](tg.md/00-overview.md) | start here: goals, architecture, spec map |
| [tg.md/](tg.md/) | the specifications `00`–`21` as a typed graph (Requirement, Constraint, Benchmark, Proof, Scenario, Change sections) |
| [tg.md/19-verification.md](tg.md/19-verification.md) | assurance strategy: Lean proofs, refinement replay, E2E, crash tests, benchmark gates |
| [formal/](formal/) | Lean 4 model of the store with machine-checked invariants and the trace oracle |
| [tg.md/open-questions.md](tg.md/open-questions.md) | design questions still open, with options and recommendations |
| [docs/adr/](docs/adr/) | architecture decision records 0002–0042 |
| [openspec/config.yaml](openspec/config.yaml) | project context and per-artifact rules every OpenSpec proposal, spec, design and task list must follow |
| [openspec/specs/](openspec/specs/) | the capability specs of finished work, with scenarios and their Lean theorems |
| [openspec/changes/](openspec/changes/) | changes in progress (hardening and freeze, the redb 4.x upgrade, the v1 bindings) and the [archive](openspec/changes/archive/) of finished ones |
| [crates/graphlava/](crates/graphlava/) | the embeddable core crate: store, graphs, hyperedges, SPARQL 1.2, Datalog ([README](crates/graphlava/README.md)) |
| [crates/graphlava-ffi/](crates/graphlava-ffi/) | C FFI, built as `libgraphlava_ffi` for desktop, iOS and Android |
| [crates/graphlava-py/](crates/graphlava-py/) | Python binding (PyO3), published on PyPI as `graphlava` |
| [crates/graphlava-node/](crates/graphlava-node/), [bindings/node/](bindings/node/) | Node.js binding (napi-rs), published on npm as `@graphlava/node` |
| [bindings/swift/](bindings/swift/), [bindings/kotlin/](bindings/kotlin/) | Swift package and Android AAR over the C FFI |
| [bindings/conformance/](bindings/conformance/) | the language-neutral conformance corpus every binding runs |
| [.github/workflows/](.github/workflows/) | CI (Linux, macOS, Windows, iOS, Android, MSRV, cargo-deny, `tg` checks) and tag-driven releases |
| [CONTEXT.md](CONTEXT.md) | glossary of domain terms |
| [site/](site/) | source of [graphlava.org](https://graphlava.org): website, articles and the mini book |
| [tg.md/pre-design.md](tg.md/pre-design.md) | the original design handover |

## Working with the docs

The specs are checked with the `tg` CLI: `tg check`, `tg ontology check`, `tg trace`. The project's own ontology types (`OpenQuestion`, `Benchmark`, `Proof`) and edges (`follows_from`, `resolved_by`, `blocks`, `measured_by`, `proves`) are declared in [Types/meta-core-types.md](Types/meta-core-types.md).

## Building

```sh
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
(cd formal && lake build && ./scripts/lean-audit.sh)   # Lean 4 proofs
```

Rust 1.87+ (edition 2024). Pushing a `v*` tag builds `libgraphlava_ffi` for every desktop and iOS target and attaches the archives to a GitHub release.

## Benchmarks

Macro benchmarks live in [bench/](bench/), a separate Cargo workspace (excluded from the root one, so the MSRV and root CI never build it) with the `meta-bench` binary: the agent-memory mix, LUBM, WatDiv, BSBM, an SNB-style traversal workload and the graphlava-specific benchmarks of [tg.md/17-benchmarks.md](tg.md/17-benchmarks.md), with adapters for graphlava, SQLite, Oxigraph and (feature `tiramemsu`) tiramemsu. Results are JSON files with raw HDR histograms in `bench/results/`. How to run them: [bench/README.md](bench/README.md).

```sh
cd bench && cargo build --release
./target/release/meta-bench mix --adapters graphlava,sqlite,oxigraph --statements 100000
```

## License

Licensed under either of [Apache License 2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT) at your option.

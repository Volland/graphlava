# meta-bench: graphlava macro benchmarks

The macro tier of [ADR 0020](../docs/adr/0020-benchmark-regression-gates.md): wall-clock workloads with HDR histograms, at least 5 runs per configuration, median with a bootstrap 95% confidence interval, run nightly on one fixed machine. Workloads, metrics and pass lines are in [tg.md/17-benchmarks.md](../tg.md/17-benchmarks.md). The micro tier (criterion and `iai-callgrind`, per pull request) lives in `crates/graphlava/benches/`.

`bench/` is its own Cargo workspace and is listed in the root `exclude`, so the root build, root CI and the 1.87 MSRV never compile it or its competitors.

## Layout

| Path | Content |
| --- | --- |
| `src/main.rs` | CLI, one subcommand per benchmark |
| `src/gen.rs` | dataset generator `glv-agent-v1` (seeded, versioned; derived from `spikes/bakeoff/src/data.rs`) |
| `src/mix.rs` | agent-memory mix `agent-mix-v1`, open-loop and closed-loop drivers |
| `src/stats.rs` | HDR histograms (V2 deflate, base64), median and bootstrap CI |
| `src/env.rs` | environment capture, page-cache drop, OS write counters |
| `src/results.rs` | result files |
| `src/adapter/` | the harness trait and the graphlava, SQLite, Oxigraph, tiramemsu adapters |
| `src/suite/` | `mix`, `scaling`, `readers`, `open-recovery`, `churn` (`mixbench.rs`); `range-scan`, `commit-latency`, `sweep-cost` (`tasks.rs`) |
| `src/workload/` | LUBM, WatDiv, BSBM, SNB-style traversal |
| `scripts/` | `fetch-lubm.sh`, `fetch-watdiv.sh`, `fetch-bsbm.sh`, `nightly.sh` |
| `data/` | generators, downloaded and generated data, scratch databases (gitignored) |
| `results/` | result JSON files (committed baselines) |

## Build

```sh
cd bench
cargo build --release                       # graphlava, SQLite, Oxigraph (RocksDB)
cargo build --release --features tiramemsu  # + tiramemsu (git dependency, Rust 1.88+)
cargo build --release --no-default-features # Oxigraph in memory instead of RocksDB
```

Building the default features compiles RocksDB (C++) once.

## Adapters

All engines sit behind `adapter::Adapter` (owns the database, makes per-thread sessions) and `adapter::Session` (agent-memory primitives, bulk load, SPARQL, SPARQL update, Datalog, range scan). A missing surface returns `Unsupported`; the result records it instead of a number.

| Adapter | Version and storage | `sync` | `ordered` | `relaxed` |
| --- | --- | --- | --- | --- |
| graphlava | path dependency `../crates/graphlava`, redb file | `Durability::Sync` | `Durability::Ordered` (group commit, `--group-window-ms`) | `Durability::Relaxed` |
| sqlite | rusqlite 0.40 bundled SQLite, WAL | `synchronous=FULL` | `synchronous=FULL` (no group commit; mismatch recorded) | `synchronous=NORMAL` |
| oxigraph | 0.4.11, RocksDB directory (in memory without `oxigraph-rocksdb`) | no per-commit fsync: WAL write without sync (mismatch recorded) | same | WAL write without sync |
| tiramemsu | git `Volland/tiramemsu`, SQLite backend | `Immediate` per database | `Immediate` (mismatch recorded) | `Normal` per database |

Every result entry carries `durability.matches` (or `durability_matches`) per adapter.

SQLite schema (the "triple table with the three orderings as indexes" of the spec): a `term` dictionary keyed by the N-Triples form with a numeric column for range scans; `stmt(id, s, p, o, olit)` with indexes `spo`, `pos`, `osp`; `member(g, stmt)` for named-graph membership; `nest(child, parent)` for graph nesting, expanded by a recursive CTE in scoped reads. Statements about statements use the subject `<urn:glv-bench:stmt:ID>`. The full DDL is in `src/adapter/sqlite.rs` and in each result's `describe.schema`.

Oxigraph is pinned to 0.4.11 on purpose: 0.5.x pins `spargebra =0.4.8` with feature `sep-0002`, which Cargo unifies into graphlava's own spargebra and graphlava then fails to compile (`query/func.rs`: non-exhaustive match, `Function::Adjust` not covered). Oxigraph 0.4 uses spargebra 0.3, a separate crate. Oxigraph has no graph nesting, so scoped reads union the episode graphs of the scope; annotations use RDF-star quoted triples.

tiramemsu: the dependency is a git dependency on `https://github.com/Volland/tiramemsu` (commit in `Cargo.lock`). Cargo resolves optional git dependencies even when the feature is off, so `cargo build` fetches the repository once into `~/.cargo/git`; it compiles only with `--features tiramemsu`. Scoped star patterns are `Unsupported` on tiramemsu; statement-by-ID reads go through the triple (no public read-by-eid on `View`).

## Common options

Every benchmark subcommand takes:

| Option | Default | Meaning |
| --- | --- | --- |
| `--adapters` | `graphlava` | comma-separated engines |
| `--runs` | 5 | runs per configuration (spec minimum 5) |
| `--durability` | `ordered` | level of timed writes |
| `--cache` | `warm` | `cold` reopens the engine and drops the page cache (`sudo -n purge` on macOS, `drop_caches` on Linux) before every run; when not permitted the run records `warm (cache drop not permitted)` |
| `--seed` | 42 | generator and driver seed |
| `--work-dir` | `bench/data/work` | database files |
| `--out` | `bench/results` | result directory |
| `--no-save` | off | print a summary instead of writing a result |
| `--keep` | off | keep database files |
| `--group-window-ms` | 2 | graphlava group-commit window |
| `--cache-bytes` | 64 MiB | graphlava page cache (SQLite uses 64 MiB too) |

## Benchmarks

### Agent-memory mix (`mix`)

Dataset `glv-agent-v1` (see `src/gen.rs`): N node-valued statements, N/8 nodes with four integer properties each, 64 skewed predicates, hub-skewed objects, every statement in its subject's episode graph, episodes nested under agents. Loaded through each adapter in write batches at `relaxed`, then one durable checkpoint.

Mix `agent-mix-v1` (per mille): 300 property lookups, 300 statement-by-ID reads, 150 traversals of 1–3 hops, 100 star patterns of 4–6 predicates (half of traversals and stars inside a `GRAPH` scope: the node's episode or its agent), 75 write transactions of 1–20 statements (70% value overwrites, 20% inserts, 10% deletes), 25 memberships of an existing fact in a second episode, 50 annotations.

Latency mode is open loop: operation i is scheduled at `t0 + i / rate`, its latency is completion minus scheduled time, so a stall is charged to every operation queued behind it (no coordinated omission). The service time is recorded separately. `--rate 0` is the closed-loop throughput mode.

```sh
meta-bench mix --adapters graphlava,sqlite,oxigraph,tiramemsu --statements 10000000 --ops 100000 --rate 2000
meta-bench mix --adapters graphlava,sqlite,oxigraph --statements 10000000 --ops 1000000 --rate 0 --threads 4
meta-bench mix --adapters graphlava,tiramemsu --statements 1000000 --versions 100   # Versus tiramemsu
```

### Task 2.5

```sh
# Inline literal range scan: int, dateTime, inline double, dictionary double; 0.01%, 1%, 10%; with and without value index
meta-bench range-scan --adapters graphlava --nodes 10000000
# Durability commit latency: 1/8/64 callers x 1/2/5 ms windows x sync/ordered/relaxed, plus bytes per request at batch 1/8/64
meta-bench commit-latency
# Dictionary sweep cost: 10M and 100M statements, 10^6 and 10^7 terms, 10% unreferenced
meta-bench sweep-cost --statements 10000000 --terms 1000000
meta-bench sweep-cost --statements 100000000 --terms 10000000
```

`range-scan` measures two paths: `native` (`Reader::find_range`, inline values only, so dictionary doubles return 0 rows there) and `sparql` (`FILTER(?v > c)`). Rows scanned per row returned is recorded as `null`: graphlava exposes no executor counter yet. `commit-latency` reports durable batch commits per second as the fsync proxy (`Database::durable_commits`); count real fsyncs with `dtruss`/`strace` on the runner. Bytes per request use graphlava's I/O-counting backend, as `crates/graphlava/examples/batch_bytes.rs`. Both `commit-latency` and `sweep-cost` are graphlava only.

### Nightly sections

```sh
meta-bench scaling --adapters graphlava,sqlite --sizes 100000,1000000,10000000,100000000,1000000000
meta-bench readers --adapters graphlava,sqlite,oxigraph --statements 10000000 --readers 0,1,2,4,8,16,32,64 --writer-rate 5000
meta-bench open-recovery --adapters graphlava,sqlite,oxigraph,tiramemsu --sizes 1000000,10000000,100000000
meta-bench churn --adapters graphlava,sqlite --statements 10000000 --ops 100000000
```

These are skeletons: they run end to end and record what the public APIs expose. Gaps: pages read during open (needs OS I/O tracing), free-page ratio and compaction (graphlava has no public `compact()` or free-page counter), planner versus executor time share.

### Standard workloads

```sh
scripts/fetch-lubm.sh 10                  # official UBA 1.7 (Java)
meta-bench lubm --adapters graphlava,oxigraph --universities 10

scripts/fetch-watdiv.sh 100               # official WatDiv v0.6, built in Docker
meta-bench watdiv --adapters graphlava,oxigraph --scale 100

scripts/fetch-bsbm.sh 28480               # official BSBM tools (Java); >= 2000 products for the update stream
meta-bench bsbm --adapters graphlava,oxigraph --products 28480

meta-bench snb --adapters graphlava,oxigraph --sf 1 --readers 1,4,16
```

- LUBM: the 14 queries in SPARQL on every SPARQL engine; with `--rdfs` (default) also as Datalog over an RDFS closure (subClassOf, subPropertyOf, domain, range) on graphlava. LUBM's reference answers need OWL, so Q11–Q13 stay incomplete under RDFS. Per-query row counts are recorded for cross-engine checks.
- WatDiv: every file in `data/watdiv/sf<S>/queries/` is a template; its instances (each starting at `SELECT`) are run.
- BSBM: the official test driver needs a SPARQL endpoint; the harness drives the explore mix itself (BSBM 3.1 order, Q6 removed), drawing parameters from the loaded data with a fixed seed, and reports query mixes per hour. The explore-and-update mix adds an `INSERT DATA` of the next chunk of `dataset_update.nt` and one `DELETE WHERE` per mix. `%currentDate%` is fixed at 2008-06-20.
- SNB: an own seeded generator (`snb-lite-v1`, LDBC RDF shape, not the LDBC datagen); IS1–IS7 in SPARQL, IS1/3/4/5/6 also in Datalog on graphlava; one writer issuing person/knows/post/comment inserts at `--writer-rate` while 1–16 readers run the short reads.

SQLite has no SPARQL front end and is skipped by these suites.

### Utilities

```sh
meta-bench env                                 # the environment block
meta-bench gen --statements 1000000 --out x.nq # the mix dataset as N-Quads
```

## Result format

`results/<YYYY-MM-DD>-<commit12>-<host>.json`, one file per day, graphlava commit and host; every invocation appends one entry:

```json
{
  "schema": "meta-bench/v1",
  "date": "2026-11-24", "graphlava_commit": "<sha>", "host": "<host>",
  "benchmarks": [{
    "benchmark": "agent-mix | range-scan | commit-latency | sweep-cost | scaling | reader-scaling | open-recovery | space-churn | lubm | watdiv | bsbm | snb-traversal",
    "created": "<RFC 3339 UTC>",
    "env": {
      "host": "...", "graphlava_commit": "<sha>", "graphlava_dirty": false,
      "cpu": {"model", "physical_cores", "logical_cores", "ram_bytes", "...": "perf levels / governor"},
      "os": {"family", "arch", "kernel", "product", "uname"},
      "storage": {"dir", "device", "mount_point", "mount", "fs", "device_info"},
      "build": {"rustc", "rustc_commit", "llvm", "target", "profile", "opt_level", "rustflags", "lto", "features", "sqlite_version", "oxigraph_version"},
      "load_avg": "..."
    },
    "config": {"common": {"adapters", "runs", "durability", "cache", "seed", "group_window_ms", "cache_bytes", "generator": "glv-agent-v1", "mix": "agent-mix-v1"}, "benchmark": "<subcommand options>"},
    "result": {"adapters": [{
      "adapter": "graphlava", "describe": {"...": "version, storage, durability mapping"},
      "durability": {"requested": "ordered", "matches": true},
      "load": {"statements", "load_s", "disk_bytes", "bytes_per_statement"},
      "summary": {"<class>.p99_us": {"median": 0.0, "ci95": [0.0, 0.0], "n": 5, "runs": [0.0]}},
      "runs": [{"run": 0, "cache_state": "warm|cold", "latency": {"<class>": {"count", "p50_us", "p90_us", "p99_us", "p999_us", "max_us", "mean_us", "hdr_v2_deflate_b64"}}}]
    }]}
  }]
}
```

`hdr_v2_deflate_b64` is the base64 of the HdrHistogram V2 deflate encoding (nanoseconds, 1 ns to 10 min, 3 significant digits); any HdrHistogram implementation can decode it (`stats::hist_from_b64` here). Every summary metric holds the per-run values, the median and the percentile-bootstrap 95% CI of the median (10 000 resamples, fixed seed). Latency classes per benchmark: the mix uses `props_get`, `stmt_get`, `hop`, `star`, `write_txn`, `membership`, `annotate`; the query suites use the template names (`Q1`… or `L1`… or `IS1`…); `all` merges them.

## Nightly runner

`scripts/nightly.sh` is what the fixed runner executes: it builds with `--features tiramemsu`, fetches missing data, and runs every section above with `--runs 5 --cache cold`. `QUICK=1` runs reduced sizes. The runner must be idle, have passwordless `purge` or `drop_caches`, Java for LUBM and BSBM, and Docker for WatDiv.

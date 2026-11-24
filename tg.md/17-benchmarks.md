---
type: Benchmark
status: planned
gate: v0
specs: ["adr:0002", "adr:0005", "adr:0009", "adr:0011", "adr:0012", "adr:0015", "adr:0016", "adr:0013", "adr:0020"]
---
# Benchmarks

The measurements that must run before graphlava's performance claims are trusted: the engine bake-off that gates v0, layout comparisons, competitor baselines, Datalog, vectors and mobile.

- cites:: [[pre-design]] {intent: usesDataFrom}
- cites:: [[19-verification#CI gates]] {intent: usesMethodIn}

Every benchmark section names its workload, its metrics and its pass line. A pass line with no known number says "to be measured"; the first run sets the baseline and a later decision sets the target. All runs record hardware, OS, file system, build flags and the graphlava commit.

Common setup: release build with LTO; the database file on local NVMe on desktop; caches dropped between cold runs; each latency metric reported as p50, p99 and p99.9 over at least 10^5 operations after warm-up.

The agent-memory mix used below is: 60% point lookups (node properties, statement by ID), 15% 2–3 hop traversals, 10% star patterns (4–6 predicates), 10% small write transactions of 1–20 statements of which 70% are value overwrites, 5% annotation writes (statements about statements). Half of the traversals and star patterns run inside a `GRAPH` scope of one agent or one episode, so the scope plans of [[09-graphs#Scope expansion]] are measured, and a quarter of the writes insert an existing fact into a second episode ([[09-graphs#Membership operations]]). The dataset generator, seeds and mix are versioned in `spikes/`.

## Engine bake-off

Gates v0. redb is the chosen engine ([[adr:0002]]); libmdbx and Sanakirja run the same harness as reference points behind a minimal `Storage` trait, with the real key layout.

- cites:: [[05-storage-trait#Trait API]] {intent: usesMethodIn}
- cites:: [[04-physical-layout#Encoding v1]] {intent: usesMethodIn}

Layout under test: SPO, POS, OSP with `(a,b,c) -> id`, the `stmt` record table and the `props` table ([[04-physical-layout#Tables]]). Sizes: 10M and 100M statements. Workload: the agent-memory mix at `relaxed` and `sync` durability.

<metric>

Latency p50/p99/p99.9 per operation class (µs); throughput (operations/s, single writer plus 1, 4 and 16 reader threads); file size after load and after 10^6 overwrites (bytes per statement); write amplification (bytes written to disk per logical byte changed, from OS I/O counters); bulk-load time.

</metric>

<target>

To be measured. Reference only, not a target: redb's README reports 1.18M random reads/s, 522K range reads/s and 79.2K small batch writes/s on a Ryzen 9950X3D with NVMe (cited in [[pre-design]]). The bake-off fails redb only if another engine is more than 2x better on p99 of the mix or on file size; that threshold is a proposal for Volodia to confirm.

</target>

## Inline literal range scan

Checks that order-preserving inline literals make numeric and date filters key-range scans ([[adr:0011]]).

- cites:: [[02-id-encoding#Inline literals]] {intent: usesMethodIn}

Workload: 10M nodes with an integer, a dateTime and a double property; filters `?v > c` at selectivities 0.01%, 1% and 10%, with and without a value index; doubles split into the inline subset and dictionary doubles.

<metric>

Latency p50/p99 per selectivity (µs); rows scanned per row returned; share of dictionary decodes.

</metric>

<target>

Rows scanned per row returned at most 1.1 for inline integers and dates with a value index; absolute latencies to be measured.

</target>

## Ordering count comparison

Compares three always-on orderings plus on-demand ones against all six always on ([[adr:0015]]).

- cites:: [[14-planner-execution#On-demand orderings]] {intent: usesMethodIn}

Workload: the agent-memory mix plus a cyclic set (triangles, 4-cycles on one and on several predicates) at 10M and 100M statements, in three layouts: SPO/POS/OSP only; SPO/POS/OSP with on-demand builds enabled; all six.

<metric>

Write latency p50/p99 for small transactions; file size; cyclic query latency p50/p99; time and file growth to build one on-demand ordering.

</metric>

<target>

To be measured. The decision in [[adr:0015]] holds if the three-ordering layout writes at least 30% faster and is at least 30% smaller, while cyclic queries after on-demand builds are within 1.5x of all six; these lines are proposals.

</target>

## Versus tiramemsu

Same dataset and workload on graphlava and tiramemsu, at 1, 10 and 100 versions per fact.

Expectation from the handover: close at 1 version, graphlava ahead as history grows. If graphlava is not ahead at 10 and 100 versions, the bottleneck is the query layer, not storage.

<metric>

Latency p50/p99 of the agent-memory mix; file size; write throughput; per version count.

</metric>

<target>

To be measured.

</target>

## Versus SQLite and Turso

Baseline to beat for agent memory: a triple table with the same three orderings as indexes in SQLite (WAL mode) and in Turso.

<metric>

Latency p50/p99 of the agent-memory mix; write throughput at each durability level mapped to the closest SQLite setting; file size.

</metric>

<target>

To be measured. Proposal: graphlava at least 3x lower p99 on point lookups and overwrites than SQLite, in line with the 3–6x KV gap the handover cites from redb's README.

</target>

## Datalog versus Nemo

Materialization speed against Nemo on standard rule sets.

- cites:: [[13-datalog#On-demand evaluation]] {intent: usesMethodIn}

Rule sets: transitive closure over chains, trees and random graphs (10^5–10^7 edges); RDFS on LUBM-style data. graphlava runs `@on_demand` in v0 and `@materialized` in v1.

<metric>

Time to full materialization; peak memory; time for one goal query with magic sets versus full materialization; for v1, maintenance time per overwrite with counting and Backward/Forward.

</metric>

<target>

To be measured.

</target>

## Mobile

A subset of the agent-memory mix on one iOS and one Android device, at 1M and 10M statements. Runs in v1 with the mobile bindings.

- cites:: [[07-platforms#iOS]] {intent: usesMethodIn}
- cites:: [[07-platforms#Android]] {intent: usesMethodIn}

<metric>

Latency p50/p99 in µs per operation through each binding (Swift, Kotlin), `ordered` durability, dataset seed 42; file size; energy per 10^4 operations where the OS exposes it; time to suspend and to resume in ms, p50 and p99 over 100 backgroundings with a full open batch. Results are stored as baseline JSON with the device model and OS version. Suspend of an app-group file costs the batch flush plus redb's close, which writes its allocator state with one more flush; a private-container suspend costs the flush only.

</metric>

<target>

Gate: suspend p99 below 500 ms at 10M statements on each reference device; the run fails at 500 ms or more ([[openspec:mobile-bindings#Mobile latency]]). Every other metric is to be measured.

</target>

## Binding overhead

The time a host-language binding adds to each call above the same Rust call, for Python first and then Node.js and mobile ([[adr:0042]]).

- cites:: [[16-api#Python binding]] {intent: usesMethodIn}
- cites:: [[16-api#Node.js binding]] {intent: usesMethodIn}
- cites:: [[17-benchmarks#Durability commit latency]] {intent: usesMethodIn}

Workloads, each run through the binding and through Rust on the same file:

- a `props()` point lookup on a random node;
- an `out()` 2-hop traversal;
- a 1-statement `ordered` submit;
- a 10,000-row `SELECT`, read row by row and in 1024-row batches (`iter_batches()` in Python).

Dataset: `py-agent-v1`, seed 42, 1M statements (4 per node: two `knows` edges, `age`, `name`), the same file for both sides. The Python driver is `crates/graphlava-py/bench/overhead.py`; it runs the Rust side, `cargo run --release -p graphlava-py --example overhead_baseline` (`crates/graphlava-py/bench/rust_baseline.rs`), on the same file and reports the difference. The `sync`, `ordered` and `relaxed` 1-statement commits of [[17-benchmarks#Durability commit latency]] run through both sides too, and their delta is recorded. `bench/scripts/nightly.sh` runs it on the fixed runner.

<metric>

Overhead in µs per call above the Rust call: p50, p99 and p99.9, recorded as HDR histograms; for the batch workload, overhead per row for the row path and the batch path side by side.

</metric>

<target>

To be measured: the first run on the fixed Linux x86_64 nightly runner sets the baseline (`bench/results/binding-overhead-python.baseline.json`); after that the nightly job fails if p50 exceeds the baseline by more than 20% at 95% confidence.

</target>

## Durability commit latency

Commit latency for `sync`, `ordered` and `relaxed` on macOS, Linux, Windows and iOS ([[adr:0012]], [[adr:0025]]).

- cites:: [[06-transactions-durability#Group commit]] {intent: usesMethodIn}

Workload: 1–20 statement write transactions from 1, 8 and 64 concurrent callers; group commit windows of 1, 2 and 5 ms.

<metric>

Acknowledgment latency p50/p99 per level; commits per second; fsync calls per second; bytes written to the file per request at batch sizes 1, 8 and 64, with one engine transaction per request and per batch.

</metric>

<target>

`ordered` achieves at least 5x the commit throughput of `sync` with 8 or more concurrent callers (proposal); absolute numbers to be measured.

</target>

## Dictionary sweep cost

Cost of the background dictionary sweep ([[adr:0016]]).

- cites:: [[03-dictionary#Sweep]] {intent: usesMethodIn}

Workload: 10M and 100M statements, dictionary of 10^6 and 10^7 terms, 10% of them unreferenced after a churn phase.

<metric>

Sweep time per 10^6 terms; writer p99 latency while the sweep runs; bytes reclaimed.

</metric>

<target>

Writer p99 while sweeping within 1.2x of the idle p99 (proposal); sweep time to be measured.

</target>

## Vector search recall and latency

Recall and latency of the vector index in v1 ([[15-vector-index#IVF layout]]).

Workload: 1M and 10M vectors of dimension 384 and 768; `nprobe` from 1 to 64; with and without a scope filter; scopes of 10^2 to 10^6 candidates to place the exact-search threshold ([[15-vector-index#Search modes]]); and a hybrid query that takes the top 10 memories in an agent's scope and expands them two hops.

<metric>

Recall@10 versus exact search, with and without re-ranking; query latency p50/p99 in exact and IVF mode; insert latency; index size per vector; training and list-split time; recall after 10x growth without retraining.

</metric>

<target>

To be measured; the target is an open question ([[open-questions#Vector recall target]]).

</target>

## Harness and method

How every benchmark here is run, so that two numbers from different days or machines can be compared. The rules apply to all sections above and below ([[adr:0020]]).

- cites:: [[19-verification#CI gates]] {intent: usesMethodIn}

Layout: micro benchmarks in `crates/<crate>/benches/` (criterion for wall-clock, `iai-callgrind` for instruction counts); the macro harness in `bench/` as a binary `meta-bench` with a dataset generator, workload drivers and adapters for graphlava, SQLite, Oxigraph and tiramemsu behind one trait. Results are JSON files in `bench/results/<date>-<commit>-<host>.json` holding raw HDR histograms, not just summaries.

<metric>

Per run: hardware (CPU model, cores, RAM), OS and kernel, file system and mount options, storage device, Rust version and flags, dataset generator version and seed, graphlava commit, durability level, cache state (cold or warm).

</metric>

<target>

Latency is measured open-loop at a fixed arrival rate, so coordinated omission cannot hide stalls; closed-loop throughput is reported separately. At least 5 runs per configuration; the reported value is the median run with a bootstrap 95% confidence interval. Cold runs drop the page cache (`purge` on macOS, `drop_caches` on Linux).

</target>

## Micro benchmarks

Per-pull-request gate on hot paths, measured in instructions so that the result does not depend on runner noise.

- cites:: [[02-id-encoding#Inline literals]] {intent: usesMethodIn}
- cites:: [[03-dictionary]] {intent: usesMethodIn}

Cases: ID encode and decode per inline subtype; composite key encoding; dictionary intern (hit and miss) and lookup; one insert, update, annotate and cascade delete on a warm in-memory store; promotion; Leapfrog Triejoin intersection of 3 sorted runs of 10^4 IDs; plan cache hit; SPARQL parse and lowering of a 10-pattern query.

<metric>

Instructions, L1 and LL cache misses (Cachegrind) per operation; criterion wall-clock for local runs.

</metric>

<target>

No case may use more than 3% more instructions than on `main`; a larger rise needs a reviewer to accept it in the PR with a reason.

</target>

## Standard SPARQL workloads

Established RDF benchmarks, so graphlava can be compared with published numbers and with Oxigraph, the closest embedded Rust RDF store.

- cites:: [[11-sparql#Conformance]] {intent: usesMethodIn}

Suites: LUBM at 10, 100 and 1,000 universities (14 queries, with and without RDFS rules through Datalog); WatDiv at 10M and 100M triples (linear, star, snowflake and complex templates, 20 instances each); BSBM explore and explore-and-update mixes at 10M triples. Every suite also runs on Oxigraph, same machine and same durability setting.

<metric>

Per query template: latency p50/p99 (ms); per mix: query mixes per hour; load time; file size; for update mixes, commit latency p99.

</metric>

<target>

To be measured. Proposal: within 1.5x of Oxigraph on every WatDiv template and faster on star templates, where inline properties avoid joins.

</target>

## Traversal workload

Short-hop reads and small updates in the style of LDBC SNB Interactive, the closest public workload to agent memory.

- cites:: [[14-planner-execution#Join selection]] {intent: usesMethodIn}

Workload: an SNB-style social graph at scale factors 1 and 10, the short reads IS1–IS7 and the insert operations, expressed in SPARQL and Datalog in v0 and in Cypher in v1, with 1 writer and 1–16 readers.

<metric>

Latency p50/p99/p99.9 per operation; throughput; the share of time in the planner versus the executor.

</metric>

<target>

To be measured; the v1 Cypher run must not be slower than the v0 SPARQL run of the same operation by more than 10%, since both use one IR.

</target>

## Scaling sweep

How latency and size grow with the database, to find cliffs such as the working set leaving the page cache.

- cites:: [[04-physical-layout]] {intent: usesMethodIn}

Workload: the agent-memory mix at 10^5, 10^6, 10^7, 10^8 and 10^9 statements on desktop, and 10^5 to 10^7 on phones; fixed RAM per platform.

<metric>

Point lookup and overwrite latency p50/p99 per size; bytes per statement; the fitted slope of latency against log N.

</metric>

<target>

Point lookups and overwrites grow at most logarithmically while the working set fits in RAM (fitted slope reported); a step change between two sizes is explained in the results report.

</target>

## Reader scaling

Snapshot readers must not slow the single writer, and must scale with cores.

- cites:: [[06-transactions-durability#Transaction model]] {intent: usesMethodIn}

Workload: one writer at a fixed 5,000 small transactions per second at `ordered`; 1, 2, 4, 8, 16, 32 and 64 reader threads running the read part of the mix.

<metric>

Reader throughput per thread count; reader p99; writer p99 and achieved rate; file growth caused by long-lived snapshots.

</metric>

<target>

Writer p99 with 16 readers within 1.3x of writer p99 with none (proposal); reader throughput at least 0.7 x thread count x single-thread throughput up to the physical core count.

</target>

## Open and recovery time

How long an application waits to open a database after a clean close and after a crash, which bounds app start on mobile.

- cites:: [[06-transactions-durability#Crash recovery]] {intent: usesMethodIn}

Workload: files of 10^6, 10^7 and 10^8 statements; open after clean close, and after `kill -9` during a large `relaxed` transaction.

<metric>

Time to first query (ms); pages read during open; memory after open.

</metric>

<target>

Open after a clean close under 50 ms at 10^7 statements on desktop (proposal); recovery time to be measured.

</target>

## Space under churn

Long-run file size under the overwrite-heavy agent workload, since redb does not compact automatically.

- cites:: [[04-physical-layout#Encoding v1]] {intent: usesMethodIn}

Workload: load 10^7 statements, then 10^8 overwrite and annotate operations; measure every 10^7 operations, before and after an explicit compaction.

<metric>

File size and bytes per live statement over time; free-page ratio; compaction time and peak extra disk.

</metric>

<target>

Steady-state size at most 2x the size right after load (proposal), and compaction restores it to within 1.2x.

</target>

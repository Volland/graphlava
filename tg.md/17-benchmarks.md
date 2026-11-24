---
type: Benchmark
status: planned
gate: v0
specs: ["adr:0002", "adr:0005", "adr:0009", "adr:0011", "adr:0012", "adr:0015", "adr:0016", "adr:0013"]
---
# Benchmarks

The measurements that must run before meta-core's performance claims are trusted: the engine bake-off that gates v0, layout comparisons, competitor baselines, Datalog, vectors and mobile.

- cites:: [[pre-design]] {intent: usesDataFrom}

Every benchmark section names its workload, its metrics and its pass line. A pass line with no known number says "to be measured"; the first run sets the baseline and a later decision sets the target. All runs record hardware, OS, file system, build flags and the meta-core commit.

Common setup: release build with LTO; the database file on local NVMe on desktop; caches dropped between cold runs; each latency metric reported as p50, p99 and p99.9 over at least 10^5 operations after warm-up.

The agent-memory mix used below is: 60% point lookups (node properties, statement by ID), 15% 2–3 hop traversals, 10% star patterns (4–6 predicates), 10% small write transactions of 1–20 statements of which 70% are value overwrites, 5% annotation writes (statements about statements). The dataset generator, seeds and mix are versioned in `spikes/`.

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

Same dataset and workload on meta-core and tiramemsu, at 1, 10 and 100 versions per fact.

Expectation from the handover: close at 1 version, meta-core ahead as history grows. If meta-core is not ahead at 10 and 100 versions, the bottleneck is the query layer, not storage.

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

To be measured. Proposal: meta-core at least 3x lower p99 on point lookups and overwrites than SQLite, in line with the 3–6x KV gap the handover cites from redb's README.

</target>

## Datalog versus Nemo

Materialization speed against Nemo on standard rule sets.

- cites:: [[13-datalog#On-demand evaluation]] {intent: usesMethodIn}

Rule sets: transitive closure over chains, trees and random graphs (10^5–10^7 edges); RDFS on LUBM-style data. meta-core runs `@on_demand` in v0 and `@materialized` in v1.

<metric>

Time to full materialization; peak memory; time for one goal query with magic sets versus full materialization; for v1, maintenance time per overwrite with counting and DRed.

</metric>

<target>

To be measured.

</target>

## Mobile

A subset of the agent-memory mix on one iOS and one Android device, at 1M and 10M statements. Runs in v1 with the mobile bindings.

- cites:: [[07-platforms#iOS]] {intent: usesMethodIn}
- cites:: [[07-platforms#Android]] {intent: usesMethodIn}

<metric>

Latency p50/p99; file size; energy per 10^4 operations where the OS exposes it; time to suspend and resume.

</metric>

<target>

Suspend completes in under 1 s so the app is never killed for a held lock (iOS gives a few seconds; the exact budget is to be measured). Other targets to be measured.

</target>

## Durability commit latency

Commit latency for `sync`, `ordered` and `relaxed` on macOS, Linux, Windows and iOS ([[adr:0012]]).

- cites:: [[06-transactions-durability#Group commit]] {intent: usesMethodIn}

Workload: 1–20 statement write transactions from 1, 8 and 64 concurrent callers; group commit windows of 1, 2 and 5 ms.

<metric>

Acknowledgment latency p50/p99 per level; commits per second; fsync calls per second.

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

Recall and latency of the IVF-PQ index in v1 ([[15-vector-index#IVF-PQ layout]]).

Workload: 1M and 10M vectors of dimension 384 and 768; `nprobe` from 1 to 64; with and without a scope filter.

<metric>

Recall@10 versus exact search; query latency p50/p99; insert latency; index size per vector; training time.

</metric>

<target>

To be measured; the target is an open question ([[open-questions#Vector recall target]]).

</target>

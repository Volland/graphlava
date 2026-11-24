---
type: Requirement
status: draft
priority: should
specs: ["adr:0013", "adr:0002", "adr:0018"]
---
# Vector index

An IVF-PQ vector index stored as B+tree entries in the same file, for similarity search over nodes, statements and hyperedges, usable as an IR operator inside SPARQL and Datalog. Arrives in v1.

- follows_from:: [[adr:0013]]
- depends_on:: [[05-storage-trait#Trait API]] {strength: hard}
- depends_on:: [[10-ir#Operators]] {strength: hard}
- introduced_by:: [[18-roadmap#v1]]
- measured_by:: [[17-benchmarks#Vector search recall and latency]]

## Purpose

Agent memory needs "find memories similar to this one" next to graph queries. The index lives inside the database file so it is transactional, portable and covered by the same durability levels.

<non-goal>

HNSW or any graph-based ANN index. Neighbour-list rewrites touch many pages per insert, which amplifies copy-on-write writes in redb. Exact (brute-force) search over more than one IVF list is also not a goal, except as a test oracle.

</non-goal>

<non-goal>

Computing embeddings. Callers supply vectors; meta-core stores and searches them.

</non-goal>

## IVF-PQ layout

Inverted file with product quantisation (Jégou, Douze and Schmid, IEEE TPAMI 2011). Vectors are assigned to their nearest coarse centroid; each list entry stores a short PQ code of the residual.

- depends_on:: [[04-physical-layout#Tables]] {strength: hard}
- depends_on:: [[02-id-encoding#Bit layout]] {strength: hard}

| Table | Key | Value | Notes |
| --- | --- | --- | --- |
| `vec_meta_<idx>` | constant | dimension, metric, `nlist`, `m`, `nbits`, model version, state | one per index |
| `vec_centroids_<idx>` | centroid number `u32` | `d` x `f32` | coarse quantiser |
| `vec_codebooks_<idx>` | sub-quantiser number `u16` | `2^nbits` x `(d/m)` x `f32` | PQ codebooks |
| `vec_list_<idx>` | `(centroid u32, id u64)` big-endian | PQ code, `m` bytes when `nbits = 8` | the inverted lists |
| `vec_assign_<idx>` | `id u64` | `centroid u32` | reverse map for update and delete |
| `vec_raw_<idx>` | `id u64` | `d` x `f32` or `f16` | optional, for re-ranking and retraining |

<rule id="vec-target-kinds" strength="must">

An index is declared in the meta-model over one target kind: nodes (IRI or blank node), statements, or hyperedges of one type. The key `id` is the target's ID ([[02-id-encoding#Bit layout]]); one target has at most one vector per index.

</rule>

<rule id="vec-write-cost" strength="must">

Inserting or overwriting a vector touches at most four keys: `vec_assign`, the old and the new `vec_list` entry, and `vec_raw` when raw vectors are kept. No other entry is rewritten.

</rule>

<rule id="vec-cascade" strength="must">

Deleting a target deletes its vector entries in the same transaction, as part of the cascade ([[06-transactions-durability#Cascade delete]]).

</rule>

Default parameters: `nlist ≈ 4·sqrt(N)`, `m = d/8`, `nbits = 8`; the right values for agent-memory embeddings are to be measured. Size per vector is `m` bytes plus a 12-byte key, about 108 bytes for `d = 768`, `m = 96`, before redb overhead (to be measured).

## Training

How centroids and codebooks are produced and replaced. Training runs outside the writer transaction; switching to new parameters is one commit.

- depends_on:: [[06-transactions-durability#Durability levels]] {strength: soft}

<rule id="vec-untrained-state" strength="must">

Before the index has enough vectors to train (default 32·`nlist`, minimum 1,000), it is in state `flat`: vectors are stored in `vec_raw` only and search is exact. Training moves it to state `trained`.

</rule>

<rule id="vec-train-offline" strength="must">

Training (k-means for centroids, per-subspace k-means for codebooks) reads a sample of `vec_raw` in a read snapshot and builds the new tables under a new model version. A single write transaction then re-encodes changed entries in bounded batches and flips `vec_meta.model_version`. Readers see either the old or the new model, never a mix.

</rule>

<rule id="vec-retrain-trigger" strength="should">

Retraining is triggered when the index has grown by a configured factor since the last training (default 4x), when mean distance to the assigned centroid drifts by more than a configured fraction, or on request.

</rule>

Training cost is O(sample · nlist · d · iterations); wall time at 1M vectors of `d = 768` is to be measured. On mobile, training runs only in the foreground or on charge.

## IR operator

The vector search operator in the shared IR, so a query can mix similarity with graph patterns and Datalog rules.

- depends_on:: [[10-ir#Operators]] {strength: hard}
- depends_on:: [[13-datalog#On-demand evaluation]] {strength: soft}
- depends_on:: [[11-sparql#Datalog service]] {strength: soft}

<rule id="vec-operator-signature" strength="must">

`VectorSearch(index, query_vector, k, nprobe) -> (id, distance)` returns at most `k` rows ordered by distance. `nprobe` (default 8) is the number of inverted lists scanned. `query_vector` may be a constant or a bound variable, so the operator can run once per input row.

</rule>

<rule id="vec-operator-datalog" strength="must">

The operator is a built-in predicate in Datalog (`vec_near(index, ?target, query, k, ?dist)`) usable in rule bodies, including recursive ones. It is treated as an extensional relation for stratification and is never materialized.

</rule>

<rule id="vec-operator-filter" strength="should">

When a graph pattern restricts candidates (for example a scope or a type), the planner pushes the candidate set into the operator as a bitmap filter applied during the list scan, and increases `nprobe` when too few candidates pass.

</rule>

<example>

<input>

`SELECT ?m WHERE { GRAPH :agent1 { ?m a :Memory } SERVICE <meta:vector> { ?m meta:near (:q 10) } }`

</input>

<output>

Scope `:agent1` (transitive) becomes a bitmap of statement and node IDs; the vector operator scans 8 lists, keeps candidates in the bitmap, and returns the 10 nearest `?m`.

</output>

</example>

## Failure behaviour

All vector tables are ordinary redb tables, so writes are atomic with the rest of the transaction.

<rule id="vec-train-crash" strength="must">

A crash during training leaves the old model active; partial new-version tables are deleted on open.

</rule>

## Platform notes

Distance kernels use the same SIMD dispatch as the executor ([[14-planner-execution#Vectorized execution]]): AVX2 on x86-64, NEON on arm64, scalar fallback. `f32` is stored little-endian ([[adr:0018]]). On iOS and Android, raw vectors may be kept as `f16` to halve size.

## Test plan

Recall and correctness tests, with exact search as the oracle.

- Recall@10 versus exact search on a public set (for example a 1M sample of a text-embedding set) for a grid of `nlist`, `m`, `nprobe`.
- Transaction test: overwrite a vector and read it in the same and a concurrent snapshot.
- Retrain test: queries during retraining return results from exactly one model version.
- Benchmark: [[17-benchmarks#Vector search recall and latency]].

## Open items

Questions this spec leaves open; each is an `OpenQuestion` in [[open-questions]].

- [[open-questions#Vector SPARQL syntax]]: how SPARQL calls the vector operator (`SERVICE <meta:vector>`, a function, or Datalog only).
- [[open-questions#Raw vector retention]]: whether raw vectors are kept by default, which doubles size but enables re-ranking and retraining.
- [[open-questions#Vector recall target]]: the recall@10 and p99 latency targets that define done for v1.

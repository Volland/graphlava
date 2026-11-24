---
type: Requirement
status: draft
priority: should
specs: ["adr:0013", "adr:0002", "adr:0018", "adr:0030"]
---
# Vector index

A scope-aware vector index in the same file: exact search over small candidate sets, IVF lists with RaBitQ codes over large ones, for nodes, statements and hyperedges, as an IR operator in SPARQL and Datalog. Arrives in v1.

- follows_from:: [[adr:0013]]
- follows_from:: [[adr:0030]]
- depends_on:: [[05-storage-trait#Trait API]] {strength: hard}
- depends_on:: [[10-ir#Operators]] {strength: hard}
- introduced_by:: [[18-roadmap#v1]]
- measured_by:: [[17-benchmarks#Vector search recall and latency]]

## Purpose

Agent memory needs "find memories similar to this one" next to graph queries, almost always inside one agent's or one episode's scope. The index lives in the database file, so it is transactional, portable and covered by the same durability levels.

<non-goal>

HNSW or any graph-based ANN index. Neighbour-list rewrites touch many pages per insert, which amplifies copy-on-write writes in redb.

</non-goal>

<non-goal>

Computing embeddings. Callers supply vectors; graphlava stores and searches them.

</non-goal>

## Search modes

Each search runs in one of two modes, chosen per query by the planner from the size of the candidate set ([[adr:0030]]). Small scopes are searched exactly; large ones through the IVF lists.

- depends_on:: [[09-graphs#Scope expansion]] {strength: hard}
- depends_on:: [[14-planner-execution#Statistics]] {strength: soft}

<rule id="vec-mode-choice" strength="must">

When the query restricts candidates (a graph scope, a hyperedge type or a bound set of IDs), the planner estimates their number c. If c · d is below a threshold (to be measured; starting point 2 × 10^6), the search is exact: it reads the raw vectors of the candidates and computes every distance. Otherwise it is an IVF search with the candidates as a filter. Without a restriction the search is an IVF search.

</rule>

<rule id="vec-exact-locality" strength="should">

Vectors of statements and hyperedges are keyed by their IDs, which are chunked per home graph ([[09-graphs#Chunk membership]]). An exact search over a graph scope therefore reads `vec_raw` as a few contiguous ID ranges. Indexing a memory as a statement or hyperedge rather than as an IRI node is recommended for scoped search.

</rule>

## IVF layout

An inverted file: each vector is assigned to its nearest coarse centroid, and its list entry holds a RaBitQ code of the residual with an error bound (Gao and Long, SIGMOD 2024).

- depends_on:: [[04-physical-layout#Tables]] {strength: hard}
- depends_on:: [[02-id-encoding#Bit layout]] {strength: hard}

| Table | Key | Value | Notes |
| --- | --- | --- | --- |
| `vec_meta_<idx>` | constant | dimension, metric, `nlist`, rotation seed, code bits, model version, state | one per index |
| `vec_centroids_<idx>` | `(version u32, centroid u32)` | `d` x `f32` | coarse quantiser, per model version |
| `vec_list_<idx>` | `(version u32, centroid u32, id u64)` big-endian | RaBitQ code (`d` bits, rounded up to bytes) plus two `f32` correction factors | the inverted lists |
| `vec_assign_<idx>` | `id u64` | `(version u32, centroid u32)` | reverse map for update and delete |
| `vec_raw_<idx>` | `id u64` | `d` x `f16` (default) or `f32` | always kept; exact search, re-ranking and retraining |

<rule id="vec-target-kinds" strength="must">

An index is declared in the meta-model over one target kind: nodes (IRI or blank node), statements, or hyperedges of one type. The key `id` is the target's ID ([[02-id-encoding#Bit layout]]); one target has at most one vector per index.

</rule>

<rule id="vec-write-cost" strength="must">

Inserting or overwriting a vector touches at most four keys: `vec_raw`, `vec_assign`, and the old and the new `vec_list` entry. While a retraining is in progress it also writes the entries of the new model version ([[15-vector-index#Training and rebalancing]]).

</rule>

<rule id="vec-cascade" strength="must">

Deleting a target deletes its vector entries in the same transaction, as part of the cascade ([[06-transactions-durability#Cascade delete]]).

</rule>

<rule id="vec-rerank" strength="must">

An IVF search scans `nprobe` lists, estimates each distance from the RaBitQ code with its error bound, and re-ranks with the raw vector every candidate whose lower bound is below the current k-th estimated distance. The result is ordered by exact distance.

</rule>

Default parameters: `nlist ≈ 4·sqrt(N)` and one bit per dimension; the right values for agent-memory embeddings are to be measured. Size per vector is about `d/8 + 8` bytes of code plus a 16-byte key in the list, and `2·d` bytes raw in `f16`: about 1.6 KB for `d = 768`, before redb overhead (to be measured).

## Training and rebalancing

How centroids are produced, how lists stay balanced as data drifts, and how a new model replaces the old one without readers seeing a mix.

- depends_on:: [[06-transactions-durability#Durability levels]] {strength: soft}

<rule id="vec-untrained-state" strength="must">

Before the index has enough vectors to train (default 32·`nlist`, minimum 1,000), it is in state `flat`: vectors are stored in `vec_raw` only and every search is exact. Training moves it to state `trained`.

</rule>

<rule id="vec-split" strength="must">

A list that grows beyond a bound (default 4 × the mean list size) is split in a background write transaction: its vectors are clustered into two new centroids, re-encoded, and the vectors near the boundary of neighbouring lists are reassigned (SPFresh, Xu et al., SOSP 2023). A list that shrinks below a quarter of the mean is merged into its nearest neighbour. Splits and merges keep the model version.

</rule>

<rule id="vec-retrain" strength="must">

A full retraining runs only on request or when an embedding model changes. It reads a sample of `vec_raw` in a read snapshot and writes centroids and lists under a new model version in bounded background batches. From the first batch until the switch, the writer applies every vector write to both versions. One commit then flips `vec_meta.model_version`; readers see either the old or the new version, never a mix. The old version is deleted afterwards in background batches.

</rule>

Training cost is O(sample · nlist · d · iterations); wall time at 1M vectors of `d = 768` is to be measured. On mobile, training and rebalancing run only in the foreground or on charge.

## IR operator

The vector search operator in the shared IR, so a query can mix similarity with graph patterns and Datalog rules.

- depends_on:: [[10-ir#Operators]] {strength: hard}
- depends_on:: [[13-datalog#On-demand evaluation]] {strength: soft}
- depends_on:: [[11-sparql#Datalog service]] {strength: soft}

<rule id="vec-operator-signature" strength="must">

`VectorSearch(index, query_vector, k, nprobe, candidates) -> (id, distance)` returns at most `k` rows ordered by exact distance. `nprobe` (default 8) applies to IVF mode. `candidates` is an optional ID filter. `query_vector` may be a constant or a bound variable, so the operator can run once per input row.

</rule>

<rule id="vec-operator-datalog" strength="must">

The operator is a built-in predicate in Datalog (`vec_near(index, ?target, query, k, ?dist)`) usable in rule bodies, including recursive ones. It is treated as an extensional relation for stratification and is never materialized.

</rule>

<rule id="vec-operator-filter" strength="must">

When a graph pattern restricts candidates (for example a scope or a type), the planner pushes the candidate set into the operator. In exact mode it is the set that is scanned; in IVF mode it filters list entries before distance estimation, and `nprobe` grows until k candidates pass or every list is scanned.

</rule>

<example>

<input>

`SELECT ?m WHERE { GRAPH :agent1 { ?m a :Memory } SERVICE <meta:vector> { ?m meta:near (:q 10) } }` with 5,000 memories of `d = 768` in the scope of `:agent1` and 10^7 in the index.

</input>

<output>

c · d = 3.84 × 10^6 is above the starting threshold, so this runs in IVF mode with the scope bitmap as filter; with 2,000 memories in scope (1.5 × 10^6) it runs exactly, reading 2,000 raw vectors from a few contiguous `vec_raw` ranges and returning the exact 10 nearest.

</output>

</example>

## Failure behaviour

All vector tables are ordinary redb tables, so writes are atomic with the rest of the transaction.

<rule id="vec-train-crash" strength="must">

A crash during training, retraining or a split leaves the active model version intact; partial new-version entries are deleted on open, and dual writes stop until a retraining is requested again.

</rule>

## Platform notes

Distance kernels and RaBitQ estimation use the executor's SIMD dispatch ([[14-planner-execution#Vectorized execution]]): AVX2 on x86-64, NEON on arm64, scalar fallback.

`f32` and `f16` are stored little-endian ([[adr:0018]]). On mobile the raw vectors may be stored as `int8` with a per-vector scale to cut size by half again.

## Test plan

Recall and correctness tests, with exact search as the oracle.

- Recall@10 versus exact search on a public set (for example a 1M sample of a text-embedding set) for a grid of `nlist` and `nprobe`, with and without re-ranking.
- Mode choice: the same query over a 500-vector scope and a 10^6-vector scope runs exactly and through IVF respectively, and the small-scope result equals brute force.
- Transaction test: overwrite a vector and read it in the same and a concurrent snapshot.
- Rebalancing: skewed inserts trigger splits; recall after 10x growth without retraining stays within a margin of a freshly trained index (to be measured).
- Retrain test: queries during retraining return results from exactly one model version, and writes during it appear in the new version.
- Benchmark: [[17-benchmarks#Vector search recall and latency]].

## Open items

Questions this spec leaves open; each is an `OpenQuestion` in [[open-questions]].

- [[open-questions#Vector SPARQL syntax]]: how SPARQL calls the vector operator (`SERVICE <meta:vector>`, a function, or Datalog only).
- [[open-questions#Vector recall target]]: the recall@10 and p99 latency targets that define done for v1.
- [[open-questions#Exact search threshold]]: the `c · d` crossover between exact and IVF mode.

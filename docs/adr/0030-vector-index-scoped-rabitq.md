---
status: accepted
date: 2026-11-24
decision-makers: [Volodia]
---
# Vector search is scope-aware with exact search for small scopes, RaBitQ codes and incremental list splits

## Context and Problem Statement

The v1 vector index ([ADR-0013](0013-build-order-sparql-datalog-first.md)) is IVF-PQ with exact search as a non-goal. Agent queries are almost always scoped to an agent or episode; filtering IVF lists by a small scope collapses recall. PQ needs per-subspace codebook training, gives no error bound for re-ranking, and global retraining at 4x growth reacts late to topic drift.

## Considered Options

<option id="scoped-rabitq-spfresh" outcome="chosen">

- The planner chooses per query between exact search over the candidate set (scope bitmap or type range) and IVF search, by estimated candidates × dimension against a threshold (to be measured; starting point 2 × 10^6 dimension-candidates).
- Codes are RaBitQ (Gao and Long, SIGMOD 2024) on the residual to the list centroid, with raw vectors kept (`f16` by default) for re-ranking the candidates whose error bound overlaps the k-th distance.
- Lists split and reassign locally when they exceed a size bound (SPFresh, Xu et al., SOSP 2023); global retraining is on request only.
- Retraining writes new-version tables while the writer dual-writes both versions, then flips the version in one commit.

</option>

<option id="ivf-pq" outcome="rejected">

IVF-PQ with bitmap post-filtering and global retraining (previous spec): poor recall for small scopes, codebook training, late reaction to drift.

</option>

<option id="hnsw" outcome="rejected">

HNSW: neighbour-list rewrites amplify copy-on-write writes (unchanged reason).

</option>

## Decision Outcome

Chosen option: "scoped-rabitq-spfresh", because small scopes get exact answers at scan cost, large ones get bounded-error codes with targeted re-ranking, and the index adapts to drift without full rebuilds.

### Consequences

<consequence kind="good">

Exact results for typical agent scopes; no codebook training; recall degrades predictably and is repaired by re-ranking.

</consequence>

<consequence kind="bad">

Raw vectors are kept by default (about 2·d bytes per vector in `f16`). The planner needs scope-size estimates for the vector operator. Local splits add a maintenance job.

</consequence>

## More Information

Refines the vector index scope in [ADR-0013](0013-build-order-sparql-datalog-first.md). Resolves open question "Raw vector retention".

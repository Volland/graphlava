---
type: Requirement
openspec: [term-identity]
status: draft
priority: must
specs: ["adr:0011", "adr:0016", "adr:0018"]
---
# Dictionary

The mapping between terms that do not fit in an ID (IRIs, blank nodes, long or typed literals) and their 64-bit IDs: tables, prefix compression, interning, lookup cost and the background sweep.

- follows_from:: [[adr:0011]]
- follows_from:: [[adr:0016]]
- depends_on:: [[02-id-encoding#Canonical forms]] {strength: hard}
- depends_on:: [[05-storage-trait#Trait API]] {strength: hard}

## Purpose

The dictionary makes every term an integer so that indexes and joins never touch strings. Inline literals bypass it entirely ([[02-id-encoding#Inline literals]]); everything else is interned once and decoded only at final projection.

## Layout

Four tables hold the dictionary. Forward lookups go from canonical term bytes to an ID, reverse lookups from an ID to term bytes; IRIs share namespace prefixes through a small namespace table.

| Table | Key | Value | Notes |
| --- | --- | --- | --- |
| `ns_fwd` | namespace IRI bytes | `ns_id: u32` | longest-prefix split, see below |
| `ns_rev` | `ns_id: u32` (BE) | namespace IRI bytes | loaded into memory at open |
| `dict_fwd` | encoded term key | `id: u64` (LE) | one entry per interned term |
| `dict_rev` | `id: u64` (BE) | encoded term record | one entry per interned term |

### Encoded term key

The `dict_fwd` key is the canonical term in a compact form. Short keys are stored in full; long ones are hashed so B-tree keys stay small.

| Byte | Field | Meaning |
| --- | --- | --- |
| 0 | `kind` | `0x01` IRI, `0x02` blank node, `0x03` dictionary literal, `0xFF` hashed |
| 1..4 | `ns_id` (IRI) or datatype ID low 32 bits (literal) | IRI namespace or literal datatype |
| 5..6 | language ID (literal only, `0` if none) | see [[open-questions#Language tag table]] |
| 7.. | local name or lexical form, UTF-8 | canonical form |

<rule id="dict-key-hash" strength="must">

If an encoded key is longer than 256 bytes, the stored key is `0xFF` followed by the 128-bit XXH3 hash of the full encoded key. A lookup that hits a hashed key compares the full term from `dict_rev` before returning, and a true collision is resolved by appending a 1-byte disambiguator to the key.

</rule>

### Encoded term record

The `dict_rev` value is self-contained so that decoding needs at most one extra in-memory lookup (the namespace).

| Field | Size | Meaning |
| --- | --- | --- |
| `kind` | 1 B | as in the key |
| `flags` | 1 B | bit 0: compressed body (v2), others reserved |
| `ns_or_datatype` | 4 B | `ns_id` or datatype |
| `lang` | 2 B | language ID |
| `created_tx` | 8 B | commit counter at interning, for [[03-dictionary#Sweep]] |
| `body` | rest | local name or lexical form |

<rule id="dict-prefix-split" strength="must">

An IRI is split at the last `#`, `/` or `:` that leaves a non-empty local name. The prefix is interned in `ns_fwd` (created on first use), and the local name is stored in the term key and record. Splitting is deterministic so the same IRI always produces the same key.

</rule>

<rule id="dict-literal-canonical" strength="must">

Only canonical literal forms are interned ([[02-id-encoding#Canonical forms]]). A literal that fits inline is never interned.

</rule>

## Interning

Interning returns the ID of a term, creating it if absent, inside the current write transaction. Read transactions only look up and never create.

<rule id="dict-intern-in-txn" strength="must">

A term created by a write transaction exists only if that transaction commits. Its ID comes from the sequential counter for its kind ([[02-id-encoding#Allocation]]); an aborted transaction's IDs are never reused.

</rule>

<rule id="dict-cache" strength="should">

The owner process keeps a bounded in-memory cache of `term → id` and `id → term`, filled on lookup and invalidated by the sweep. The cache never holds entries created by an uncommitted transaction outside that transaction.

</rule>

### Algorithm

Interning is four steps; only the last one writes.

1. Canonicalise the term; if it fits inline, return the inline ID.
2. Split IRIs into namespace and local name; look up or create the `ns_id`.
3. Build the encoded key; look it up in the cache, then in `dict_fwd`.
4. If absent, allocate the next ID of its kind, write `dict_fwd` and `dict_rev`, and record it in the transaction's cache overlay.

| Step | Cost | Notes |
| --- | --- | --- |
| lookup hit (cache) | O(1) | hash map |
| lookup hit (disk) | O(log n) B-tree descent | one `dict_fwd` read |
| insert | O(log n) | two B-tree writes |
| decode | O(log n) | one `dict_rev` read, namespace from memory |

Absolute latencies are to be measured in [[17-benchmarks#Engine bake-off]].

### Bulk load

A bulk load interns terms in sorted batches instead of one at a time.

Terms of a batch are collected, sorted by encoded key, deduplicated, looked up with one ordered scan of `dict_fwd`, and the missing ones are allocated in sorted order and written as sorted runs. This turns random B-tree inserts into sequential ones; the speedup is to be measured.

## Sweep

Unused terms are reclaimed by a background sweep, never by reference counts on the write path. The sweep scans `dict_rev` in ID order between write transactions and deletes terms that nothing references.

- follows_from:: [[adr:0016]]

<rule id="dict-sweep-liveness" strength="must">

A term is live if its ID appears in any of: an ordering (SPO, POS, OSP, any on-demand ordering, any multi table) in any position; a property record or value index; a hyperedge record or incidence entry; `gmem`; a derived-fact table; a vector index; or the meta-model. Each new structure that can hold a term ID must be added to this list before it ships.

</rule>

<rule id="dict-sweep-window" strength="must">

A term whose `created_tx` is younger than the safety window (default: one hour of commits, configurable) is skipped.

</rule>

<rule id="dict-sweep-batches" strength="must">

The sweep works in batches of at most N terms (default 1,024) per write transaction, holds no lock between batches, and records its cursor in `meta` so it resumes after a restart.

</rule>

### Liveness check per term

Each term is checked by point probes, cheapest first.

Probes: as subject in SPO, as object in OSP, as predicate in POS, as value in the value index of its datatype, then the remaining structures. Most dead terms are found dead after all probes; most live terms are found live after the first two. Total cost is O(dictionary size × probes × log n), to be measured.

### Triggers

The sweep starts when the dictionary has grown by a set fraction (default 20 %) since the last completed sweep, or on explicit request. On iOS and Android it runs only in the foreground while charging, or on request ([[07-platforms#iOS]], [[07-platforms#Android]]).

## Failure behaviour

Interning is part of the user's write transaction, so a crash leaves either the term and all its uses, or neither.

A sweep batch is its own transaction: a crash loses at most one batch of progress, and the cursor resumes from the last committed batch.

<rule id="dict-sweep-race" strength="must">

A sweep batch and a user write never run at once (single writer). A term that becomes referenced after the sweep checked it but before the batch commits cannot exist, because the batch's checks and deletes run in one write transaction.

</rule>

## Platform notes

The dictionary is plain table data and has no platform-specific behaviour. On mobile the in-memory cache size is configured lower (default to be measured), and the sweep runs only under the conditions in [[03-dictionary#Sweep]].

## Test plan

These tests cover correctness of the mapping and safety of the sweep.

- Round-trip: random IRIs, blank nodes and literals intern and decode to the same canonical term.
- Hash keys: forced collisions on long keys resolve to distinct IDs.
- Sweep safety: a model-based test that interleaves writes, deletes and sweep batches never deletes a live term.
- Crash: kill during a sweep batch and during interning; no orphan `dict_fwd` without `dict_rev` and vice versa.
- Benchmark: bulk load interning throughput, in [[17-benchmarks#Engine bake-off]].

## Open items

Questions this spec leaves open; each is an `OpenQuestion` in [[open-questions]].

- [[open-questions#Language tag table]]
- [[open-questions#Large literal storage]]

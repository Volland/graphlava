---
type: Requirement
openspec: [term-identity]
status: draft
priority: must
specs: ["adr:0011", "adr:0016", "adr:0023", "adr:0018"]
---
# Dictionary

The mapping between terms that do not fit in an ID (IRIs, blank nodes, long or typed literals) and their 64-bit IDs: tables, prefix compression, interning, lookup cost and the background sweep.

- follows_from:: [[adr:0011]]
- follows_from:: [[adr:0016]]
- follows_from:: [[adr:0023]]
- depends_on:: [[02-id-encoding#Canonical forms]] {strength: hard}
- depends_on:: [[05-storage-trait#Trait API]] {strength: hard}

## Purpose

The dictionary makes every term an integer so that indexes and joins never touch strings. Inline literals bypass it entirely ([[02-id-encoding#Inline literals]]); everything else is interned once and decoded only at final projection.

## Layout

Eight tables hold the dictionary: forward and reverse term lookups, and forward and reverse tables for namespaces, datatypes and language tags.

Forward lookups go from canonical term bytes to an ID, reverse lookups from an ID to term bytes; IRIs share namespace prefixes and literals share datatypes through small name tables.

| Table | Key | Value | Notes |
| --- | --- | --- | --- |
| `ns_fwd` | namespace IRI bytes | `ns_id: u32` | longest-prefix split, see below |
| `ns_rev` | `ns_id: u32` (BE) | namespace IRI bytes | loaded into memory at open; capped, see `dict-ns-cap` |
| `dt_fwd` | datatype IRI bytes | `dt_id: u32` | one entry per literal datatype |
| `dt_rev` | `dt_id: u32` (BE) | datatype IRI bytes | loaded into memory at open |
| `dict_fwd` | encoded term key | `id: u64` (LE) | one entry per interned term |
| `dict_rev` | `id: u64` (BE) | encoded term record | one entry per interned or owned term |
| `lang_fwd` | lower-cased language tag | `lang_id: u16` | append-only, at most 65,535 ([[adr:0033]]) |
| `lang_rev` | `lang_id: u16` (BE) | language tag | |

Blank nodes have no entry: their label is derived from the ID ([[22-implementation-decisions#Blank nodes have no dictionary entry]]).

### Encoded term key

The `dict_fwd` key is the canonical term in a compact form. Short keys are stored in full; long ones are hashed so B-tree keys stay small.

| Byte | Field | Meaning |
| --- | --- | --- |
| 0 | `kind` | `0x01` IRI, `0x02` blank node, `0x03` dictionary literal, `0xFF` hashed |
| 1..4 | `ns_id` (IRI) or `dt_id` (literal) | IRI namespace or literal datatype |
| 5..6 | language ID (literal only, `0` if none) | 16-bit append-only tag table ([[adr:0033]]) |
| 7.. | local name or lexical form, UTF-8 | canonical form |

<rule id="dict-key-hash" strength="must">

If an encoded key is longer than 256 bytes, the stored key is `0xFF` followed by the 128-bit XXH3 hash of the full encoded key. A lookup that hits a hashed key compares the full term from `dict_rev` before returning, and a true collision is resolved by appending a 1-byte disambiguator to the key.

</rule>

### Encoded term record

The `dict_rev` value is self-contained so that decoding needs at most one extra in-memory lookup (the namespace).

| Field | Size | Meaning |
| --- | --- | --- |
| `kind` | 1 B | as in the key |
| `flags` | 1 B | bit 0: compressed body (v2); bit 1: owned literal, a 16-byte content hash follows `created_tx`; others reserved |
| `ns_or_datatype` | 4 B | `ns_id` or `dt_id` |
| `lang` | 2 B | language ID |
| `created_tx` | 8 B | commit counter at interning, for [[03-dictionary#Sweep]] |
| `hash` | 16 B, owned literals only | XXH3-128 of the encoded term key, for equality ([[03-dictionary#Owned literals]]) |
| `body` | rest | local name or lexical form |

<rule id="dict-prefix-split" strength="must">

An IRI is split at the last `#`, `/` or `:` that leaves a non-empty local name. The prefix is interned in `ns_fwd` (created on first use), and the local name is stored in the term key and record. Splitting is deterministic so the same IRI always produces the same key.

</rule>

<rule id="dict-ns-cap" strength="must">

The namespace table holds at most a configured number of entries (default 65,536). When it is full, a new prefix is not interned: the IRI is stored with `ns_id = 0` (the empty namespace) and its whole text as local name. Path-shaped IRIs such as `…/memory/2026/11/24/x` therefore cannot grow the in-memory table without bound. Because the cap is reached once and never undone, splitting stays deterministic for every IRI interned after it.

</rule>

<rule id="dict-dt-table" strength="must">

Literal datatypes are interned in `dt_fwd` and named by a `u32` `dt_id` in keys and records, never by bits of the datatype's IRI ID.

</rule>

<rule id="dict-literal-canonical" strength="must">

Only canonical literal forms are interned ([[02-id-encoding#Canonical forms]]). A literal that fits inline is never interned, and a literal above the owned threshold is owned instead ([[03-dictionary#Owned literals]]).

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

1. Canonicalise the term; if it fits inline, return the inline ID. If it is a literal whose encoded key exceeds the owned threshold, allocate an owned ID, write only `dict_rev`, and return.
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

## Owned literals

A large literal is owned by the one slot that holds it instead of being interned. It is deleted with its slot, so overwriting long text never leaves garbage for the sweep ([[adr:0023]]).

- follows_from:: [[adr:0023]]

<rule id="dict-owned-threshold" strength="must">

A literal whose encoded term key is longer than a threshold (default 256 bytes, fixed per file in `meta`) is owned. It gets a dictionary-literal ID with bit 59 set, a `dict_rev` record with the owned flag and the 128-bit content hash, and no `dict_fwd` entry.

</rule>

<rule id="dict-owned-lifetime" strength="must">

An owned literal is referenced by exactly one property slot or one promoted statement. Overwriting or deleting that slot deletes its `dict_rev` record in the same transaction. Writing the same text into another slot creates another owned literal.

</rule>

<rule id="dict-owned-equality" strength="must">

Two literals are equal if their IDs are equal, or if either is owned and both have the same content hash and the same bytes. Joins, `=`, `DISTINCT` and the uniqueness check of promoted statements use this rule on any column the planner cannot prove free of owned literals ([[14-planner-execution#Vectorized execution]]).

</rule>

## Sweep

Unused interned terms are reclaimed by a background mark-and-sweep, never by reference counts on the write path. Owned literals are never swept; they die with their slot.

- follows_from:: [[adr:0016]]
- follows_from:: [[adr:0023]]

<rule id="dict-sweep-liveness" strength="must">

A term is live if its ID appears in any of: an ordering (SPO, POS, OSP, any on-demand ordering, any multi table) in any position; a property record or value index; a hyperedge record or incidence entry; a hyperedge type declaration; a graph name (`gname`); the system IRIs recorded in `meta`; `gmem`; a derived-fact table; a vector index; or the meta-model. Each new structure that can hold a term ID must be added to this list, and to the mark pass, before it ships.

</rule>

<rule id="dict-sweep-window" strength="must">

A term whose `created_tx` is younger than the safety window (default: one hour of commits, configurable) is skipped.

</rule>

<rule id="dict-sweep-batches" strength="must">

The sweep phase deletes in batches of at most N terms (default 1,024) per write transaction, holds no lock between batches, and records its cursor in `meta` so it resumes after a restart. A restart after a crash repeats the mark pass.

</rule>

### Mark and sweep

Liveness is decided for all terms at once by one sequential pass over a read snapshot, not by per-term probes: a dictionary literal lives inside property records, which no point probe can reach.

1. Mark: open a read snapshot at commit C. Scan every structure of `dict-sweep-liveness` sequentially and set the bit of each sequential ID found (IRI, blank node, interned dictionary literal) in a roaring bitmap per kind. From this moment the owner process records in a kept set every sequential ID that the intern path returns to a writer.
2. Sweep: scan `dict_rev` in ID order. Delete each interned term whose bit is clear, that is not in the kept set, and whose `created_tx` is older than the safety window, in batches of `dict-sweep-batches`.
3. Clear the kept set when the sweep completes.

Cost: O(total data) sequential reads for the mark and O(dictionary) for the sweep, plus the bitmap in memory (to be measured, [[17-benchmarks#Dictionary sweep cost]]).

### Triggers

The sweep starts when the dictionary has grown by a set fraction (default 20 %) since the last completed sweep, or on explicit request. On iOS and Android it runs only in the foreground while charging, or on request ([[07-platforms#iOS]], [[07-platforms#Android]]).

## Failure behaviour

Interning is part of the user's write transaction, so a crash leaves either the term and all its uses, or neither.

A sweep batch is its own transaction: a crash loses at most one batch of progress, and the cursor resumes from the last committed batch.

<rule id="dict-sweep-race" strength="must">

A sweep batch and a user write never run at once (single writer). A term referenced by a write committed after the mark snapshot is either in the mark (it was already referenced at C) or in the kept set (the writer obtained its ID from the intern path after C), so the sweep never deletes it. A term created after C is younger than the safety window.

</rule>

## Platform notes

The dictionary is plain table data and has no platform-specific behaviour. On mobile the in-memory cache size is configured lower (default to be measured), and the sweep runs only under the conditions in [[03-dictionary#Sweep]].

## Test plan

These tests cover correctness of the mapping and safety of the sweep.

- Round-trip: random IRIs, blank nodes and literals intern and decode to the same canonical term.
- Hash keys: forced collisions on long keys resolve to distinct IDs.
- Sweep safety: a model-based test that interleaves writes, deletes, the mark pass and sweep batches never deletes a live term, including a term first re-referenced between the mark snapshot and its sweep batch.
- Owned literals: overwrite, delete, promote and cascade a property holding an owned literal; its `dict_rev` record goes with it, and equal texts in two slots compare equal.
- Crash: kill during a sweep batch and during interning; no orphan `dict_fwd` without `dict_rev` and vice versa.
- Benchmark: bulk load interning throughput, in [[17-benchmarks#Engine bake-off]].

## Open items

No questions remain open; the ones this spec raised are answered in [[resolved-questions]].


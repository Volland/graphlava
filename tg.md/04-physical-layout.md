---
type: Requirement
openspec: [statement-store]
status: draft
priority: must
specs: ["adr:0002", "adr:0004", "adr:0005", "adr:0009", "adr:0010", "adr:0015", "adr:0017", "adr:0018"]
---
# Physical layout

Every table in the database file, its key and value bytes, the key byte order, the v1 encoding on plain redb tables, the v2 block encoding, and size estimates per statement.

- follows_from:: [[adr:0015]]
- follows_from:: [[adr:0004]]
- follows_from:: [[adr:0005]]
- follows_from:: [[adr:0018]]
- depends_on:: [[02-id-encoding#Bit layout]] {strength: hard}
- depends_on:: [[05-storage-trait#Trait API]] {strength: hard}

## Purpose

This spec is the on-disk contract. Everything here is frozen at [[18-roadmap#Format freeze]] and later changes need a format version and a migration ([[adr:0018]]). Logical meaning is in [[01-data-model]].

## Tables

All structures are B+tree tables in one redb file. Fixed-width keys and values are used wherever possible so the engine can store them without per-entry length fields.

### Base tables

These tables exist in every database and are written by ordinary statement writes.

| Table | Key | Value | Written on |
| --- | --- | --- | --- |
| `spo` | `(s, p, o)` 24 B | `id: u64` | node-valued statement, promoted property |
| `pos` | `(p, o, s)` 24 B | `id: u64` | same |
| `osp` | `(o, s, p)` 24 B | `id: u64` | same |
| `stmt` | `id` 8 B | `(s, p, o)` 24 B + `flags: u16` | every statement with an ID |
| `props` | `node_id` 8 B | property record, variable | literal-valued statement |
| `meta` | key string | value bytes | format version, flags, counters, cursors, statistics |

<rule id="pl-base-orderings" strength="must">

`spo`, `pos` and `osp` always exist and always contain exactly the statements with node-valued objects plus promoted properties, each once, with the statement ID as value.

</rule>

<rule id="pl-stmt-record" strength="must">

Every statement that has an ID has one `stmt` record. `stmt` is the only table that maps an ID to its `(s, p, o)`.

</rule>

| `stmt.flags` bit | Meaning |
| --- | --- |
| 0 | promoted from inline property |
| 1 | promoted hyperedge role |
| 2 | promoted graph membership |
| 3 | promoted derived fact |
| 4 | has annotations (hint for cascade; may be stale-true, never stale-false) |
| 5 | multi predicate |
| 6..15 | reserved, must be zero |

### Multi predicate tables

Statements on a **Multi predicate** go to separate tables whose key includes the ID, so several statements with the same `(s, p, o)` can coexist.

| Table | Key | Value |
| --- | --- | --- |
| `spo_m` | `(s, p, o, id)` 32 B | — |
| `pos_m` | `(p, o, s, id)` 32 B | — |
| `osp_m` | `(o, s, p, id)` 32 B | — |

<rule id="pl-multi-separate" strength="must">

A statement is in the base orderings or in the multi tables, never both, decided by its predicate's `multi` flag. A scan whose predicate is unbound reads both and merges by key prefix ([[adr:0004]]).

</rule>

### On-demand tables

These tables exist only after the planner or the meta-model creates them. Their existence is recorded in `meta` and in the file's feature flags.

| Table | Key | Value | Created by |
| --- | --- | --- | --- |
| `pso`, `sop`, `ops` | permutation 24 B | `id: u64` | [[14-planner-execution#On-demand orderings]] |
| `pso_m`, `sop_m`, `ops_m` | permutation + `id` 32 B | — | same, for multi predicates |
| `propidx` | `(pred, value, node)` 24 B | — | value index, [[adr:0005]] |

<rule id="pl-ondemand-online" strength="must">

An on-demand table is built online: it is filled in batches by background write transactions and marked usable in `meta` only after the last batch commits. Until then the planner ignores it and every write keeps it in step.

</rule>

<rule id="pl-propidx-single" strength="should">

All value indexes share one `propidx` table keyed by predicate first, instead of one table per predicate as in the handover. Which predicates are indexed is recorded in `meta`. This keeps the table count bounded; the trade-off is that dropping an index is a range delete, not a table drop.

</rule>

### Literal predicate scans

Literal-valued statements are not in the orderings, so a pattern `?x :age ?v` without a value index on `:age` has no index to drive it. Until a value index exists it scans every property record; this is open ([[open-questions#Literal predicate scan without value index]]).

### Property record

A property record holds all inline properties of one node, sorted by predicate then value. Overwriting a value rewrites this one record.

| Offset | Field | Size | Meaning |
| --- | --- | --- | --- |
| 0 | `version` | 1 B | record format, `1` |
| 1 | `flags` | 1 B | bit 0: spilled to `props_ext` |
| 2 | `count` | 2 B LE | number of entries |
| 4 | entries | 16 B each | `(pred: u64, value: u64)` LE |

<rule id="pl-props-value-kinds" strength="must">

An entry's value is an inline literal ID, a dictionary literal ID, or a statement ID. A statement ID means the property is promoted and the value lives in `stmt` ([[01-data-model#Promotion]]).

</rule>

<rule id="pl-props-spill" strength="should">

A record with more than 256 entries spills: the record keeps the header with the spill flag, and the entries move to `props_ext` keyed `(node, pred, value)` 24 B with no value. The threshold is to be tuned by measurement ([[open-questions#Large property records]]).

</rule>

### Hyperedge tables

Owned by [[08-hyperedges#Record format]]; listed here for the complete table inventory.

| Table | Key | Value |
| --- | --- | --- |
| `hedge` | `e` 8 B | `type: u64`, `n: u16`, `n × (role: u64, node: u64)` |
| `incidence` | `(node, role, type, e)` 32 B | — |

### Graph tables

Owned by [[09-graphs#Chunk membership]] and [[09-graphs#Extra membership]]; listed here for the complete table inventory.

| Table | Key | Value |
| --- | --- | --- |
| `gchunk` | `chunk_no` 8 B | `(graph: u64, kind: u64)` |
| `kind_chunk` | `(kind, chunk_no)` 16 B | — |
| `gmem` | `(g, id)` 16 B | `(flags: u8, ptr: u64)`: extra membership or exclusion from the home graph, plus promoted membership statement ID |
| `gmem_rev` | `(id, g)` 16 B | — |
| `in_star` | `(child, ancestor)` 16 B | `paths: u32` path count for counting maintenance |

### Other tables

The dictionary is specified in [[03-dictionary#Layout]] (`ns_fwd`, `ns_rev`, `dict_fwd`, `dict_rev`). Derived facts of materialized rules live in tables named `drv_<rule>` ([[13-datalog#Materialized maintenance]]). Vector tables are named `vec_<index>` ([[15-vector-index#IVF-PQ layout]]).

### Partitioned graph tables

A **Partitioned graph** has its own copy of the statement tables, named with the suffix `@<graph-id as 16 hex digits>` (for example `spo@5000000000000123`). The layouts are identical; the planner routes scans by scope ([[09-graphs#Partitioned graphs]]).

## Key byte order

Composite keys are stored big-endian per component so that byte-wise comparison equals tuple order on unsigned integers. Values and records are little-endian.

<rule id="pl-key-be" strength="must">

Every component of a composite key is a `u64` (or `u32` where stated) written big-endian, and components are concatenated without separators. redb compares these keys with `memcmp` order.

</rule>

<rule id="pl-value-le" strength="must">

Every multi-byte integer outside a key is little-endian, on every platform ([[adr:0018]]).

</rule>

<example>

<input>

Statement `(s = 0x1000_0000_0000_0005, p = 0x1000_0000_0000_0002, o = 0x8180_0000_0000_002A, the inline integer 42)`, a promoted property, stored in `pos`.

</input>

<output>

Key bytes: `10 00 00 00 00 00 00 02 | 81 80 00 00 00 00 00 2A | 10 00 00 00 00 00 00 05` (p, o, s). A range filter on the integer is a contiguous key interval under prefix `p`. Unpromoted literal values are found the same way in `propidx`, whose key also starts `(pred, value)`.

</output>

</example>

## Encoding v1

v1 stores one B-tree entry per tuple in plain redb tables with fixed-width keys. It needs no custom compression and is the v0 encoding.

<rule id="pl-v1-plain" strength="must">

In v1, every table in [[04-physical-layout#Tables]] is a redb table with fixed-width key and value types as listed. Multimap tables are not used: the base orderings map a unique key to one ID, and the multi tables have empty values.

</rule>

Seeking and iteration use redb range queries directly. Leapfrog Triejoin seeks by key prefix: `seek(prefix || x)` is a range query starting at that key ([[14-planner-execution#Leapfrog Triejoin]]).

## Encoding v2

v2 replaces one entry per tuple by blocks of about 512 sorted tuples, compressed column-wise. It applies to the orderings, multi tables, `incidence`, `propidx` and `stmt`; it ships in v2 ([[18-roadmap#v2]]).

### Block key and value

Each block is one redb entry: the key is the block's first tuple, the value is the compressed block.

| Offset | Field | Size | Meaning |
| --- | --- | --- | --- |
| 0 | `version` | 1 B | block format, `1` |
| 1 | `arity` | 1 B | 3 for `(a,b,c)`, 4 with ID |
| 2 | `count` | 2 B LE | tuples in block, 1..1024 |
| 4 | `col_off` | 4 B × arity | byte offset of each column |
| … | column `a` | var | run-length: `(value, run)` pairs |
| … | column `b` | var | delta from previous `b` within a run of `a`, frame-of-reference bitpacked in groups of 128 |
| … | column `c` | var | same as `b`, within runs of `(a, b)` |
| … | column `id` / value | var | bitpacked deltas from the block's minimum ID |

<rule id="pl-v2-split-merge" strength="must">

A block splits into two when it exceeds 1,024 tuples or 8 KiB encoded; it merges with its right neighbour when both together hold at most 512 tuples. Split and merge happen in the writing transaction.

</rule>

<rule id="pl-v2-seek" strength="must">

To seek to key `k`, the iterator finds the last block whose first tuple is `<= k` (one reverse range query), decodes that block's columns lazily, and binary-searches inside it. Decoded blocks are cached per read transaction.

</rule>

<rule id="pl-v2-update" strength="must">

Inserting or deleting a tuple rewrites exactly one block (two on split, two on merge). Under copy-on-write this replaces the block's pages in the new version.

</rule>

Decoding uses SIMD with runtime dispatch (AVX2 on x86-64, NEON on ARM64), with a scalar fallback; speed is to be measured. A block of about 8 KiB is larger than a 4 KiB page, so redb stores it across pages or in overflow; the effect on write amplification is to be measured ([[open-questions#Page size on redb]]).

### Interaction with overwrites

An overwrite of a node-valued statement touches three blocks (one per ordering) plus the `stmt` block, instead of three B-tree entries.

Each block rewrite is larger than an entry rewrite, so v2 trades write bytes for space; whether that pays off on the agent-memory mix is to be measured ([[17-benchmarks#Engine bake-off]]).

## Size estimates

Estimates are raw byte counts before engine overhead. Every number here is an estimate to be measured; the bake-off reports the real bytes per statement.

### v1, per statement with a node-valued object

Counted bytes are key plus value per table; redb page fill and per-entry overhead are not counted.

| Table | Bytes |
| --- | --- |
| `spo`, `pos`, `osp` | 3 × (24 + 8) = 96 |
| `stmt` | 8 + 26 = 34 |
| total raw | 130 B |

The handover estimate was 224 B with six orderings and the ID in the key; three orderings with the ID as value save about 42 % of raw bytes. On-demand orderings add 32 B each.

### v1, per inline property

One 16-byte entry in the owner's property record; no ordering entries and no `stmt` record unless promoted. A value index adds a 24-byte `propidx` entry.

### v2

With column compression the orderings are expected to shrink 3–5× (handover estimate, to be measured). For 1B node-valued statements that gives roughly 130 GB raw in v1 and 35–60 GB in v2 before engine overhead.

| Scale | v1 raw | v2 raw (3–5×) |
| --- | --- | --- |
| 10M statements | 1.3 GB | 0.3–0.45 GB |
| 100M statements | 13 GB | 3.5–6 GB |
| 1B statements | 130 GB | 35–60 GB |

## Failure behaviour

All tables of one database change in one redb transaction, so a crash leaves them mutually consistent at the last commit.

Background work that spans transactions (on-demand table build, block rebalancing, sweep) records its progress in `meta` and resumes or restarts after a crash.

## Platform notes

The layout is identical on every platform: little-endian values, big-endian keys, no OS-specific fields ([[adr:0018]]). redb's page size and its use of mmap versus read calls are engine details, covered in [[05-storage-trait#redb adapter]] and [[07-platforms]].

## Test plan

These tests check the on-disk contract and the consistency invariants between tables.

- Golden files: a fixed small dataset written by the encoder must match checked-in bytes; any change fails until the format version is bumped.
- Cross-table invariant checker: for every `stmt` record, exactly one entry exists in each base ordering (or multi table), and vice versa; runs after every randomized test.
- v2 block fuzzing: random insert/delete sequences against v1 as oracle; iteration and seek results must match.
- Size benchmark: [[17-benchmarks#Ordering count comparison]].

## Open items

Questions this spec leaves open; each is an `OpenQuestion` in [[open-questions]].

- [[open-questions#Large property records]]
- [[open-questions#Page size on redb]]
- [[open-questions#Value index drop policy]]
- [[open-questions#Literal predicate scan without value index]]

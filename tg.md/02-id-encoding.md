---
type: Requirement
openspec: [term-identity]
status: draft
priority: must
specs: ["adr:0011", "adr:0021", "adr:0023", "adr:0010", "adr:0006", "adr:0018"]
---
# ID encoding

The 64-bit ID that names every term in meta-core: kind tag, chunked and sequential allocation, inline literals with value-preserving order, canonical forms, and how IDs are allocated.

- follows_from:: [[adr:0011]]
- follows_from:: [[adr:0021]]
- follows_from:: [[adr:0010]]
- depends_on:: [[01-data-model#Terms]] {strength: hard}

## Purpose

Every IRI, blank node, statement, graph, hyperedge and literal is one `u64`. Joins, orderings and range scans work on these integers; strings appear only at projection.

The layout is on disk in every index, so it is frozen by [[18-roadmap#Format freeze]] before v0 ships.

## Bit layout

All IDs are little-endian `u64` values on disk and compare as unsigned integers in keys (stored big-endian inside composite keys, see [[04-physical-layout#Key byte order]]). Bit 63 separates inline literals (`1`) from every other kind (`0`).

| Bits | Chunked kinds (statement, graph, hyperedge) | Sequential kinds (IRI, blank node, dictionary literal) | Inline double | Other inline literal |
| --- | --- | --- | --- | --- |
| 63..62 | kind tag, high bits (`01`) | kind tag, high bits (`00`) | `10` | `11` |
| 61..60 | kind tag, low bits | kind tag, low bits | payload, top bits | subtype, high bits |
| 59..58 | chunk number, high bits | sequence number, high bits (bit 59: owned flag of a dictionary literal) | payload | subtype, low bits |
| 57..20 | chunk number (40 bits in all, bits 59..20) | sequence number (60 bits in all, bits 59..0) | payload | payload, high bits |
| 19..0 | offset in chunk (20 bits) | sequence number, low bits | payload (62 bits in all, bits 61..0) | payload, low bits (58 bits in all, bits 57..0) |

<rule id="id-kind-tag" strength="must">

For a non-literal ID the kind tag is bits 63..60, with bit 63 clear. Every ID with bit 63 set (tags `0x8`..`0xF`) is an inline literal, so every inline literal ID is greater than every non-literal ID. Value `0x0` is the null ID and is never allocated.

</rule>

| Tag | Kind | Allocation |
| --- | --- | --- |
| `0x0` | null / unbound | never allocated |
| `0x1` | IRI | sequential, [[03-dictionary]] |
| `0x2` | blank node | sequential, [[03-dictionary]] |
| `0x3` | dictionary literal | sequential, [[03-dictionary]]; bit 59 set marks an owned literal ([[03-dictionary#Owned literals]]) |
| `0x4` | statement | chunked, home graph |
| `0x5` | graph | chunked, from the system graph |
| `0x6` | hyperedge | chunked, home graph and type |
| `0x7` | reserved | system objects (meta-model, rules) |
| `0x8`..`0xB` | inline double (bits 63..62 = `10`) | value lives in the ID |
| `0xC`..`0xF` | other inline literal (bits 63..62 = `11`) | value lives in the ID; unassigned subtypes are an error for a reader |

<rule id="id-chunk-split" strength="must">

For statements, graphs and hyperedges, bits 59..20 are the chunk number and bits 19..0 are the offset inside the chunk. A chunk holds 2^20 = 1,048,576 IDs. The chunk number never changes for an ID, so the home graph of an item is a pure function of its ID and the `gchunk` table.

</rule>

<rule id="id-never-reused" strength="must">

An allocated ID is never reused, even after its item is deleted ([[adr:0006]]). The allocator state is monotonic and is committed in the same transaction as the writes that use it.

</rule>

## Inline literals

Inline literals use a prefix code ([[adr:0021]]): `10` in bits 63..62 is a double with a 62-bit payload; `11` is any other subtype (4 bits, 61..58) with a 58-bit payload. Every encoding preserves value order.

An inline literal ID therefore sorts by value within its run, and a range filter on an indexed position is a key range.

| Class, subtype | Datatype | Payload | Order-preserving transform |
| --- | --- | --- | --- |
| `10` | `xsd:double` in range (see below) | 62 bits: 9-bit rebiased exponent and 52-bit mantissa (61-bit magnitude), with the sign applied by the transform | non-negative: 2^61 + magnitude; negative: 2^61 − 1 − magnitude |
| `11`, `0x0` | `xsd:integer` and the derived integer types in range | signed 58-bit integer | add 2^57 (offset-binary) |
| `11`, `0x1` | `xsd:decimal` with at most 9 fractional digits and absolute value below 1.4 × 10^8 | value × 10^9 as a signed 58-bit integer | add 2^57 |
| `11`, `0x2` | `xsd:boolean` | bit 0 | `false < true` |
| `11`, `0x3` | `xsd:dateTime`, `xsd:dateTimeStamp` | UTC microseconds since 1970-01-01, signed 57 bits, then a has-timezone bit | add 2^56 to the instant, then append the bit |
| `11`, `0x4` | `xsd:date` | days since 1970-01-01 of the written date, signed 57 bits, then a has-timezone bit | add 2^56, then append the bit |
| `11`, `0x5` | `xsd:string` of 0..7 UTF-8 bytes | 7 bytes, zero padded on the right, in payload bits 57..2 | lexicographic |
| `11`, `0x6` | `rdf:langString` of 0..5 bytes | 5 bytes then a 16-bit language ID, in payload bits 57..2 | lexicographic, then language |
| `11`, `0x7`..`0xF` | reserved | | |

<rule id="id-numeric-adjacent" strength="must">

The numeric runs are adjacent in key order: inline doubles (class `10`), then integers (`11`, `0x0`), then decimals (`11`, `0x1`). Each run is ordered by value, but the runs interleave in value, so a numeric filter such as `?v > 30` compiles to three key intervals, one per run, and only a scan of every numeric value is one interval. A total numeric `ORDER BY` merges the three runs. Dictionary numerics are reached through the value index ([[04-physical-layout#Tables#On-demand tables]]).

</rule>

<rule id="id-inline-range" strength="must">

Every integer in [-2^57, 2^57 - 1], every decimal and double in its inline range, every date and dateTime in range (years about -313 to 4253), every string of at most 7 bytes and every `xsd:boolean` is stored inline, never in the dictionary. A value outside the inline range is a dictionary literal and keeps its exact lexical form and datatype.

</rule>

<rule id="id-double-inline" strength="must">

An `xsd:double` is stored inline, exactly, when it is ±0, ±infinity, or a normal value whose unbiased exponent is in [-255, 254] (magnitudes from about 10^-77 to 10^77). Its magnitude field is the exponent plus 256 in 9 bits (0 for zero, 511 for infinity) followed by all 52 mantissa bits. NaN, subnormals and doubles outside the range are dictionary literals; range filters on them are served by the value index ([[adr:0022]]), never by an ordering scan.

</rule>

### Why doubles need a rule

A double has 64 bits but the inline payload has 62, so the 11-bit exponent is narrowed to 9 bits while the mantissa stays whole.

The earlier layout of [[adr:0011]] kept only doubles with eight zero low mantissa bits, which turned most computed doubles (0.1, embedding scores) into dictionary literals. [[adr:0021]] replaces it: only extreme magnitudes, subnormals and NaN leave the inline range.

<example>

<input>

`INSERT DATA { :a :score 0.5 . :b :score 0.1 }` then `SELECT ?x WHERE { ?x :score ?v FILTER(?v > 0.2) }` with no value index on `:score`.

</input>

<output>

Both values are inline doubles. The plan reads the `plist` range of `:score`, then each node's property record, and compares `?v` with `0.2` as unsigned integers, with no decode. Once the lookup repeats, the planner creates a value index on `:score` and the filter becomes a key interval per numeric run.

</output>

</example>

## Canonical forms

Literals are folded to one canonical form before interning, so equal values written differently get one ID. The folding table is part of the file format.

<rule id="id-canonical" strength="must">

Every literal is canonicalised before interning, and the canonical form is the only form stored. `"01"^^xsd:integer` and `"1"^^xsd:integer` are the same ID. `-0.0` and `0.0` are distinct IDs (they are equal but not identical in XSD, and SPARQL `=` handles equality), `NaN` has one fixed bit pattern, `xsd:dateTime` values with a zone are converted to UTC and keep only a has-timezone bit (the original offset is not stored, see [[11-sparql#Deviations]]), and `xsd:string` equals the same string with no datatype IRI. Lexical forms that differ only in leading zeros, plus signs or trailing decimal zeros are folded.

</rule>

<rule id="id-canonical-contract" strength="must">

The canonicalisation table is part of the file format. Changing it needs a format version bump and a migration ([[adr:0018]]).

</rule>

## Allocation

How IDs are handed out: chunked kinds lease 2^20-ID chunks per `(graph, kind)`; sequential kinds use one counter each. All allocator state lives in `meta`.

<rule id="id-alloc-chunk-lease" strength="must">

The allocator keeps one global `next_chunk` counter in `meta`. Leasing a chunk for `(graph, kind)` increments it, writes `gchunk[chunk_no] = (graph_id, kind)` and `kind_chunk[(kind, chunk_no)]`, and returns the chunk to the caller. IDs inside a chunk are handed out sequentially from offset 0. A leased chunk is held by the writer; unused offsets are not returned.

</rule>

| Item | Kind value in `gchunk` | Chunk owner |
| --- | --- | --- |
| statement | `0` | home graph |
| graph | `1` | the system graph |
| hyperedge of type `T` | the ID of `T` | home graph |

<rule id="id-graph-bootstrap" strength="must">

Graph IDs come from `graph` chunks owned by the system graph. The system graph has the fixed ID `0x5000_0000_0000_0000` (chunk 0, offset 0) and is created when the file is created.

</rule>

<rule id="id-sequential-alloc" strength="must">

IRI, blank-node and dictionary-literal IDs come from three counters in `meta`, one per kind, each starting at 1. Owned dictionary literals ([[03-dictionary#Owned literals]]) take the next dictionary-literal number with bit 59 set, so the counter is shared and stays below 2^59.

</rule>

### Capacity

A 40-bit chunk number allows 2^40 chunks of 2^20 IDs, about 1.15 x 10^18 IDs per chunked kind, far above the 1B-statement target.

A 1B-statement database uses about 1,000 chunks if packed. Sparse `(graph, kind)` pairs waste up to one chunk each, so a database with 10^6 small graphs uses about 10^6 chunks, or 10^12 IDs, which is far below the limit. Wasted ID space costs no disk space.

## Failure behaviour

<rule id="id-alloc-atomic" strength="must">

Allocator counters and the `gchunk` entry commit atomically with the transaction that uses them. After a crash the counters equal their last committed value, so no ID that appears in a committed record is ever handed out again.

</rule>

## Platform notes

IDs are plain integers and need no platform handling. The on-disk byte order is little-endian for records and big-endian inside composite keys so that lexicographic key order equals numeric order ([[04-physical-layout#Key byte order]]).

## Test plan

Property, corpus and crash tests that prove the encoding round-trips, preserves order and survives a crash.

- Round-trip property test: for 10^7 random values per subtype, `decode(encode(v)) == v`.
- Order property test: for random pairs `a < b` of the same subtype, `encode(a) < encode(b)` as unsigned integers; for doubles the pairs include ±0, ±infinity and both ends of the exponent range.
- Formal model: `MetaCore.Id` proves the integer round trip and order, the double sign-flip order (`encDbl_strictMono`) and the adjacency of the numeric runs (`double_before_int`, `int_before_decimal`).
- Canonicalisation corpus: a table of lexical forms with the expected single canonical ID, run against the XSD datatype list.
- Crash test: kill the writer after each step of a chunk lease and check that counters and `gchunk` agree after recovery.
- Benchmark: [[17-benchmarks#Inline literal range scan]].

## Open items

Questions this spec leaves open; each is an `OpenQuestion` in [[open-questions]].

- [[open-questions#Language tag table]]: the size and growth policy of the 16-bit language ID table.

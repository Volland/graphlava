---
type: Requirement
openspec: [term-identity]
status: draft
priority: must
specs: ["adr:0011", "adr:0010", "adr:0006", "adr:0018"]
---
# ID encoding

The 64-bit ID that names every term in meta-core: kind tag, chunked and sequential allocation, inline literals with value-preserving order, canonical forms, and how IDs are allocated.

- follows_from:: [[adr:0011]]
- follows_from:: [[adr:0010]]
- depends_on:: [[01-data-model#Terms]] {strength: hard}

## Purpose

Every IRI, blank node, statement, graph, hyperedge and literal is one `u64`. Joins, orderings and range scans work on these integers; strings appear only at projection.

The layout is on disk in every index, so it is frozen by [[18-roadmap#Format freeze]] before v0 ships.

## Bit layout

All IDs are little-endian `u64` values on disk and compare as unsigned integers in keys (stored big-endian inside composite keys, see [[04-physical-layout#Key byte order]]). The top 4 bits are the kind tag.

| Bits | Chunked kinds (statement, graph, hyperedge) | Sequential kinds (IRI, blank node, dictionary literal) | Inline literal |
| --- | --- | --- | --- |
| 63..60 | kind tag | kind tag | `0x8` (inline literal tag) |
| 59..56 | chunk number, high bits | sequence number, high bits | subtype |
| 55..20 | chunk number (40 bits in all, bits 59..20) | sequence number (60 bits in all, bits 59..0) | payload, high bits |
| 19..0 | offset in chunk (20 bits) | sequence number, low bits | payload, low bits (56 bits in all, bits 55..0) |

<rule id="id-kind-tag" strength="must">

The kind tag is bits 63..60. Value `0x0` is the null ID and is never allocated. Kinds are numbered so that every inline literal ID is greater than every non-literal ID.

</rule>

| Tag | Kind | Allocation |
| --- | --- | --- |
| `0x0` | null / unbound | never allocated |
| `0x1` | IRI | sequential, [[03-dictionary]] |
| `0x2` | blank node | sequential, [[03-dictionary]] |
| `0x3` | dictionary literal | sequential, [[03-dictionary]] |
| `0x4` | statement | chunked, home graph |
| `0x5` | graph | chunked, from the system graph |
| `0x6` | hyperedge | chunked, home graph and type |
| `0x7` | reserved | system objects (meta-model, rules) |
| `0x8` | inline literal | value lives in the ID |
| `0x9`..`0xF` | reserved | not allocated; a reader treats them as an error |

<rule id="id-chunk-split" strength="must">

For statements, graphs and hyperedges, bits 59..20 are the chunk number and bits 19..0 are the offset inside the chunk. A chunk holds 2^20 = 1,048,576 IDs. The chunk number never changes for an ID, so the home graph of an item is a pure function of its ID and the `gchunk` table.

</rule>

<rule id="id-never-reused" strength="must">

An allocated ID is never reused, even after its item is deleted ([[adr:0006]]). The allocator state is monotonic and is committed in the same transaction as the writes that use it.

</rule>

## Inline literals

Tag `0x8`, then a 4-bit subtype, then a 56-bit payload. The payload encoding preserves value order, so an inline literal ID sorts by value within its subtype and a range filter on an indexed position is a key range.

| Subtype | Datatype | Payload (56 bits) | Order-preserving transform |
| --- | --- | --- | --- |
| `0x0` | `xsd:boolean` | bit 0 | `false < true` |
| `0x1` | `xsd:integer` and the derived integer types in range | signed 56-bit integer | add 2^55 (offset-binary) |
| `0x2` | `xsd:double` that fits (see below) | 56 bits of the double | sign-flip transform, low 8 mantissa bits zero |
| `0x3` | `xsd:decimal` with scale 0..9 and 15 digits | scale and digits | scale-normalised, sign-flipped |
| `0x4` | `xsd:dateTime`, `xsd:dateTimeStamp` | UTC microseconds since 1970-01-01, signed | add 2^55 |
| `0x5` | `xsd:date` | days since 1970-01-01, signed | add 2^55 |
| `0x6` | `xsd:string` of 0..7 UTF-8 bytes | 7 bytes, zero padded on the right | lexicographic |
| `0x7` | `rdf:langString` of 0..5 bytes | 5 bytes then a 16-bit language ID | lexicographic, then language |
| `0x8`..`0xF` | reserved | | |

<rule id="id-numeric-adjacent" strength="must">

The numeric subtypes (integer `0x1`, double `0x2`, decimal `0x3`) are adjacent. A numeric filter such as `?v > 30` therefore compiles to one contiguous key interval that covers all three, and the result is ordered by subtype first and by value second. A query that needs a total numeric `ORDER BY` merges the three runs.

</rule>

<rule id="id-inline-range" strength="must">

Every integer in [-2^55, 2^55 - 1], every date and dateTime in range, every string of at most 7 bytes and every `xsd:boolean` is stored inline, never in the dictionary. A value outside the inline range is a dictionary literal and keeps its exact lexical form and datatype.

</rule>

<rule id="id-double-inline" strength="should">

An `xsd:double` is stored inline only when its low 8 mantissa bits are zero, which holds for all integers up to 2^45 and for dyadic values such as 0.5 and 0.25. Any other double is a dictionary literal. Range filters on dictionary doubles are served by a value index ([[adr:0005]]), never by an ordering scan.

</rule>

### Why doubles need a rule

A double has 64 bits but the inline payload has 56, so only a subset of doubles can be inline.

[[adr:0011]] speaks of "sign-flipped IEEE doubles"; that holds only for the subset above. Most computed doubles (for example 0.1, or an embedding score) are dictionary literals, and filters on them need a value index.

<example>

<input>

`INSERT DATA { :a :score 0.5 . :b :score 0.1 }` then `SELECT ?x WHERE { ?x :score ?v FILTER(?v > 0.2) }` with no value index on `:score`.

</input>

<output>

`0.5` is inline and `0.1` is a dictionary literal. The filter runs as a scan of `:score` statements with a decode of `?v` for dictionary values; the planner creates a value index on `:score` once the lookup repeats.

</output>

</example>

## Canonical forms

Literals are folded to one canonical form before interning, so equal values written differently get one ID. The folding table is part of the file format.

<rule id="id-canonical" strength="must">

Every literal is canonicalised before interning, and the canonical form is the only form stored. `"01"^^xsd:integer` and `"1"^^xsd:integer` are the same ID. `-0.0` and `0.0` are distinct IDs (they are equal but not identical in XSD, and SPARQL `=` handles equality), `NaN` has one fixed bit pattern, `xsd:dateTime` values with a zone are converted to UTC, and `xsd:string` equals the same string with no datatype IRI. Lexical forms that differ only in leading zeros, plus signs or trailing decimal zeros are folded.

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

IRI, blank-node and dictionary-literal IDs come from three counters in `meta`, one per kind, each starting at 1.

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
- Order property test: for random pairs `a < b` of the same subtype, `encode(a) < encode(b)` as unsigned integers.
- Canonicalisation corpus: a table of lexical forms with the expected single canonical ID, run against the XSD datatype list.
- Crash test: kill the writer after each step of a chunk lease and check that counters and `gchunk` agree after recovery.
- Benchmark: [[17-benchmarks#Inline literal range scan]].

## Open items

Questions this spec leaves open; each is an `OpenQuestion` in [[open-questions]].

- [[open-questions#Inline double precision]]: whether doubles outside the 56-bit subset stay dictionary literals.
- [[open-questions#Language tag table]]: the size and growth policy of the 16-bit language ID table.

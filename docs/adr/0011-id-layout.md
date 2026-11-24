---
status: accepted
date: 2026-11-24
decision-makers: [Volodia]
---
# Use a 64-bit ID layout with order-preserving inline literals

## Context and Problem Statement

One 64-bit space identifies IRIs, blank nodes, statements, graphs, hyperedges and literals. The layout is on disk in every index and cannot change after data exists. Range filters on numbers and dates should be index range scans, not decodes.

## Considered Options

<option id="tag4-chunked" outcome="chosen">

- 4-bit kind tag in the top bits (IRI, blank node, statement, graph, hyperedge, inline literal, dictionary literal; spares reserved).
- Chunked kinds (statement, graph, hyperedge): 40-bit chunk number + 20-bit offset ([ADR-0010](0010-chunks-per-graph-and-kind.md)).
- IRI, blank node, dictionary literal: 60-bit sequential dictionary number.
- Inline literal: 4-bit subtype + 56-bit payload. Encodings preserve value order: offset-binary integers, sign-flipped IEEE doubles, UTC microsecond datetimes, zero-padded lexicographic short strings (up to 7 bytes). Numeric subtypes are adjacent so cross-type numeric comparison is one range.
- Dictionary literals are not order-preserving; range filters on them decode and are planned as filters.
- Literals are canonicalized at write time (`"01"^^xsd:integer` equals `"1"`; fixed forms for `-0.0` and `NaN`).

</option>

<option id="flat-dictionary" outcome="rejected">

Every literal gets a dictionary ID: simple, but every range filter decodes terms.

</option>

<option id="wide-id" outcome="rejected">

128-bit IDs: room for more tags, but every key doubles in size.

</option>

## Decision Outcome

Chosen option: "tag4-chunked", because it keeps IDs at 64 bits, makes common range filters index scans, and carries chunk information in the ID itself.

### Consequences

<consequence kind="good">

Numeric and date filters are range scans on `pos`; chunk number is readable from any statement ID; no dictionary lookup for small literals.

</consequence>

<consequence kind="bad">

Only 56 payload bits for inline values, so large integers and high-precision decimals spill to the dictionary. Only 16 kind slots in total. Canonicalization rules are a permanent compatibility contract. Long-string range filters are slow.

</consequence>

## More Information

Refines handover section 3.1; the full bit tables belong in `docs/02-id-encoding.md`.

Specification note (2026-11-24): an IEEE double has 64 bits and the inline payload has 56, so only doubles whose low 8 mantissa bits are zero are inline; others are dictionary literals. See `tg.md/02-id-encoding.md` rule `id-double-inline` and the open question "Inline double precision".

Amended by [ADR-0021](0021-inline-literal-prefix-code.md) and [ADR-0023](0023-dictionary-mark-sweep-owned-literals.md) (2026-11-24): inline literals use a prefix code (`10` double with 62 payload bits, `11` plus 4-bit subtype and 58 payload bits); dictionary-literal sequence bit 59 marks an owned literal. The specification note above is superseded.

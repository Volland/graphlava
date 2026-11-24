---
status: accepted
date: 2026-11-24
decision-makers: [Volodia]
---
# Value indexes key on an order-preserving sort key; literal predicates get an always-on node list

## Context and Problem Statement

[ADR-0005](0005-inline-properties-routing.md) keys `propidx` by `(pred, value_id, node)`. Dictionary literal IDs are sequential, not ordered by value, so a range filter on dictionary doubles, decimals or long strings cannot be a key range, contrary to the spec. And a pattern `?x :age ?v` without a value index has no index at all and scans every property record.

## Considered Options

<option id="sort-key-plus-node-list" outcome="chosen">

- `propidx` key is `(pred, sort_key: u64, node, value_id)`, 32 bytes. `sort_key` is the inline ID for inline literals; for dictionary literals it is an order-preserving 64-bit key in the same order as inline values of that type (numerics share one numeric order; strings use their first 8 bytes). Equal sort keys are resolved by recheck on decode.
- `plist` key is `(pred, node)`, 16 bytes, always on for every literal predicate. It changes only when a predicate appears on or disappears from a node, never on a value overwrite.

</option>

<option id="value-id-key" outcome="rejected">

Keep `(pred, value_id, node)`: exact for inline values only; ranges on dictionary values silently degrade to scans.

</option>

<option id="auto-index-only" outcome="rejected">

Rely on auto-created value indexes for literal predicate scans: the first queries are full scans, and overwrites pay value-index writes.

</option>

## Decision Outcome

Chosen option: "sort-key-plus-node-list", because ranges become key ranges for every literal kind, and every literal predicate scan is a range scan without adding cost to overwrites.

### Consequences

<consequence kind="good">

`?x :p ?v` is a `plist` range; `FILTER(?v > c)` with a value index is a key range for all numeric and string values. Overwrites of an existing property still touch one tree when no value index exists.

</consequence>

<consequence kind="bad">

`propidx` entries grow from 24 to 32 bytes. The first value of a predicate on a node writes one `plist` entry. String ranges recheck values that share an 8-byte prefix.

</consequence>

## More Information

Amends [ADR-0005](0005-inline-properties-routing.md). Resolves open questions "Literal predicate scan without value index" and "Inline double precision" (with [ADR-0021](0021-inline-literal-prefix-code.md)).

---
status: accepted
date: 2026-11-24
decision-makers: [Volodia]
---
# Reclaim dictionary terms with a background sweep, not reference counts

## Context and Problem Statement

Overwrites and deletes leave unreferenced IRIs and dictionary literals. Reference counting would add up to three read-modify-write operations on `dict_rev` per statement write, inside the single-writer transaction.

## Considered Options

<option id="background-sweep" outcome="chosen">

An incremental sweeper scans `dict_rev` in ID order between write transactions. A term is live if it appears in the SPO/POS/OSP orderings (any position), `props`, `hedge`, `incidence`, `gmem`, or the meta-model. Terms younger than a configurable age (default one hour) are skipped. Term IDs are never reused. It runs when the dictionary has grown by a set fraction, or on request; on mobile it runs only in the foreground and on charge, or on request. Inline literals never enter the dictionary.

</option>

<option id="refcount" outcome="rejected">

Exact and immediate, but adds writes to the hottest tree on every statement change.

</option>

## Decision Outcome

Chosen option: "background-sweep", because the write path stays free of dictionary bookkeeping and unused terms only cost disk space for a while.

### Consequences

<consequence kind="good">

No write amplification from dictionary maintenance; sweep timing is controllable per platform.

</consequence>

<consequence kind="bad">

File size is not minimal between sweeps. A sweep costs O(dictionary size) point probes (to be measured). The liveness check must be kept in step with every new place that can hold a term ID.

</consequence>

## More Information

Resolves handover open question 9. Related: [ADR-0011](0011-id-layout.md) for inline literals.

Amended by [ADR-0023](0023-dictionary-mark-sweep-owned-literals.md) (2026-11-24): liveness is decided by a mark pass over a read snapshot instead of per-term probes, and large literals are owned and deleted synchronously.

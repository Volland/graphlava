---
status: accepted
date: 2026-11-24
decision-makers: [Volodia]
---
# Dictionary reclamation by snapshot mark-and-sweep; large literals are owned, not interned

## Context and Problem Statement

[ADR-0016](0016-dictionary-background-sweep.md) checks liveness per term by point probes. A dictionary literal lives only inside `props` records, which no probe can reach, so proving one dead costs a scan of every property record. Agent memory overwrites long strings (summaries, notes) constantly, so these are most of the garbage. The namespace table and literal datatypes also have unbounded or colliding encodings.

## Considered Options

<option id="mark-sweep-owned" outcome="chosen">

- Mark: one sequential pass over a read snapshot of every term-holding structure sets bits in a roaring bitmap of live sequential IDs. Sweep: delete unmarked terms older than the safety window, in write batches. Terms that the intern path returned to a writer since the mark began are kept (an in-memory set in the owner process).
- Owned literals: a literal whose canonical form exceeds a threshold (default 256 bytes) gets an owned dictionary-literal ID (bit 59 set) with a `dict_rev` record and no `dict_fwd` entry. It belongs to one property slot or promoted statement and is deleted with it, synchronously. Equality compares the 128-bit content hash in the record.
- Datatypes get a `dt` table (`u32` IDs) like namespaces; the namespace table is capped (default 65,536) and IRIs beyond it are stored with the empty namespace.

</option>

<option id="probe-per-term" outcome="rejected">

Keep per-term probes: cost is O(dictionary × property records) for literals.

</option>

<option id="refcount" outcome="rejected">

Reference counts on the write path (rejected in ADR-0016 for write cost).

</option>

## Decision Outcome

Chosen option: "mark-sweep-owned", because the sweep becomes one sequential scan instead of random probes, and the commonest garbage (overwritten long strings) never needs a sweep.

### Consequences

<consequence kind="good">

Sweep cost is O(data) sequential reads, runs on a snapshot without blocking the writer, and the liveness list can no longer silently miss a structure that is scanned in the mark pass. Large text never grows `dict_fwd`.

</consequence>

<consequence kind="bad">

Two equal long strings are two owned literals; joins and `=` on them compare hashes, not IDs, so the planner must know a column may hold owned literals. The mark pass needs a bitmap of up to one bit per sequential ID in memory (to be measured; about 120 MB at 10^9 terms uncompressed, far less with roaring).

</consequence>

## More Information

Amends [ADR-0016](0016-dictionary-background-sweep.md) (liveness method) and [ADR-0011](0011-id-layout.md) (dictionary-literal sequence bit 59). Resolves open question "Large literal storage".

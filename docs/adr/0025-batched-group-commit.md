---
status: accepted
date: 2026-11-24
decision-makers: [Volodia]
---
# Group commit runs a whole batch in one engine write transaction

## Context and Problem Statement

[ADR-0012](0012-durability-levels-group-commit.md) commits each `ordered` request as its own redb transaction with `Durability::None` and closes the batch with one `Immediate` commit. Each redb commit copies the root-to-leaf path of every tree it touches (orderings, `stmt`, `props`, dictionary, `meta`), so a one-statement request writes on the order of 100 KB of pages (estimate, to be measured). Readers also see `None` commits before they are durable, and redb frees pages of `None` commits only after a durable commit.

## Considered Options

<option id="one-engine-txn-per-batch" outcome="chosen">

The writer opens one redb write transaction per batch and runs every request of the batch inside it, in arrival order. Each request first checks everything that can fail (conflicts, shape, cascade limit) by reads, then applies its writes; a request that fails before applying is rejected without affecting the batch. Its writes go through a per-request overlay that is merged into the engine transaction only when the request succeeds. The batch commits once with `Immediate` (or `None` for an all-`relaxed` batch), and then every caller is acknowledged.

</option>

<option id="commit-per-request" outcome="rejected">

One engine commit per request (ADR-0012 as written): simplest, but full path copies per request and visibility before durability.

</option>

<option id="engine-savepoints" outcome="rejected">

Roll back failed requests with redb savepoints: redb does not allow taking a savepoint in a transaction that has already written (to be confirmed in the spike), so it cannot isolate requests inside a batch.

</option>

## Decision Outcome

Chosen option: "one-engine-txn-per-batch", because pages shared by a batch are copied once, and a commit becomes visible to readers exactly when it is durable.

### Consequences

<consequence kind="good">

Write amplification drops by roughly the batch size; `ordered` gets visible-implies-durable; the design no longer depends on redb's `None` commit semantics for `ordered`.

</consequence>

<consequence kind="bad">

A request becomes visible to readers only when its batch commits (up to the window N later). An I/O failure fails the whole batch. The per-request overlay adds a merge on reads inside the writer.

</consequence>

## More Information

Amends [ADR-0012](0012-durability-levels-group-commit.md). Resolves open question "Visibility before durability".

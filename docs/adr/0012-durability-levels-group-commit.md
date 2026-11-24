---
status: accepted
date: 2026-11-24
decision-makers: [Volodia]
---
# Durability levels are defined by guarantee; `ordered` is group commit

## Context and Problem Statement

The handover maps `sync`, `ordered` and `relaxed` to OS primitives (`F_FULLFSYNC`, `F_BARRIERFSYNC`, no flush). redb exposes only per-commit `Immediate` (flushed) or `None` (not flushed) as far as known; to be verified in the bake-off spike. There is no barrier-only mode.

## Considered Options

<option id="guarantee-based" outcome="chosen">

- `sync`: redb `Immediate`; durable when the commit returns.
- `relaxed`: redb `None`; atomic and consistent after a crash but may be lost until the next `sync` commit.
- `ordered`: group commit in the owner process; commits are coalesced for up to N ms or M bytes into one `Immediate` commit and each caller is acknowledged after its batch is durable.

</option>

<option id="drop-ordered" outcome="rejected">

Only `sync` and `relaxed`: simplest, but leaves no cheap durable option for many small transactions.

</option>

<option id="own-barrier" outcome="deferred">

Implement `F_BARRIERFSYNC` semantics through an own pager: needs bypassing redb. Possible later behind the `Storage` trait.

</option>

## Decision Outcome

Chosen option: "guarantee-based", because the guarantees stay stable across engines while the mapping to OS primitives is an implementation detail per engine.

### Consequences

<consequence kind="good">

Many small transactions get durable acknowledgment at far fewer syncs; the public interface does not depend on redb internals.

</consequence>

<consequence kind="bad">

`ordered` adds up to N ms latency and needs a batching scheduler in the owner process. Transactions in one batch must not conflict in meaning (the single writer serializes them, so this holds). The OS-primitive table in the spec is "engine-defined, verified by spike".

</consequence>

## More Information

Refines handover D9 and section 9. Related: [ADR-0002](0002-redb-single-owner-process.md).

Amended by [ADR-0025](0025-batched-group-commit.md) (2026-11-24): a batch runs in one engine write transaction and commits once, so `ordered` commits become visible only when durable.

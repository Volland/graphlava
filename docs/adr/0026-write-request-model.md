---
status: accepted
date: 2026-11-24
decision-makers: [Volodia]
---
# Remote writes are one-shot requests; a typed core API sits below the query languages

## Context and Problem Statement

The API exposes an interactive `WriteTxn` that holds the single writer until `commit`. Over IPC or the server crate, every round trip of one client stalls all writers. And 60% of the agent-memory mix is point lookups, for which parsing, planning and batch setup dominate microsecond latencies.

## Considered Options

<option id="one-shot-plus-core-api" outcome="chosen">

- Interactive `WriteTxn` is in-process only, with a hard time limit (default 100 ms). IPC and server clients submit one-shot write requests: an update text with parameters, a Datalog program, or a batch of core operations.
- Core operations include compare-and-set: `update(id, o, expected)` fails with `Conflict` if the current object is not `expected`, so clients can read from a snapshot and write optimistically.
- A typed core API (`props`, `out`, `in`, `stmt`, `insert`, `set`, `update`, `annotate`, `hedge`, `delete`) runs directly on the data model, without the parser or planner.

</option>

<option id="interactive-everywhere" outcome="rejected">

Interactive transactions over IPC: familiar, but one slow client blocks every writer.

</option>

## Decision Outcome

Chosen option: "one-shot-plus-core-api", because the single writer is only ever held by code running in the owner process, and point operations skip the query stack.

### Consequences

<consequence kind="good">

Bounded writer hold time; optimistic read-modify-write without locks; point lookups at storage speed.

</consequence>

<consequence kind="bad">

Remote clients cannot hold a transaction open across round trips; multi-step remote logic uses CAS or a Datalog/SPARQL update. Two API layers to keep consistent.

</consequence>

## More Information

Refines [ADR-0002](0002-redb-single-owner-process.md) (owner process) and [ADR-0025](0025-batched-group-commit.md).

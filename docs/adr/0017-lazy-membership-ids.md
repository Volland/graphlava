---
status: accepted
date: 2026-11-24
decision-makers: [Volodia]
---
# Graph membership statements get IDs lazily, on annotation

## Context and Problem Statement

A statement's home graph comes from its ID chunk ([ADR-0010](0010-chunks-per-graph-and-kind.md)), and `(stmt, :in, g)` is a virtual triple. It must be decided whether membership triples are stored statements with IDs or only get an ID when annotated.

## Considered Options

<option id="lazy" outcome="chosen">

Membership is virtual with no ID. Annotating it assigns a statement ID in a chunk of the member's home graph, and the chunk or `gmem` entry stores a pointer to it. Deleting the membership deletes the promoted statement and cascades to its annotations ([ADR-0006](0006-cascade-delete-annotations.md)). SPARQL reaches an annotated membership through a reifier on `<< ?s :in ?g >>`.

</option>

<option id="always" outcome="rejected">

A stored statement per membership: uniform, but adds a record for every statement and undoes zero-byte primary membership.

</option>

## Decision Outcome

Chosen option: "lazy", because it keeps primary membership free and mirrors promotion of inline properties ([ADR-0005](0005-inline-properties-routing.md)).

### Consequences

<consequence kind="good">

No per-statement membership cost; same promotion pattern as inline properties.

</consequence>

<consequence kind="bad">

The planner has two paths for `:in` (virtual and promoted). Promotion needs a pointer slot in chunk and `gmem` entries.

</consequence>

## More Information

Resolves handover open question 5.

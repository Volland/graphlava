---
status: accepted
date: 2026-11-24
decision-makers: [Volodia]
---
# Deleting a statement cascades to its annotations; IDs are never reused

## Context and Problem Statement

Statements are terms, so any statement can be the subject of other statements (annotations) or a role in a hyperedge. Deleting a statement must define what happens to those references.

## Considered Options

<option id="cascade" outcome="chosen">

Delete transitively every statement whose subject is the deleted ID, in the same transaction. A hyperedge role that references a deleted statement or node blocks the delete unless the caller asks to remove the whole hyperedge (DETACH-style). `KEEP ANNOTATIONS` leaves orphans reachable by ID only. IDs are never reused. A value update keeps the ID and cascades nothing.

</option>

<option id="dangling" outcome="rejected">

Cheapest delete, but leaves unreachable garbage and ambiguous references.

</option>

<option id="block" outcome="rejected">

Safe, but agent cleanup must delete annotations first, which makes simple deletes error-prone.

</option>

## Decision Outcome

Chosen option: "cascade", because annotations have no meaning without their subject and a single atomic transaction keeps the graph consistent.

### Consequences

<consequence kind="good">

No dangling references by default; deletes are atomic; no ID-reuse hazard.

</consequence>

<consequence kind="bad">

Delete cost scales with annotation fan-out and needs a per-transaction cascade limit. The 64-bit ID space is consumed monotonically and must be sized accordingly. Cypher `DELETE` and SPARQL `DELETE DATA` semantics need documented mappings.

</consequence>

## More Information

Resolves handover open question 3.

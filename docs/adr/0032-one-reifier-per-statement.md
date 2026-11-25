---
status: accepted
date: 2026-11-24
decision-makers: [Volodia]
---
# One reifier per statement, aliases and unasserted statements

## Context and Problem Statement

RDF 1.2 allows several reifiers per triple and reifiers of triples that are not asserted. graphlava makes the statement ID the reifier ([ADR-0004](0004-unique-statements-multi-opt-in.md)), which gives zero-join annotation lookup.

## Considered Options

<option id="one-reifier-alias-unasserted" outcome="chosen">

One reifier per statement: the statement ID. A second reifier IRI for the same triple, including an update that names a new IRI for a triple that already has an ID, is recorded as an alias of that ID. A reifier of a triple that is not asserted creates a statement with an `unasserted` flag in `stmt`; orderings skip it for normal matching, reifier lookups see it.

</option>

<option id="many-reifiers" outcome="rejected">

Separate reifier nodes linked by virtual `rdf:reifies`: full RDF 1.2, but every annotation lookup needs a join.

</option>

<option id="many-for-multi" outcome="deferred">

Several reifiers only for `multi` predicates.

</option>

<option id="reject-unasserted" outcome="rejected">

Reject annotations on unasserted triples: breaks RDF 1.2 claims and quotes.

</option>

<option id="error-on-new-reifier" outcome="rejected">

Reject an update that names a new reifier for an existing statement.

</option>

## Decision Outcome

Chosen option: "one-reifier-alias-unasserted", because it keeps one ID scheme and the zero-join reifier binding, and per-episode metadata is covered by annotating the promoted membership `<< X :in g >>`.

### Consequences

<consequence kind="good">

Annotation lookup stays a point read; RDF 1.2 data with quoted claims loads.

</consequence>

<consequence kind="bad">

Two reifiers of one triple cannot carry separate annotations; aliases need an alias table; every ordering scan must skip `unasserted` statements.

</consequence>

## More Information

Resolves open questions "Multiple reifiers per triple", "Reifier on existing statement" and "Unasserted triple terms".

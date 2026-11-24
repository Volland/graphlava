---
status: accepted
date: 2026-11-24
decision-makers: [Volodia]
---
# Functional predicates update in place; literal hyperedge roles are properties of the hyperedge

## Context and Problem Statement

`set(s,p,o)` on a node-valued predicate deletes the old statement and inserts a new one, so its annotations are cascaded away on every change of, for example, `:currentLocation`. The planner has no declared cardinality for such predicates. Separately, hyperedge roles may only be filled by nodes, and literal role values (a meeting's time) had no home.

## Considered Options

<option id="functional-and-literal-props" outcome="chosen">

- The meta-model may declare a predicate `functional`: at most one object per subject. `set` and `insert` on a functional node-valued predicate with an existing statement update its object in place and keep its ID and annotations. The planner treats `(s, p)` as a key. `functional` and `multi` are exclusive.
- A literal-valued role is an inline property of the hyperedge node `e` (in `props[e]`), not a role. Role fillers stay node-valued, so `incidence` never holds literals.

</option>

<option id="status-quo" outcome="rejected">

Keep delete-and-insert for `set` and leave literal roles open: loses annotations on every overwrite of a single-valued edge.

</option>

## Decision Outcome

Chosen option: "functional-and-literal-props", because overwrites of single-valued edges keep their history-free identity and annotations, and literal role values reuse inline properties with no new structure.

### Consequences

<consequence kind="good">

Stable IDs for single-valued edges; exact cardinality for the planner; hyperedges carry literal attributes at one-record cost.

</consequence>

<consequence kind="bad">

Declaring a predicate `functional` over existing data with several objects per subject is rejected until the data is fixed. Literal hyperedge attributes are not visible through `incidence`; finding hyperedges by a literal value needs a value index on that predicate.

</consequence>

## More Information

Amends [ADR-0004](0004-unique-statements-multi-opt-in.md) (predicate declarations). Resolves open question "Hyperedge role fillers that are literals".

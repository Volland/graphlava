---
status: accepted
date: 2026-11-24
decision-makers: [Volodia]
---
# A 16-bit append-only language tag table

## Context and Problem Statement

Inline and dictionary language strings carry a 16-bit language-tag slot ([ADR-0011](0011-id-layout.md)); the size and growth of the table behind it were open.

## Considered Options

<option id="table16" outcome="chosen">

A persistent append-only table of up to 65,535 canonical (lower-cased BCP 47) tags. When the table is full, a literal with a new tag becomes a dictionary literal carrying the full tag.

</option>

<option id="tags-as-iris" outcome="rejected">

Intern tags as dictionary IRIs: unbounded, but the 16-bit inline slot cannot hold them.

</option>

## Decision Outcome

Chosen option: "table16", because real data uses few distinct tags, and overflow stays correct through the dictionary.

### Consequences

<consequence kind="good">

Language strings stay inline and order-preserving for nearly all data.

</consequence>

<consequence kind="bad">

Tags are never removed from the table; the overflow path is rarely exercised and needs its own test.

</consequence>

## More Information

Resolves open question "Language tag table".

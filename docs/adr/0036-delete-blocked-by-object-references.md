---
status: accepted
date: 2026-11-24
decision-makers: [Volodia]
---
# Deleting a statement referenced as an object is blocked unless DETACH

## Context and Problem Statement

A statement whose object is a statement ID, such as `(:note, :about, stmt#42)`, is not an annotation, so the cascade of [ADR-0006](0006-cascade-delete-annotations.md) does not cover it.

## Considered Options

<option id="block-unless-detach" outcome="chosen">

Deleting a statement that appears in object position of another statement fails unless the delete is `DETACH`, as for hyperedge roles. `DETACH` deletes the referring statements as well.

</option>

<option id="cascade" outcome="rejected">

Cascade silently: deletes data the user may want to keep.

</option>

<option id="dangle" outcome="rejected">

Keep a dangling object: breaks the no-dangling rule of ADR-0006.

</option>

## Decision Outcome

Chosen option: "block-unless-detach", because dangling object IDs are never created silently and the rule matches the hyperedge guard.

### Consequences

<consequence kind="good">

One guard rule for roles and object references; the store stays reference-closed.

</consequence>

<consequence kind="bad">

Deletes must check an object-position index (OSP) for statement IDs; callers must handle the new error.

</consequence>

## More Information

Amends [ADR-0006](0006-cascade-delete-annotations.md). Resolves open question "Delete with object-position references".

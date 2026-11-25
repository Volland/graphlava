---
status: accepted
date: 2026-11-24
decision-makers: [Volodia]
---
# Project name graphlava and the https namespace

## Context and Problem Statement

`meta-core` was a working name. The crate names, the file magic, the system IRI namespace and the reifier IRIs all carry the name, so it must be fixed before the format freeze ([ADR-0018](0018-portable-file-format.md)).

## Considered Options

<option id="graphlava-https" outcome="chosen">

Name the project and crates `graphlava` (`graphlava`, `graphlava-ffi`). Meta-model terms live in `https://graphlava.org/ns#` (prefix `meta:`). Statement reifiers are `https://graphlava.org/ns/stmt/{id}` with `{id}` the decimal 64-bit statement ID. The file magic is derived from the name.

</option>

<option id="urn-namespace" outcome="rejected">

A URN namespace `urn:graphlava:`: no DNS dependency, but not dereferenceable; the project already owns graphlava.org.

</option>

<option id="keep-meta-core" outcome="rejected">

Keep `meta-core`: generic and hard to search.

</option>

## Decision Outcome

Chosen option: "graphlava-https", because the name matches the repository and site, and the project controls graphlava.org, so vocabulary IRIs can be served there later.

### Consequences

<consequence kind="good">

One name everywhere; vocabulary and reifier IRIs are dereferenceable once the site serves them.

</consequence>

<consequence kind="bad">

The namespace depends on keeping the graphlava.org domain. The parser must recognise the reifier IRI prefix and map it back to an ID without a dictionary lookup.

</consequence>

## More Information

Resolves open questions "Project name", "System namespace IRI" and "Reifier IRI form".

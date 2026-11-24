---
status: accepted
date: 2026-11-24
decision-makers: [Volodia]
---
# Page and block sizes are chosen by measurement

## Context and Problem Statement

redb's page size and overflow handling affect the v2 block size, and 16 KB-page Android devices and Apple Silicon may prefer 16 KB database pages.

## Considered Options

<option id="measure" outcome="chosen">

Measure redb page behaviour in the engine bake-off and size v2 blocks to fit one redb page. Run the mobile benchmark on a 16 KB-page Android device before choosing anything engine-specific. No page-size constant enters the file format until both are measured.

</option>

<option id="assume-4k" outcome="rejected">

Assume 4 KB pages everywhere: unmeasured.

</option>

<option id="fork-redb" outcome="deferred">

Patch redb for a configurable page size: maintenance burden.

</option>

## Decision Outcome

Chosen option: "measure", because the decision depends on numbers nobody has yet.

### Consequences

<consequence kind="good">

No unmeasured page-size assumption is frozen into the format.

</consequence>

<consequence kind="bad">

The v2 block size and mobile tuning stay open until the bake-off and the device benchmark run.

</consequence>

## More Information

Resolves open questions "Page size on redb" and "Page size on 16 KB devices".

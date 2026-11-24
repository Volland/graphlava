---
status: accepted
date: 2026-11-24
decision-makers: [Volodia]
---
# Property records spill at 256 entries or 2 KiB

## Context and Problem Statement

Nodes with very many literal properties make the `props` record large; the threshold and layout for spilling to `props_ext` were open.

## Considered Options

<option id="spill256" outcome="chosen">

Spill to `props_ext` above 256 entries or 2 KiB, whichever comes first. Both numbers are tuned by benchmark before the format freeze.

</option>

<option id="always-split" outcome="rejected">

One key per property: loses the one-lookup read of a node.

</option>

## Decision Outcome

Chosen option: "spill256", because typical agent-memory nodes keep the one-lookup read while outliers do not rewrite a huge record on every update.

### Consequences

<consequence kind="good">

Small nodes read in one lookup; large ones stay bounded in write cost.

</consequence>

<consequence kind="bad">

Two record layouts to maintain and test; the threshold is a format-visible constant until the freeze.

</consequence>

## More Information

Resolves open question "Large property records".

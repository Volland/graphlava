# Design

## Context

The full design lives in `tg.md/` (specs 00–21) and the decisions in `docs/adr/` (0002–0041). This slice of the former `v0-core` change summarises only what shapes its own work.

## Decisions

| Decision | ADR |
| --- | --- |
| One portable, versioned file format, frozen at the end of v0 | [[adr:0018]] |
| Lean model and refinement testing | [[adr:0019]] |
| Instruction-count PR gate, statistical nightly wall-clock | [[adr:0020]] |
| Page and block sizes chosen by measurement | [[adr:0035]] |

## Freeze criteria

The format freezes only when: PO-ID-STRING, PO-ID-DOUBLE and PO-CASCADE-FUEL are closed (in `v0-store-core`), the nightly baselines exist and two runs agree within their confidence intervals, the golden-file job passes on Linux, macOS and Windows, and the property-record spill and page-size numbers are fixed by measurement.

## Risks / Trade-offs

- [A fixed nightly runner is infrastructure outside the repository] → tasks 13.5 and 13.6 need that machine; until it exists the freeze waits.

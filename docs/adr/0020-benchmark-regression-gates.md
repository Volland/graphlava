---
status: proposed
date: 2026-11-24
decision-makers: [Volodia]
---
# Benchmarks gate merges: instruction counts per PR, statistical wall-clock nightly

## Context and Problem Statement

Performance on the agent-memory mix is goal 1 ([[00-overview#Goals]]). Wall-clock numbers on shared CI runners vary by 10–30%, which hides real regressions and raises false alarms. We need gates that are deterministic enough to block a merge, and real-time numbers that are trustworthy enough to set targets.

## Considered Options

<option id="two-tier" outcome="chosen">

Two tiers. Per pull request: micro benchmarks measured in instructions and cache accesses under Cachegrind (via `iai-callgrind`), compared with `main`; a rise above 3% fails the check. Nightly on one fixed, dedicated machine: wall-clock macro workloads with HDR histograms, at least 5 runs, and a regression flagged when the bootstrap 95% confidence interval of the change excludes the threshold.

</option>

<option id="wallclock-pr" outcome="rejected">

Wall-clock criterion runs on shared PR runners. Too noisy to gate on; teams learn to ignore the alarms.

</option>

<option id="no-gate" outcome="rejected">

Benchmarks run by hand before releases. Regressions are found late, and bisecting them across many merges is expensive.

</option>

## Decision Outcome

Chosen option: "two-tier", because instruction counts are reproducible to well under 1% and can block merges, while the nightly tier measures what users feel (latency percentiles, fsync behaviour, file size) on stable hardware.

### Consequences

<consequence kind="good">

Every performance claim has a stored baseline and a history; regressions are caught at the merge that causes them.

</consequence>

<consequence kind="bad">

Instruction counts miss I/O and fsync cost, so durability and file-size regressions surface only nightly. A dedicated runner must be kept.

</consequence>

## More Information

Workloads, metrics and pass lines: `tg.md/17-benchmarks.md`. Gates: `tg.md/19-verification.md#CI gates`.
